//! Conflict-safe, reversible Agent Skill setup for supported local hosts.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use directories::BaseDirs;
use fs2::FileExt;
use heyfood_platform::{AtomicFile, NativePaths};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const PACKAGE_VERSION: &str = "0.6.0";
const LOCK_TIMEOUT: Duration = Duration::from_secs(3);
const LOCK_RETRY: Duration = Duration::from_millis(10);
const CODEX_VERSION: &str = "codex-cli 0.145.0-alpha.18";
const CLAUDE_VERSION: &str = "2.1.128 (Claude Code)";

const SKILL_FILES: &[(&str, &str)] = &[
    (
        "SKILL.md",
        include_str!("../../../agent-integrations/skills/heyfood/SKILL.md"),
    ),
    (
        "agents/openai.yaml",
        include_str!("../../../agent-integrations/skills/heyfood/agents/openai.yaml"),
    ),
    (
        "references/authentication-and-capabilities.md",
        include_str!(
            "../../../agent-integrations/skills/heyfood/references/authentication-and-capabilities.md"
        ),
    ),
    (
        "references/grocery.md",
        include_str!("../../../agent-integrations/skills/heyfood/references/grocery.md"),
    ),
    (
        "references/safety-and-recovery.md",
        include_str!(
            "../../../agent-integrations/skills/heyfood/references/safety-and-recovery.md"
        ),
    ),
    (
        "references/workflow-selection.md",
        include_str!("../../../agent-integrations/skills/heyfood/references/workflow-selection.md"),
    ),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupTarget {
    Codex,
    Claude,
    All,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupScope {
    User,
    Project,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupOperation {
    Install,
    Uninstall,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupMode {
    DryRun,
    Apply,
}

#[derive(Clone, Debug)]
pub struct SetupOptions {
    pub target: SetupTarget,
    pub scope: SetupScope,
    pub project_root: Option<PathBuf>,
    pub operation: SetupOperation,
    pub mode: SetupMode,
    pub replace: bool,
}

#[derive(Clone, Debug)]
pub struct HostProbe {
    pub host: Host,
    pub executable: Option<PathBuf>,
    pub version: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Host {
    Codex,
    Claude,
}

impl Host {
    const fn name(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }

    const fn compatible_version(self) -> &'static str {
        match self {
            Self::Codex => CODEX_VERSION,
            Self::Claude => CLAUDE_VERSION,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SetupEnvironment {
    pub home_dir: PathBuf,
    pub state_dir: PathBuf,
    pub heyfood_executable: PathBuf,
    pub probes: Vec<HostProbe>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BinaryIdentity {
    pub path: PathBuf,
    pub sha256: String,
    pub version: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SkillPackageIdentity {
    pub name: &'static str,
    pub version: &'static str,
    pub sha256: String,
    pub files: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct HostSetupPlan {
    pub host: Host,
    pub host_executable: Option<PathBuf>,
    pub host_version: Option<String>,
    pub compatible_version: &'static str,
    pub compatibility: &'static str,
    pub skill_path: PathBuf,
    pub receipt_path: PathBuf,
    pub action: &'static str,
    pub conflicts: Vec<String>,
    pub user_actions: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SetupPlan {
    pub schema_version: u32,
    pub operation: SetupOperation,
    pub mode: SetupMode,
    pub target: SetupTarget,
    pub scope: SetupScope,
    pub project_root: Option<PathBuf>,
    pub binary: BinaryIdentity,
    pub package: SkillPackageIdentity,
    pub ready: bool,
    pub changed: bool,
    pub hosts: Vec<HostSetupPlan>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetupError {
    pub kind: &'static str,
    pub message: String,
    pub hint: Option<String>,
    pub uncertain: bool,
}

impl SetupError {
    fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            hint: None,
            uncertain: false,
        }
    }

    fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    fn uncertain(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            hint: Some(
                "Inspect `heyfood agent setup --dry-run` before attempting recovery.".to_owned(),
            ),
            uncertain: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Receipt {
    schema_version: u32,
    host: Host,
    host_executable: PathBuf,
    host_version: String,
    scope: SetupScope,
    project_root: Option<PathBuf>,
    heyfood_executable: PathBuf,
    heyfood_sha256: String,
    package_version: String,
    package_sha256: String,
    skill_path: PathBuf,
    files: BTreeMap<String, String>,
}

impl<'de> Deserialize<'de> for Host {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "codex" => Ok(Self::Codex),
            "claude" => Ok(Self::Claude),
            _ => Err(serde::de::Error::custom("unknown host")),
        }
    }
}

impl<'de> Deserialize<'de> for SetupScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "user" => Ok(Self::User),
            "project" => Ok(Self::Project),
            _ => Err(serde::de::Error::custom("unknown setup scope")),
        }
    }
}

pub fn discover_environment() -> Result<SetupEnvironment, SetupError> {
    let home_dir = BaseDirs::new()
        .map(|base| base.home_dir().to_owned())
        .ok_or_else(|| SetupError::new("agent_setup_home", "user home directory is unavailable"))?;
    let state_dir = NativePaths::discover()
        .map_err(|error| SetupError::new("agent_setup_state", error.message))?
        .config_dir()
        .join("agent-integrations");
    let heyfood_executable = env::current_exe().and_then(fs::canonicalize).map_err(|_| {
        SetupError::new(
            "agent_setup_binary",
            "the exact running heyfood executable could not be resolved",
        )
    })?;
    Ok(SetupEnvironment {
        home_dir,
        state_dir,
        heyfood_executable,
        probes: vec![probe_host(Host::Codex), probe_host(Host::Claude)],
    })
}

pub fn execute(options: &SetupOptions) -> Result<SetupPlan, SetupError> {
    let environment = discover_environment()?;
    execute_with_environment(options, &environment)
}

pub fn execute_with_environment(
    options: &SetupOptions,
    environment: &SetupEnvironment,
) -> Result<SetupPlan, SetupError> {
    let mut plan = build_plan(options, environment)?;
    if options.mode == SetupMode::DryRun {
        return Ok(plan);
    }
    if !plan.ready {
        return Err(SetupError::new(
            "agent_setup_not_ready",
            "agent integration setup has unresolved compatibility or file conflicts",
        )
        .hint("Review the dry-run plan and resolve every reported conflict before applying."));
    }

    let _lock = SetupLock::acquire(&environment.state_dir.join("setup.lock"))?;
    plan = build_plan(options, environment)?;
    if !plan.ready {
        return Err(SetupError::new(
            "agent_setup_changed",
            "agent integration setup state changed after planning",
        )
        .hint("Run the dry-run again and review the current plan."));
    }

    let package = package_identity();
    let binary = binary_identity(&environment.heyfood_executable)?;
    let probes = probe_map(environment);
    let mut completed = Vec::new();
    for host_plan in &plan.hosts {
        if host_plan.action == "none" {
            continue;
        }
        let probe = probes
            .get(&host_plan.host)
            .expect("every selected host has a probe");
        let result = match options.operation {
            SetupOperation::Install => {
                install_host(options, host_plan, probe, &package, &binary, environment)
            }
            SetupOperation::Uninstall => uninstall_host(host_plan),
        };
        if let Err(error) = result {
            if error.uncertain {
                return Err(error);
            }
            if options.operation == SetupOperation::Install {
                for completed_plan in completed.iter().rev() {
                    if uninstall_host(completed_plan).is_err() {
                        return Err(SetupError::uncertain(
                            "agent_setup_rollback",
                            "agent integration apply failed and rollback could not be verified",
                        ));
                    }
                }
            }
            return Err(error);
        }
        completed.push(host_plan.clone());
    }
    plan.changed = !completed.is_empty();
    Ok(plan)
}

fn build_plan(
    options: &SetupOptions,
    environment: &SetupEnvironment,
) -> Result<SetupPlan, SetupError> {
    let project_root = normalize_project_root(options)?;
    let binary = binary_identity(&environment.heyfood_executable)?;
    let package = package_identity();
    let probes = probe_map(environment);
    let selected = match options.target {
        SetupTarget::Codex => vec![Host::Codex],
        SetupTarget::Claude => vec![Host::Claude],
        SetupTarget::All => vec![Host::Codex, Host::Claude],
    };
    let mut hosts = Vec::new();
    for host in selected {
        let probe = probes.get(&host).cloned().unwrap_or(HostProbe {
            host,
            executable: None,
            version: None,
        });
        hosts.push(plan_host(
            options,
            environment,
            project_root.as_deref(),
            &binary,
            &package,
            &probe,
        )?);
    }
    if options.target == SetupTarget::All {
        for host in &mut hosts {
            if host.action == "replace" {
                host.conflicts.push(
                    "replace each host separately so an older installation remains recoverable"
                        .to_owned(),
                );
                host.action = "conflict";
            }
        }
    }
    let ready = hosts.iter().all(|host| {
        host.conflicts.is_empty()
            && host.compatibility == "compatible"
            && matches!(host.action, "install" | "replace" | "uninstall" | "none")
    });
    Ok(SetupPlan {
        schema_version: 1,
        operation: options.operation,
        mode: options.mode,
        target: options.target,
        scope: options.scope,
        project_root,
        binary,
        package,
        ready,
        changed: false,
        hosts,
    })
}

fn plan_host(
    options: &SetupOptions,
    environment: &SetupEnvironment,
    project_root: Option<&Path>,
    binary: &BinaryIdentity,
    package: &SkillPackageIdentity,
    probe: &HostProbe,
) -> Result<HostSetupPlan, SetupError> {
    let compatibility = match (&probe.executable, &probe.version) {
        (Some(_), Some(version)) if version == probe.host.compatible_version() => "compatible",
        (Some(_), Some(_)) => "incompatible",
        _ => "missing",
    };
    let skill_path = skill_path(
        probe.host,
        options.scope,
        &environment.home_dir,
        project_root,
    )?;
    let receipt_path = receipt_path(
        &environment.state_dir,
        probe.host,
        options.scope,
        project_root,
    );
    let mut conflicts = Vec::new();
    let receipt = load_receipt(&receipt_path)?;
    let current = inspect_skill(&skill_path)?;
    let action = match options.operation {
        SetupOperation::Install => match (&current, &receipt) {
            (None, _) => "install",
            (Some(files), Some(receipt))
                if receipt_matches_current(
                    receipt,
                    options,
                    project_root,
                    &skill_path,
                    package,
                    binary,
                    probe,
                ) && *files == receipt.files =>
            {
                "none"
            }
            (Some(files), Some(receipt))
                if options.replace
                    && receipt.skill_path == skill_path
                    && *files == receipt.files =>
            {
                "replace"
            }
            (Some(_), _) => {
                conflicts.push(
                    "existing skill is not the exact receipt-bound heyfood installation".to_owned(),
                );
                "conflict"
            }
        },
        SetupOperation::Uninstall => match (&current, &receipt) {
            (None, None) => "none",
            (Some(files), Some(receipt))
                if receipt.skill_path == skill_path && *files == receipt.files =>
            {
                "uninstall"
            }
            (None, Some(_)) => {
                conflicts.push("receipt exists but the installed skill is missing".to_owned());
                "conflict"
            }
            (Some(_), Some(_)) => {
                conflicts.push(
                    "installed skill was modified; uninstall will preserve user files".to_owned(),
                );
                "conflict"
            }
            (Some(_), None) => {
                conflicts.push("skill exists without a heyfood setup receipt".to_owned());
                "conflict"
            }
        },
    };
    let mut user_actions = Vec::new();
    if compatibility == "missing" {
        user_actions.push(format!(
            "Install {} {} before applying this integration.",
            probe.host.name(),
            probe.host.compatible_version()
        ));
    } else if compatibility == "incompatible" {
        user_actions.push(format!(
            "Use the qualified {} host version {}.",
            probe.host.name(),
            probe.host.compatible_version()
        ));
    }
    if options.scope == SetupScope::Project && probe.host == Host::Claude {
        user_actions.push(
            "Open the explicit project in Claude Code and complete its normal trust decision."
                .to_owned(),
        );
    }
    Ok(HostSetupPlan {
        host: probe.host,
        host_executable: probe.executable.clone(),
        host_version: probe.version.clone(),
        compatible_version: probe.host.compatible_version(),
        compatibility,
        skill_path,
        receipt_path,
        action,
        conflicts,
        user_actions,
    })
}

fn install_host(
    options: &SetupOptions,
    plan: &HostSetupPlan,
    probe: &HostProbe,
    package: &SkillPackageIdentity,
    binary: &BinaryIdentity,
    _environment: &SetupEnvironment,
) -> Result<(), SetupError> {
    validate_destination(&plan.skill_path)?;
    let parent = plan.skill_path.parent().ok_or_else(|| {
        SetupError::new(
            "agent_setup_path",
            "skill destination has no parent directory",
        )
    })?;
    create_directories_without_redirect(parent)?;
    let stage = parent.join(format!(".heyfood.{}.stage", std::process::id()));
    if fs::symlink_metadata(&stage).is_ok() {
        return Err(SetupError::new(
            "agent_setup_stage",
            "setup staging path already exists",
        ));
    }
    fs::create_dir(&stage)
        .map_err(|error| SetupError::new("agent_setup_stage", error.to_string()))?;
    let staged = write_skill_files(&stage);
    if let Err(error) = staged {
        let _ = fs::remove_dir_all(&stage);
        return Err(error);
    }
    let backup = parent.join(format!(".heyfood.{}.backup", std::process::id()));
    let replacing = plan.action == "replace";
    if replacing {
        if fs::symlink_metadata(&backup).is_ok() {
            let _ = fs::remove_dir_all(&stage);
            return Err(SetupError::new(
                "agent_setup_backup",
                "setup backup path already exists",
            ));
        }
        fs::rename(&plan.skill_path, &backup).map_err(|error| {
            let _ = fs::remove_dir_all(&stage);
            SetupError::new("agent_setup_replace", error.to_string())
        })?;
    }
    if let Err(error) = fs::rename(&stage, &plan.skill_path) {
        if replacing {
            let _ = fs::rename(&backup, &plan.skill_path);
        }
        let _ = fs::remove_dir_all(&stage);
        return Err(SetupError::new("agent_setup_commit", error.to_string()));
    }

    let receipt = Receipt {
        schema_version: 1,
        host: plan.host,
        host_executable: probe
            .executable
            .clone()
            .expect("compatible host has an executable"),
        host_version: probe
            .version
            .clone()
            .expect("compatible host has a version"),
        scope: options.scope,
        project_root: normalize_project_root(options)?,
        heyfood_executable: binary.path.clone(),
        heyfood_sha256: binary.sha256.clone(),
        package_version: package.version.to_owned(),
        package_sha256: package.sha256.clone(),
        skill_path: plan.skill_path.clone(),
        files: expected_file_digests(),
    };
    let mut bytes = serde_json::to_vec(&receipt)
        .map_err(|error| SetupError::new("agent_setup_receipt", error.to_string()))?;
    bytes.push(b'\n');
    if let Err(error) = AtomicFile::replace(&plan.receipt_path, &bytes) {
        let _ = fs::remove_dir_all(&plan.skill_path);
        if replacing {
            let _ = fs::rename(&backup, &plan.skill_path);
        }
        return Err(SetupError::new("agent_setup_receipt", error.message));
    }
    if replacing && fs::remove_dir_all(&backup).is_err() {
        return Err(SetupError::uncertain(
            "agent_setup_cleanup",
            "the new skill and receipt are committed but the prior backup could not be removed",
        ));
    }
    Ok(())
}

fn receipt_matches_current(
    receipt: &Receipt,
    options: &SetupOptions,
    project_root: Option<&Path>,
    skill_path: &Path,
    package: &SkillPackageIdentity,
    binary: &BinaryIdentity,
    probe: &HostProbe,
) -> bool {
    receipt.host == probe.host
        && receipt.host_executable.as_path() == probe.executable.as_deref().unwrap_or(Path::new(""))
        && receipt.host_version.as_str() == probe.version.as_deref().unwrap_or("")
        && receipt.scope == options.scope
        && receipt.project_root.as_deref() == project_root
        && receipt.heyfood_executable == binary.path
        && receipt.heyfood_sha256 == binary.sha256
        && receipt.package_version == package.version
        && receipt.package_sha256 == package.sha256
        && receipt.skill_path == skill_path
}

fn uninstall_host(plan: &HostSetupPlan) -> Result<(), SetupError> {
    if plan.action == "none" {
        return Ok(());
    }
    validate_destination(&plan.skill_path)?;
    let parent = plan.skill_path.parent().ok_or_else(|| {
        SetupError::new(
            "agent_setup_path",
            "skill destination has no parent directory",
        )
    })?;
    let tombstone = parent.join(format!(".heyfood.{}.uninstall", std::process::id()));
    if fs::symlink_metadata(&tombstone).is_ok() {
        return Err(SetupError::new(
            "agent_setup_uninstall",
            "uninstall staging path already exists",
        ));
    }
    fs::rename(&plan.skill_path, &tombstone)
        .map_err(|error| SetupError::new("agent_setup_uninstall", error.to_string()))?;
    if let Err(error) = fs::remove_file(&plan.receipt_path) {
        let _ = fs::rename(&tombstone, &plan.skill_path);
        return Err(SetupError::new("agent_setup_uninstall", error.to_string()));
    }
    fs::remove_dir_all(&tombstone).map_err(|_| {
        SetupError::uncertain(
            "agent_setup_uninstall_cleanup",
            "the receipt was removed but installed skill cleanup could not be verified",
        )
    })
}

fn write_skill_files(root: &Path) -> Result<(), SetupError> {
    for (relative, contents) in SKILL_FILES {
        let destination = root.join(relative);
        let parent = destination
            .parent()
            .expect("every skill file has a parent directory");
        fs::create_dir_all(parent)
            .map_err(|error| SetupError::new("agent_setup_write", error.to_string()))?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|error| SetupError::new("agent_setup_write", error.to_string()))?;
        file.write_all(contents.as_bytes())
            .and_then(|()| file.flush())
            .and_then(|()| file.sync_all())
            .map_err(|error| SetupError::new("agent_setup_write", error.to_string()))?;
    }
    Ok(())
}

fn normalize_project_root(options: &SetupOptions) -> Result<Option<PathBuf>, SetupError> {
    match (options.scope, options.project_root.as_deref()) {
        (SetupScope::User, None) => Ok(None),
        (SetupScope::User, Some(_)) => Err(SetupError::new(
            "agent_setup_project_root",
            "--project-root is valid only with --scope project",
        )),
        (SetupScope::Project, None) => Err(SetupError::new(
            "agent_setup_project_root",
            "project scope requires an explicit absolute --project-root",
        )),
        (SetupScope::Project, Some(root)) if !root.is_absolute() => Err(SetupError::new(
            "agent_setup_project_root",
            "project scope requires an absolute --project-root",
        )),
        (SetupScope::Project, Some(root)) => {
            if root
                .components()
                .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
            {
                return Err(SetupError::new(
                    "agent_setup_project_root",
                    "project root must not contain relative path components",
                ));
            }
            validate_existing_directory(root)?;
            let git = root.join(".git");
            let git_metadata = fs::symlink_metadata(&git).map_err(|_| {
                SetupError::new(
                    "agent_setup_project_root",
                    "project root must identify an existing Git worktree",
                )
            })?;
            if redirects(&git_metadata) {
                return Err(SetupError::new(
                    "agent_setup_project_root",
                    "project Git identity must not be a symlink or reparse point",
                ));
            }
            fs::canonicalize(root)
                .map(Some)
                .map_err(|error| SetupError::new("agent_setup_project_root", error.to_string()))
        }
    }
}

fn skill_path(
    host: Host,
    scope: SetupScope,
    home: &Path,
    project_root: Option<&Path>,
) -> Result<PathBuf, SetupError> {
    let base = match scope {
        SetupScope::User => match host {
            Host::Codex => home.join(".agents").join("skills"),
            Host::Claude => home.join(".claude").join("skills"),
        },
        SetupScope::Project => {
            let root = project_root.ok_or_else(|| {
                SetupError::new(
                    "agent_setup_project_root",
                    "project scope requires a project root",
                )
            })?;
            match host {
                Host::Codex => root.join(".agents").join("skills"),
                Host::Claude => root.join(".claude").join("skills"),
            }
        }
    };
    Ok(base.join("heyfood"))
}

fn receipt_path(
    state_dir: &Path,
    host: Host,
    scope: SetupScope,
    project_root: Option<&Path>,
) -> PathBuf {
    let mut digest = Sha256::new();
    digest.update(host.name().as_bytes());
    digest.update([0]);
    digest.update(match scope {
        SetupScope::User => b"user".as_slice(),
        SetupScope::Project => b"project".as_slice(),
    });
    if let Some(root) = project_root {
        digest.update([0]);
        digest.update(root.as_os_str().as_encoded_bytes());
    }
    state_dir
        .join("receipts")
        .join(format!("{}.json", hex(&digest.finalize())))
}

fn load_receipt(path: &Path) -> Result<Option<Receipt>, SetupError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(SetupError::new("agent_setup_receipt", error.to_string())),
    };
    if redirects(&metadata) || !metadata.is_file() || hardlinked(path, &metadata) {
        return Err(SetupError::new(
            "agent_setup_receipt",
            "setup receipt must be one regular, unlinked file",
        ));
    }
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(64 * 1024).read_to_end(&mut bytes))
        .map_err(|error| SetupError::new("agent_setup_receipt", error.to_string()))?;
    let receipt: Receipt = serde_json::from_slice(&bytes)
        .map_err(|_| SetupError::new("agent_setup_receipt", "setup receipt is malformed"))?;
    if receipt.schema_version != 1 {
        return Err(SetupError::new(
            "agent_setup_receipt",
            "setup receipt version is unsupported",
        ));
    }
    Ok(Some(receipt))
}

fn inspect_skill(path: &Path) -> Result<Option<BTreeMap<String, String>>, SetupError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(SetupError::new("agent_setup_inspect", error.to_string())),
    };
    if redirects(&metadata) || !metadata.is_dir() {
        return Err(SetupError::new(
            "agent_setup_redirect",
            "skill destination must be a regular directory, not a redirect",
        ));
    }
    let mut files = BTreeMap::new();
    inspect_directory(path, path, &mut files)?;
    Ok(Some(files))
}

fn inspect_directory(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<String, String>,
) -> Result<(), SetupError> {
    for entry in fs::read_dir(directory)
        .map_err(|error| SetupError::new("agent_setup_inspect", error.to_string()))?
    {
        let entry =
            entry.map_err(|error| SetupError::new("agent_setup_inspect", error.to_string()))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| SetupError::new("agent_setup_inspect", error.to_string()))?;
        if redirects(&metadata) {
            return Err(SetupError::new(
                "agent_setup_redirect",
                "installed skill contains a symlink or reparse point",
            ));
        }
        if metadata.is_dir() {
            inspect_directory(root, &path, files)?;
        } else if metadata.is_file() && !hardlinked(&path, &metadata) {
            let relative = path
                .strip_prefix(root)
                .expect("inspected path is below skill root")
                .to_string_lossy()
                .replace('\\', "/");
            files.insert(relative, sha256_file(&path)?);
        } else {
            return Err(SetupError::new(
                "agent_setup_redirect",
                "installed skill contains a non-regular or linked file",
            ));
        }
    }
    Ok(())
}

fn validate_destination(path: &Path) -> Result<(), SetupError> {
    if let Some(parent) = path.parent() {
        validate_existing_ancestors(parent)?;
    }
    let _ = inspect_skill(path)?;
    Ok(())
}

fn validate_existing_directory(path: &Path) -> Result<(), SetupError> {
    validate_existing_ancestors(path)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| SetupError::new("agent_setup_path", error.to_string()))?;
    if redirects(&metadata) || !metadata.is_dir() {
        return Err(SetupError::new(
            "agent_setup_path",
            "setup path must be an existing regular directory",
        ));
    }
    Ok(())
}

fn validate_existing_ancestors(path: &Path) -> Result<(), SetupError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if redirects(&metadata) => {
                return Err(SetupError::new(
                    "agent_setup_redirect",
                    "setup path contains a symlink or reparse point",
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(SetupError::new(
                    "agent_setup_path",
                    "setup path contains a non-directory ancestor",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(SetupError::new("agent_setup_path", error.to_string())),
        }
    }
    Ok(())
}

fn create_directories_without_redirect(path: &Path) -> Result<(), SetupError> {
    validate_existing_ancestors(path)?;
    fs::create_dir_all(path)
        .map_err(|error| SetupError::new("agent_setup_path", error.to_string()))?;
    validate_existing_ancestors(path)
}

fn package_identity() -> SkillPackageIdentity {
    let mut digest = Sha256::new();
    for (relative, contents) in SKILL_FILES {
        digest.update((relative.len() as u64).to_be_bytes());
        digest.update(relative.as_bytes());
        digest.update((contents.len() as u64).to_be_bytes());
        digest.update(contents.as_bytes());
    }
    SkillPackageIdentity {
        name: "heyfood",
        version: PACKAGE_VERSION,
        sha256: hex(&digest.finalize()),
        files: SKILL_FILES.len(),
    }
}

fn expected_file_digests() -> BTreeMap<String, String> {
    SKILL_FILES
        .iter()
        .map(|(relative, contents)| {
            (
                (*relative).to_owned(),
                hex(&Sha256::digest(contents.as_bytes())),
            )
        })
        .collect()
}

fn binary_identity(path: &Path) -> Result<BinaryIdentity, SetupError> {
    Ok(BinaryIdentity {
        path: path.to_owned(),
        sha256: sha256_file(path)?,
        version: PACKAGE_VERSION,
    })
}

fn sha256_file(path: &Path) -> Result<String, SetupError> {
    let mut file = File::open(path)
        .map_err(|error| SetupError::new("agent_setup_digest", error.to_string()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| SetupError::new("agent_setup_digest", error.to_string()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex(&digest.finalize()))
}

fn probe_map(environment: &SetupEnvironment) -> BTreeMap<Host, HostProbe> {
    environment
        .probes
        .iter()
        .cloned()
        .map(|probe| (probe.host, probe))
        .collect()
}

fn probe_host(host: Host) -> HostProbe {
    let executable = find_executable(host.name());
    let version = executable.as_ref().and_then(|executable| {
        Command::new(executable)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|value| value.trim().to_owned())
    });
    HostProbe {
        host,
        executable,
        version,
    }
}

fn find_executable(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    let extensions: Vec<OsString> = if cfg!(windows) {
        env::var_os("PATHEXT")
            .unwrap_or_else(|| OsString::from(".COM;.EXE;.BAT;.CMD"))
            .to_string_lossy()
            .split(';')
            .map(OsString::from)
            .collect()
    } else {
        vec![OsString::new()]
    };
    for directory in env::split_paths(&path) {
        for extension in &extensions {
            let mut candidate = directory.join(name);
            if !extension.is_empty() {
                candidate.set_extension(extension.to_string_lossy().trim_start_matches('.'));
            }
            if candidate.is_file()
                && let Ok(canonical) = fs::canonicalize(candidate)
            {
                return Some(canonical);
            }
        }
    }
    None
}

fn hex(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(value, "{byte:02x}");
    }
    value
}

fn redirects(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    false
}

#[cfg(unix)]
fn hardlinked(_path: &Path, metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.nlink() > 1
}

#[cfg(windows)]
fn hardlinked(path: &Path, _metadata: &fs::Metadata) -> bool {
    File::open(path)
        .and_then(|file| heyfood_windows_file::file_identity(&file))
        .map_or(true, |identity| identity.number_of_links > 1)
}

#[cfg(not(any(unix, windows)))]
fn hardlinked(_path: &Path, _metadata: &fs::Metadata) -> bool {
    true
}

struct SetupLock {
    file: File,
}

impl SetupLock {
    fn acquire(path: &Path) -> Result<Self, SetupError> {
        let parent = path.parent().ok_or_else(|| {
            SetupError::new("agent_setup_lock", "setup lock has no parent directory")
        })?;
        create_directories_without_redirect(parent)?;
        validate_lock_path(path)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|error| SetupError::new("agent_setup_lock", error.to_string()))?;
        validate_open_lock(path, &file)?;
        let started = Instant::now();
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Self { file }),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if started.elapsed() >= LOCK_TIMEOUT {
                        return Err(SetupError::new(
                            "agent_setup_lock",
                            "another setup operation did not finish before the deadline",
                        ));
                    }
                    std::thread::sleep(LOCK_RETRY);
                }
                Err(error) => {
                    return Err(SetupError::new("agent_setup_lock", error.to_string()));
                }
            }
        }
    }
}

fn validate_lock_path(path: &Path) -> Result<(), SetupError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(SetupError::new("agent_setup_lock", error.to_string())),
    };
    if redirects(&metadata) || !metadata.is_file() || hardlinked(path, &metadata) {
        return Err(SetupError::new(
            "agent_setup_lock",
            "setup lock must be one regular, unlinked file",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn validate_open_lock(path: &Path, file: &File) -> Result<(), SetupError> {
    use std::os::unix::fs::MetadataExt;

    let path_metadata = fs::symlink_metadata(path)
        .map_err(|error| SetupError::new("agent_setup_lock", error.to_string()))?;
    let file_metadata = file
        .metadata()
        .map_err(|error| SetupError::new("agent_setup_lock", error.to_string()))?;
    if redirects(&path_metadata)
        || !path_metadata.is_file()
        || path_metadata.nlink() != 1
        || file_metadata.nlink() != 1
        || path_metadata.dev() != file_metadata.dev()
        || path_metadata.ino() != file_metadata.ino()
    {
        return Err(SetupError::new(
            "agent_setup_lock",
            "setup lock identity changed while it was opened",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn validate_open_lock(path: &Path, file: &File) -> Result<(), SetupError> {
    let path_metadata = fs::symlink_metadata(path)
        .map_err(|error| SetupError::new("agent_setup_lock", error.to_string()))?;
    if redirects(&path_metadata) || !path_metadata.is_file() {
        return Err(SetupError::new(
            "agent_setup_lock",
            "setup lock identity changed while it was opened",
        ));
    }
    let reopened =
        File::open(path).map_err(|error| SetupError::new("agent_setup_lock", error.to_string()))?;
    let opened_identity = heyfood_windows_file::file_identity(file)
        .map_err(|error| SetupError::new("agent_setup_lock", error.to_string()))?;
    let path_identity = heyfood_windows_file::file_identity(&reopened)
        .map_err(|error| SetupError::new("agent_setup_lock", error.to_string()))?;
    if opened_identity != path_identity || opened_identity.number_of_links != 1 {
        return Err(SetupError::new(
            "agent_setup_lock",
            "setup lock identity changed while it was opened",
        ));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn validate_open_lock(_path: &Path, _file: &File) -> Result<(), SetupError> {
    Err(SetupError::new(
        "agent_setup_lock",
        "setup lock identity cannot be verified on this platform",
    ))
}

impl Drop for SetupLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn scratch(name: &str) -> PathBuf {
        let path = env::temp_dir().join(format!(
            "heyfood-agent-setup-{name}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        fs::canonicalize(path).unwrap()
    }

    fn environment(root: &Path) -> SetupEnvironment {
        let binary = root.join("heyfood");
        fs::write(&binary, b"test-binary").unwrap();
        SetupEnvironment {
            home_dir: root.join("home"),
            state_dir: root.join("state"),
            heyfood_executable: binary,
            probes: vec![
                HostProbe {
                    host: Host::Codex,
                    executable: Some(root.join("codex")),
                    version: Some(CODEX_VERSION.to_owned()),
                },
                HostProbe {
                    host: Host::Claude,
                    executable: Some(root.join("claude")),
                    version: Some(CLAUDE_VERSION.to_owned()),
                },
            ],
        }
    }

    fn options(mode: SetupMode, operation: SetupOperation) -> SetupOptions {
        SetupOptions {
            target: SetupTarget::All,
            scope: SetupScope::User,
            project_root: None,
            operation,
            mode,
            replace: false,
        }
    }

    #[test]
    fn dry_run_is_deterministic_and_writes_nothing() {
        let root = scratch("dry-run");
        let environment = environment(&root);
        let first = execute_with_environment(
            &options(SetupMode::DryRun, SetupOperation::Install),
            &environment,
        )
        .unwrap();
        let second = execute_with_environment(
            &options(SetupMode::DryRun, SetupOperation::Install),
            &environment,
        )
        .unwrap();
        assert_eq!(first, second);
        assert!(first.ready);
        assert!(!first.changed);
        assert!(!environment.home_dir.exists());
        assert!(!environment.state_dir.exists());
    }

    #[test]
    fn apply_is_exact_idempotent_and_reversible() {
        let root = scratch("apply");
        let environment = environment(&root);
        let applied = execute_with_environment(
            &options(SetupMode::Apply, SetupOperation::Install),
            &environment,
        )
        .unwrap();
        assert!(applied.changed);
        let repeated = execute_with_environment(
            &options(SetupMode::Apply, SetupOperation::Install),
            &environment,
        )
        .unwrap();
        assert!(!repeated.changed);
        for host in &repeated.hosts {
            assert_eq!(
                inspect_skill(&host.skill_path).unwrap(),
                Some(expected_file_digests())
            );
            assert!(host.receipt_path.is_file());
        }
        let removed = execute_with_environment(
            &options(SetupMode::Apply, SetupOperation::Uninstall),
            &environment,
        )
        .unwrap();
        assert!(removed.changed);
        for host in &removed.hosts {
            assert!(!host.skill_path.exists());
            assert!(!host.receipt_path.exists());
        }
    }

    #[test]
    fn conflict_and_modified_uninstall_preserve_user_files() {
        let root = scratch("preserve");
        let environment = environment(&root);
        let codex = environment.home_dir.join(".agents/skills/heyfood");
        fs::create_dir_all(&codex).unwrap();
        fs::write(codex.join("user.md"), b"user").unwrap();
        let plan = execute_with_environment(
            &options(SetupMode::DryRun, SetupOperation::Install),
            &environment,
        )
        .unwrap();
        assert!(!plan.ready);
        assert_eq!(fs::read(codex.join("user.md")).unwrap(), b"user");

        fs::remove_dir_all(&codex).unwrap();
        execute_with_environment(
            &options(SetupMode::Apply, SetupOperation::Install),
            &environment,
        )
        .unwrap();
        fs::write(codex.join("SKILL.md"), b"modified").unwrap();
        let uninstall = execute_with_environment(
            &options(SetupMode::DryRun, SetupOperation::Uninstall),
            &environment,
        )
        .unwrap();
        assert!(!uninstall.ready);
        assert_eq!(fs::read(codex.join("SKILL.md")).unwrap(), b"modified");
    }

    #[cfg(unix)]
    #[test]
    fn link_substitution_is_rejected() {
        use std::os::unix::fs::symlink;

        let root = scratch("symlink");
        let environment = environment(&root);
        let victim = root.join("victim");
        fs::create_dir_all(&victim).unwrap();
        fs::create_dir_all(environment.home_dir.join(".agents/skills")).unwrap();
        symlink(&victim, environment.home_dir.join(".agents/skills/heyfood")).unwrap();
        let error = execute_with_environment(
            &options(SetupMode::DryRun, SetupOperation::Install),
            &environment,
        )
        .unwrap_err();
        assert_eq!(error.kind, "agent_setup_redirect");
        assert!(fs::read_dir(victim).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn setup_lock_redirect_and_hardlink_are_rejected_before_writes() {
        use std::os::unix::fs::symlink;

        for attack in ["redirect", "hardlink"] {
            let root = scratch(&format!("lock-{attack}"));
            let environment = environment(&root);
            fs::create_dir_all(&environment.state_dir).unwrap();
            let victim = root.join("victim-lock");
            fs::write(&victim, b"external").unwrap();
            let lock = environment.state_dir.join("setup.lock");
            if attack == "redirect" {
                symlink(&victim, &lock).unwrap();
            } else {
                fs::hard_link(&victim, &lock).unwrap();
            }
            let error = execute_with_environment(
                &options(SetupMode::Apply, SetupOperation::Install),
                &environment,
            )
            .unwrap_err();
            assert_eq!(error.kind, "agent_setup_lock");
            assert_eq!(fs::read(&victim).unwrap(), b"external");
            assert!(!environment.home_dir.exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn hardlinked_skill_and_receipt_files_are_rejected() {
        let root = scratch("hardlinks");
        let environment = environment(&root);
        let applied = execute_with_environment(
            &options(SetupMode::Apply, SetupOperation::Install),
            &environment,
        )
        .unwrap();
        let codex = &applied.hosts[0];
        let linked_skill = root.join("linked-skill");
        fs::hard_link(codex.skill_path.join("SKILL.md"), &linked_skill).unwrap();
        let error = execute_with_environment(
            &options(SetupMode::DryRun, SetupOperation::Install),
            &environment,
        )
        .unwrap_err();
        assert_eq!(error.kind, "agent_setup_redirect");
        fs::remove_file(linked_skill).unwrap();

        let linked_receipt = root.join("linked-receipt");
        fs::hard_link(&codex.receipt_path, &linked_receipt).unwrap();
        let error = execute_with_environment(
            &options(SetupMode::DryRun, SetupOperation::Install),
            &environment,
        )
        .unwrap_err();
        assert_eq!(error.kind, "agent_setup_receipt");
    }

    #[test]
    fn binary_drift_requires_explicit_single_host_replacement() {
        let root = scratch("binary-drift");
        let environment = environment(&root);
        execute_with_environment(
            &options(SetupMode::Apply, SetupOperation::Install),
            &environment,
        )
        .unwrap();
        fs::write(&environment.heyfood_executable, b"replacement-binary").unwrap();

        let blocked = execute_with_environment(
            &options(SetupMode::DryRun, SetupOperation::Install),
            &environment,
        )
        .unwrap();
        assert!(!blocked.ready);
        assert!(blocked.hosts.iter().all(|host| host.action == "conflict"));

        let mut replace = options(SetupMode::Apply, SetupOperation::Install);
        replace.target = SetupTarget::Codex;
        replace.replace = true;
        let replaced = execute_with_environment(&replace, &environment).unwrap();
        assert!(replaced.changed);
        assert_eq!(replaced.hosts[0].action, "replace");
    }

    #[test]
    fn project_scope_requires_an_absolute_git_worktree() {
        let root = scratch("project");
        let environment = environment(&root);
        let mut request = options(SetupMode::DryRun, SetupOperation::Install);
        request.scope = SetupScope::Project;
        assert!(execute_with_environment(&request, &environment).is_err());
        let project = root.join("repo");
        fs::create_dir_all(project.join(".git")).unwrap();
        request.project_root = Some(project.clone());
        let plan = execute_with_environment(&request, &environment).unwrap();
        assert_eq!(plan.project_root, Some(fs::canonicalize(project).unwrap()));
        assert!(plan.hosts.iter().all(|host| host.skill_path.is_absolute()));
    }

    #[test]
    fn incompatible_host_fails_closed_before_apply() {
        let root = scratch("incompatible");
        let mut environment = environment(&root);
        environment.probes[0].version = Some("codex-cli 999.0.0".to_owned());
        let plan = execute_with_environment(
            &options(SetupMode::DryRun, SetupOperation::Install),
            &environment,
        )
        .unwrap();
        assert!(!plan.ready);
        assert_eq!(plan.hosts[0].compatibility, "incompatible");
        assert!(
            execute_with_environment(
                &options(SetupMode::Apply, SetupOperation::Install),
                &environment
            )
            .is_err()
        );
        assert!(!environment.home_dir.exists());
    }

    #[test]
    fn canonical_and_host_packages_are_byte_identical() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap();
        for host in ["codex", "claude"] {
            let packaged = root.join(format!("agent-integrations/{host}/heyfood/skills/heyfood"));
            for (relative, contents) in SKILL_FILES {
                assert_eq!(
                    fs::read(packaged.join(relative)).unwrap(),
                    contents.as_bytes()
                );
            }
        }
    }

    #[test]
    fn host_plugin_manifests_are_versioned_and_skill_only() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap();
        for relative in [
            "agent-integrations/codex/heyfood/.codex-plugin/plugin.json",
            "agent-integrations/claude/heyfood/.claude-plugin/plugin.json",
        ] {
            let manifest: serde_json::Value =
                serde_json::from_slice(&fs::read(root.join(relative)).unwrap()).unwrap();
            assert_eq!(manifest["name"], "heyfood");
            assert_eq!(manifest["version"], PACKAGE_VERSION);
            assert!(manifest.get("mcpServers").is_none());
            assert!(manifest.get("hooks").is_none());
        }
    }

    #[test]
    fn concurrent_apply_serializes_and_remains_idempotent() {
        let root = scratch("concurrent");
        let environment = environment(&root);
        let first_environment = environment.clone();
        let second_environment = environment.clone();
        let first = std::thread::spawn(move || {
            execute_with_environment(
                &options(SetupMode::Apply, SetupOperation::Install),
                &first_environment,
            )
        });
        let second = std::thread::spawn(move || {
            execute_with_environment(
                &options(SetupMode::Apply, SetupOperation::Install),
                &second_environment,
            )
        });
        let first = first.join().unwrap().unwrap();
        let second = second.join().unwrap().unwrap();
        assert_ne!(first.changed, second.changed);
        for host in build_plan(
            &options(SetupMode::DryRun, SetupOperation::Install),
            &environment,
        )
        .unwrap()
        .hosts
        {
            assert_eq!(host.action, "none");
        }
    }

    #[test]
    fn generated_dry_run_validates_against_the_public_setup_schema() {
        let root = scratch("schema");
        let environment = environment(&root);
        let plan = execute_with_environment(
            &options(SetupMode::DryRun, SetupOperation::Install),
            &environment,
        )
        .unwrap();
        let instance = serde_json::to_value(plan).unwrap();
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../schemas/v1/heyfood-agent-setup-plan.schema.json"
        ))
        .unwrap();
        jsonschema::draft202012::validate(&schema, &instance).unwrap();
    }
}
