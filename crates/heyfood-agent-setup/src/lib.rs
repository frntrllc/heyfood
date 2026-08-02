//! Conflict-safe, reversible Agent Skill setup for supported local hosts.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cap_fs_ext::{DirExt as _, FollowSymlinks, OpenOptionsFollowExt as _};
use cap_std::ambient_authority;
use cap_std::fs::{Dir as CapDir, File as CapFile, OpenOptions as CapOpenOptions};
use directories::BaseDirs;
use fs2::FileExt;
use heyfood_platform::NativePaths;
#[cfg(windows)]
use heyfood_platform::OwnerOnlyPath;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const PACKAGE_VERSION: &str = "0.7.0";
// A replacement can legitimately perform up to six sequential bounded
// host-command/probe pairs while applying and restoring MCP state. Keep lock
// contention bounded, but longer than that 78-second host-owned budget plus
// local filesystem verification.
const LOCK_TIMEOUT: Duration = Duration::from_secs(120);
const LOCK_RETRY: Duration = Duration::from_millis(10);
const HOST_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const HOST_PROBE_OUTPUT_LIMIT: u64 = 4 * 1024;
const HOST_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const HOST_COMMAND_OUTPUT_LIMIT: u64 = 64 * 1024;
const CODEX_VERSION: &str = "codex-cli 0.145.0-alpha.18";
const CLAUDE_VERSION: &str = "2.1.128 (Claude Code)";

#[cfg(test)]
thread_local! {
    static TEST_FAILPOINTS: std::cell::RefCell<std::collections::BTreeSet<&'static str>> =
        const { std::cell::RefCell::new(std::collections::BTreeSet::new()) };
}

#[cfg(test)]
fn set_test_failpoints(names: &[&'static str]) {
    TEST_FAILPOINTS.with(|failpoints| {
        failpoints.borrow_mut().extend(names.iter().copied());
    });
}

#[cfg(test)]
fn hit_test_failpoint(name: &'static str) -> bool {
    TEST_FAILPOINTS.with(|failpoints| failpoints.borrow_mut().remove(name))
}

#[cfg(not(test))]
const fn hit_test_failpoint(_name: &'static str) -> bool {
    false
}

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
    pub expected_plan_sha256: Option<String>,
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
    pub host_commands: HostCommandMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostCommandMode {
    Execute,
    Simulate,
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
    pub mcp: McpRegistrationPlan,
    pub action: &'static str,
    pub conflicts: Vec<String>,
    pub user_actions: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct McpRegistrationPlan {
    pub name: &'static str,
    pub transport: &'static str,
    pub command: PathBuf,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub environment_policy_sha256: String,
    pub configuration_scope: &'static str,
    pub action: &'static str,
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
    pub plan_sha256: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mcp: Option<McpRegistrationReceipt>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct McpRegistrationReceipt {
    name: String,
    transport: String,
    command: PathBuf,
    arguments: Vec<String>,
    environment: BTreeMap<String, String>,
    environment_policy_sha256: String,
    configuration_scope: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum McpProbe {
    Missing,
    Present(McpRegistrationReceipt),
    Unavailable,
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
        host_commands: HostCommandMode::Execute,
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
    let expected = options.expected_plan_sha256.as_deref().ok_or_else(|| {
        SetupError::new(
            "agent_setup_plan_required",
            "--apply requires the exact SHA-256 from the reviewed dry-run plan",
        )
        .hint("Run the same command with --dry-run, review it, then pass --plan-sha256.")
    })?;
    if expected != plan.plan_sha256 {
        return Err(SetupError::new(
            "agent_setup_plan_changed",
            "the current setup plan does not match the reviewed plan digest",
        )
        .hint("Run a new dry-run and review the complete current plan."));
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
    if expected != plan.plan_sha256 {
        return Err(SetupError::new(
            "agent_setup_plan_changed",
            "agent integration state changed after the reviewed plan",
        )
        .hint("Run a new dry-run and review the complete current plan."));
    }
    if !plan.ready {
        return Err(SetupError::new(
            "agent_setup_changed",
            "agent integration setup state changed after planning",
        )
        .hint("Run the dry-run again and review the current plan."));
    }

    let actionable: Vec<_> = plan
        .hosts
        .iter()
        .filter(|host| host.action != "none")
        .cloned()
        .collect();
    if options.operation == SetupOperation::Uninstall {
        uninstall_hosts_transactionally(
            &actionable,
            environment,
            options.scope,
            plan.project_root.as_deref(),
        )?;
        plan.changed = !actionable.is_empty();
        return Ok(plan);
    }

    let package = package_identity();
    let binary = binary_identity(&environment.heyfood_executable)?;
    let probes = probe_map(environment);
    let mut completed = Vec::new();
    for host_plan in &actionable {
        let probe = probes
            .get(&host_plan.host)
            .expect("every selected host has a probe");
        if let Err(error) = install_host(options, host_plan, probe, &package, &binary, environment)
        {
            if error.uncertain {
                return Err(error);
            }
            for completed_plan in completed.iter().rev() {
                if uninstall_hosts_transactionally(
                    std::slice::from_ref(completed_plan),
                    environment,
                    options.scope,
                    plan.project_root.as_deref(),
                )
                .is_err()
                {
                    return Err(SetupError::uncertain(
                        "agent_setup_rollback",
                        "agent integration apply failed and rollback could not be verified",
                    ));
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
    let mut plan = SetupPlan {
        schema_version: 1,
        operation: options.operation,
        mode: options.mode,
        target: options.target,
        scope: options.scope,
        project_root,
        binary,
        package,
        plan_sha256: String::new(),
        ready,
        changed: false,
        hosts,
    };
    plan.plan_sha256 = plan_digest(&plan)?;
    Ok(plan)
}

fn plan_digest(plan: &SetupPlan) -> Result<String, SetupError> {
    let mut value = serde_json::to_value(plan)
        .map_err(|error| SetupError::new("agent_setup_plan", error.to_string()))?;
    let object = value
        .as_object_mut()
        .expect("setup plan serialization is an object");
    object.remove("plan_sha256");
    object.insert(
        "mode".to_owned(),
        serde_json::Value::String("dry_run".to_owned()),
    );
    object.insert("changed".to_owned(), serde_json::Value::Bool(false));
    let canonical = serde_json::to_vec(&value)
        .map_err(|error| SetupError::new("agent_setup_plan", error.to_string()))?;
    Ok(hex(&Sha256::digest(canonical)))
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
    let expected_mcp = expected_mcp_registration(probe.host, options.scope, binary);
    let mcp_probe = probe_mcp_registration(
        environment,
        probe,
        options.scope,
        project_root,
        receipt.as_ref(),
    );
    if options.scope == SetupScope::Project && probe.host == Host::Codex {
        conflicts.push(
            "Codex exposes no host-owned project-scope MCP registration command; use user scope"
                .to_owned(),
        );
    }
    let action = match options.operation {
        SetupOperation::Install => match (&current, &receipt) {
            (None, None) if mcp_probe == McpProbe::Missing => "install",
            (Some(files), Some(receipt))
                if receipt_matches_current(
                    receipt,
                    &ReceiptExpectation {
                        options,
                        project_root,
                        skill_path: &skill_path,
                        package,
                        binary,
                        probe,
                        mcp: &expected_mcp,
                    },
                ) && *files == receipt.files
                    && mcp_probe == McpProbe::Present(expected_mcp.clone()) =>
            {
                "none"
            }
            (Some(files), Some(receipt))
                if options.replace
                    && receipt.skill_path == skill_path
                    && *files == receipt.files
                    && receipt_mcp_matches_probe(receipt, &mcp_probe) =>
            {
                "replace"
            }
            _ => {
                conflicts.push(
                    "skill or MCP registration is not the exact receipt-bound heyfood installation"
                        .to_owned(),
                );
                "conflict"
            }
        },
        SetupOperation::Uninstall => match (&current, &receipt) {
            (None, None) if mcp_probe == McpProbe::Missing => "none",
            (Some(files), Some(receipt))
                if receipt.skill_path == skill_path
                    && *files == receipt.files
                    && receipt_mcp_matches_probe(receipt, &mcp_probe) =>
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
            _ => {
                conflicts.push(
                    "MCP registration changed outside the receipt-bound setup transaction"
                        .to_owned(),
                );
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
    if receipt
        .as_ref()
        .is_some_and(|receipt| receipt.schema_version == 1)
        && options.operation == SetupOperation::Install
        && !options.replace
    {
        user_actions.push(
            "Re-run the dry-run with --replace to migrate the receipt-bound v0.5 skill and add MCP."
                .to_owned(),
        );
    }
    let mcp_action = if action == "conflict" {
        "conflict"
    } else {
        action
    };
    Ok(HostSetupPlan {
        host: probe.host,
        host_executable: probe.executable.clone(),
        host_version: probe.version.clone(),
        compatible_version: probe.host.compatible_version(),
        compatibility,
        skill_path,
        receipt_path,
        mcp: McpRegistrationPlan {
            name: "heyfood",
            transport: "stdio",
            command: expected_mcp.command.clone(),
            arguments: expected_mcp.arguments.clone(),
            environment: expected_mcp.environment.clone(),
            environment_policy_sha256: expected_mcp.environment_policy_sha256.clone(),
            configuration_scope: expected_mcp_scope(probe.host, options.scope),
            action: mcp_action,
        },
        action,
        conflicts,
        user_actions,
    })
}

struct AnchoredDirectory {
    directory: CapDir,
    absolute_path: PathBuf,
}

impl AnchoredDirectory {
    fn open(path: &Path) -> Result<Self, SetupError> {
        Self::open_internal(path, false)
    }

    fn open_or_create(path: &Path) -> Result<Self, SetupError> {
        Self::open_internal(path, true)
    }

    fn open_internal(path: &Path, create: bool) -> Result<Self, SetupError> {
        if !path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
        {
            return Err(SetupError::new(
                "agent_setup_path",
                "anchored setup paths must be absolute and normalized",
            ));
        }

        let mut root = PathBuf::new();
        let mut components = Vec::new();
        for component in path.components() {
            match component {
                Component::Prefix(_) | Component::RootDir => {
                    if components.is_empty() {
                        root.push(component.as_os_str());
                    } else {
                        return Err(SetupError::new(
                            "agent_setup_path",
                            "setup path contains an unexpected root component",
                        ));
                    }
                }
                Component::Normal(value) => components.push(value.to_owned()),
                Component::ParentDir | Component::CurDir => unreachable!("validated above"),
            }
        }
        if root.as_os_str().is_empty() {
            return Err(SetupError::new(
                "agent_setup_path",
                "setup path has no filesystem root",
            ));
        }

        let mut absolute_path = root.clone();
        let mut directory = CapDir::open_ambient_dir(&root, ambient_authority())
            .map_err(|error| SetupError::new("agent_setup_path", error.to_string()))?;
        for component in components {
            absolute_path.push(&component);
            match directory.open_dir_nofollow(&component) {
                Ok(next) => directory = next,
                Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
                    let created = match create_private_child_directory(&directory, &component) {
                        Ok(()) => true,
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
                        Err(error) => {
                            return Err(SetupError::new("agent_setup_path", error.to_string()));
                        }
                    };
                    directory = directory
                        .open_dir_nofollow(&component)
                        .map_err(|error| SetupError::new("agent_setup_path", error.to_string()))?;
                    if created {
                        harden_open_directory(&directory, &absolute_path)?;
                    }
                }
                Err(error) => {
                    return Err(SetupError::new("agent_setup_path", error.to_string()));
                }
            }
        }
        Ok(Self {
            directory,
            absolute_path,
        })
    }

    fn child_name<'a>(&self, path: &'a Path) -> Result<&'a std::ffi::OsStr, SetupError> {
        if path.parent() != Some(self.absolute_path.as_path()) {
            return Err(SetupError::new(
                "agent_setup_path",
                "setup path escaped its anchored parent",
            ));
        }
        path.file_name()
            .ok_or_else(|| SetupError::new("agent_setup_path", "setup path has no final component"))
    }
}

fn create_private_child_directory(parent: &CapDir, name: &std::ffi::OsStr) -> std::io::Result<()> {
    #[cfg(unix)]
    let mut builder = cap_std::fs::DirBuilder::new();
    #[cfg(not(unix))]
    let builder = cap_std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use cap_std::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    parent.create_dir_with(name, &builder)
}

fn harden_open_directory(directory: &CapDir, absolute_path: &Path) -> Result<(), SetupError> {
    #[cfg(not(windows))]
    let _ = absolute_path;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        directory
            .set_permissions(
                Path::new("."),
                cap_std::fs::Permissions::from_std(fs::Permissions::from_mode(0o700)),
            )
            .map_err(|error| SetupError::new("agent_setup_permissions", error.to_string()))?;
    }
    #[cfg(windows)]
    {
        OwnerOnlyPath::directory(absolute_path)
            .map_err(|error| SetupError::new("agent_setup_permissions", error.message))?;
        validate_windows_open_directory_identity(directory, absolute_path)?;
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = directory;
        let _ = absolute_path;
        return Err(SetupError::new(
            "agent_setup_permissions",
            "owner-only directory permissions are unsupported on this platform",
        ));
    }
    Ok(())
}

fn harden_open_file(file: &CapFile, absolute_path: &Path) -> Result<(), SetupError> {
    #[cfg(not(windows))]
    let _ = absolute_path;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.try_clone()
            .and_then(|value| {
                value
                    .into_std()
                    .set_permissions(fs::Permissions::from_mode(0o600))
            })
            .map_err(|error| SetupError::new("agent_setup_permissions", error.to_string()))?;
    }
    #[cfg(windows)]
    {
        OwnerOnlyPath::file(absolute_path)
            .map_err(|error| SetupError::new("agent_setup_permissions", error.message))?;
        validate_windows_open_file_identity(file, absolute_path)?;
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        let _ = absolute_path;
        return Err(SetupError::new(
            "agent_setup_permissions",
            "owner-only file permissions are unsupported on this platform",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn validate_windows_open_directory_identity(
    directory: &CapDir,
    path: &Path,
) -> Result<(), SetupError> {
    let path_metadata = fs::symlink_metadata(path)
        .map_err(|error| SetupError::new("agent_setup_permissions", error.to_string()))?;
    if redirects(&path_metadata) || !path_metadata.is_dir() {
        return Err(SetupError::new(
            "agent_setup_redirect",
            "setup directory identity changed during permission hardening",
        ));
    }
    let opened = directory
        .try_clone()
        .map(CapDir::into_std_file)
        .map_err(|error| SetupError::new("agent_setup_permissions", error.to_string()))?;
    let opened_metadata = directory
        .dir_metadata()
        .map_err(|error| SetupError::new("agent_setup_permissions", error.to_string()))?;
    if !opened_metadata.is_dir() {
        return Err(SetupError::new(
            "agent_setup_redirect",
            "setup directory identity changed during permission hardening",
        ));
    }
    let opened_identity = heyfood_windows_file::file_identity(&opened)
        .map_err(|error| SetupError::new("agent_setup_permissions", error.to_string()))?;
    let path_identity = heyfood_windows_file::open_directory_identity(path)
        .map_err(|error| SetupError::new("agent_setup_permissions", error.to_string()))?;
    if opened_identity != path_identity || opened_identity.number_of_links == 0 {
        return Err(SetupError::new(
            "agent_setup_redirect",
            "setup directory identity changed during permission hardening",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn validate_windows_open_file_identity(file: &CapFile, path: &Path) -> Result<(), SetupError> {
    let path_metadata = fs::symlink_metadata(path)
        .map_err(|error| SetupError::new("agent_setup_permissions", error.to_string()))?;
    if redirects(&path_metadata) || !path_metadata.is_file() {
        return Err(SetupError::new(
            "agent_setup_redirect",
            "setup file identity changed during permission hardening",
        ));
    }
    let opened = file
        .try_clone()
        .map(CapFile::into_std)
        .map_err(|error| SetupError::new("agent_setup_permissions", error.to_string()))?;
    let reopened = File::open(path)
        .map_err(|error| SetupError::new("agent_setup_permissions", error.to_string()))?;
    let opened_identity = heyfood_windows_file::file_identity(&opened)
        .map_err(|error| SetupError::new("agent_setup_permissions", error.to_string()))?;
    let path_identity = heyfood_windows_file::file_identity(&reopened)
        .map_err(|error| SetupError::new("agent_setup_permissions", error.to_string()))?;
    if opened_identity != path_identity {
        return Err(SetupError::new(
            "agent_setup_redirect",
            "setup file identity changed during permission hardening",
        ));
    }
    Ok(())
}

fn install_host(
    options: &SetupOptions,
    plan: &HostSetupPlan,
    probe: &HostProbe,
    package: &SkillPackageIdentity,
    binary: &BinaryIdentity,
    environment: &SetupEnvironment,
) -> Result<(), SetupError> {
    let prior_receipt = load_receipt(&plan.receipt_path)?;
    validate_destination(&plan.skill_path)?;
    let parent = plan.skill_path.parent().ok_or_else(|| {
        SetupError::new(
            "agent_setup_path",
            "skill destination has no parent directory",
        )
    })?;
    let anchored_parent = AnchoredDirectory::open_or_create(parent)?;
    let skill_name = anchored_parent.child_name(&plan.skill_path)?;
    let stage_name = OsString::from(format!(".heyfood.{}.stage", std::process::id()));
    let stage = parent.join(&stage_name);
    if anchored_parent
        .directory
        .symlink_metadata(&stage_name)
        .is_ok()
    {
        return Err(SetupError::new(
            "agent_setup_stage",
            "setup staging path already exists",
        ));
    }
    create_private_child_directory(&anchored_parent.directory, &stage_name)
        .map_err(|error| SetupError::new("agent_setup_stage", error.to_string()))?;
    let stage_directory = anchored_parent
        .directory
        .open_dir_nofollow(&stage_name)
        .map_err(|error| SetupError::new("agent_setup_stage", error.to_string()))?;
    harden_open_directory(&stage_directory, &stage)?;
    #[cfg(windows)]
    let staged_identity = stage_directory
        .try_clone()
        .map(CapDir::into_std_file)
        .and_then(|directory| heyfood_windows_file::file_identity(&directory))
        .map_err(|error| SetupError::new("agent_setup_stage", error.to_string()))?;
    let staged = write_skill_files(&stage_directory, &stage).and_then(|()| {
        let mut files = BTreeMap::new();
        inspect_directory(&stage_directory, Path::new(""), &mut files)?;
        if files != expected_file_digests() {
            return Err(SetupError::new(
                "agent_setup_stage",
                "staged Agent Skill bytes do not match the embedded package",
            ));
        }
        Ok(())
    });
    // Windows capability directory handles intentionally deny delete sharing.
    // Validate through that open capability first, then close it so Windows
    // can acquire a delete-capable handle for the identity-pinned commit.
    drop(stage_directory);
    if let Err(error) = staged {
        let _ = anchored_parent.directory.remove_dir_all(&stage_name);
        return Err(error);
    }
    #[cfg(windows)]
    let stage_commit_handle =
        heyfood_windows_file::DirectoryRenameHandle::open(&stage).map_err(|error| {
            let _ = anchored_parent.directory.remove_dir_all(&stage_name);
            SetupError::new("agent_setup_commit", error.to_string())
        })?;
    #[cfg(windows)]
    if stage_commit_handle.identity() != staged_identity || staged_identity.number_of_links == 0 {
        return Err(SetupError::new(
            "agent_setup_redirect",
            "setup staging directory identity changed before commit",
        ));
    }
    let backup_name = OsString::from(format!(".heyfood.{}.backup", std::process::id()));
    let replacing = plan.action == "replace";
    if replacing {
        if anchored_parent
            .directory
            .symlink_metadata(&backup_name)
            .is_ok()
        {
            let _ = anchored_parent.directory.remove_dir_all(&stage_name);
            return Err(SetupError::new(
                "agent_setup_backup",
                "setup backup path already exists",
            ));
        }
        anchored_parent
            .directory
            .rename(skill_name, &anchored_parent.directory, &backup_name)
            .map_err(|error| {
                let _ = anchored_parent.directory.remove_dir_all(&stage_name);
                SetupError::new("agent_setup_replace", error.to_string())
            })?;
    }
    #[cfg(windows)]
    let published_stage = {
        if hit_test_failpoint("skill_commit_publish") {
            drop(stage_commit_handle);
            return Err(rollback_staged_commit_or_uncertain(
                &anchored_parent.directory,
                skill_name,
                &stage_name,
                &backup_name,
                replacing,
                SetupError::new("agent_setup_commit", "injected skill publish failure"),
            ));
        }
        match stage_commit_handle.publish(&plan.skill_path, false) {
            Ok(published) => published,
            Err(error) => {
                return Err(rollback_staged_commit_or_uncertain(
                    &anchored_parent.directory,
                    skill_name,
                    &stage_name,
                    &backup_name,
                    replacing,
                    SetupError::new("agent_setup_commit", error.to_string()),
                ));
            }
        }
    };
    #[cfg(not(windows))]
    {
        let published = if hit_test_failpoint("skill_commit_publish") {
            Err(std::io::Error::other("injected skill publish failure"))
        } else {
            anchored_parent
                .directory
                .rename(&stage_name, &anchored_parent.directory, skill_name)
        };
        if let Err(error) = published {
            return Err(rollback_staged_commit_or_uncertain(
                &anchored_parent.directory,
                skill_name,
                &stage_name,
                &backup_name,
                replacing,
                SetupError::new("agent_setup_commit", error.to_string()),
            ));
        }
    }
    #[cfg(windows)]
    let published_identity = published_stage.identity();
    #[cfg(windows)]
    drop(published_stage);
    let committed_directory = match anchored_parent.directory.open_dir_nofollow(skill_name) {
        Ok(directory) => directory,
        Err(error) => {
            return Err(rollback_installed_skill_or_uncertain(
                &anchored_parent.directory,
                skill_name,
                &backup_name,
                replacing,
                SetupError::new("agent_setup_commit", error.to_string()),
            ));
        }
    };
    #[cfg(windows)]
    let committed_identity = committed_directory
        .try_clone()
        .map(CapDir::into_std_file)
        .and_then(|directory| heyfood_windows_file::file_identity(&directory));
    let mut committed_files = BTreeMap::new();
    let committed_inspection = if hit_test_failpoint("skill_post_publish_validation") {
        Err(SetupError::new(
            "agent_setup_stage",
            "injected post-publish validation failure",
        ))
    } else {
        inspect_directory(&committed_directory, Path::new(""), &mut committed_files)
    };
    drop(committed_directory);
    if let Err(error) = committed_inspection {
        return Err(rollback_installed_skill_or_uncertain(
            &anchored_parent.directory,
            skill_name,
            &backup_name,
            replacing,
            error,
        ));
    }
    #[cfg(windows)]
    let committed_identity_matches = match (committed_identity, published_identity) {
        (Ok(committed), Ok(published)) => {
            committed == staged_identity && published == staged_identity
        }
        (Err(error), _) | (_, Err(error)) => {
            return Err(rollback_installed_skill_or_uncertain(
                &anchored_parent.directory,
                skill_name,
                &backup_name,
                replacing,
                SetupError::new("agent_setup_commit", error.to_string()),
            ));
        }
    };
    #[cfg(windows)]
    if !committed_identity_matches {
        return Err(rollback_installed_skill_or_uncertain(
            &anchored_parent.directory,
            skill_name,
            &backup_name,
            replacing,
            SetupError::new(
                "agent_setup_redirect",
                "committed Agent Skill identity changed during final inspection",
            ),
        ));
    }
    if committed_files != expected_file_digests() {
        return Err(rollback_installed_skill_or_uncertain(
            &anchored_parent.directory,
            skill_name,
            &backup_name,
            replacing,
            SetupError::new(
                "agent_setup_stage",
                "committed Agent Skill bytes do not match the embedded package",
            ),
        ));
    }

    let project_root = normalize_project_root(options)?;
    let expected_mcp = expected_mcp_registration(plan.host, options.scope, binary);
    if replacing {
        let prior = prior_receipt.as_ref().ok_or_else(|| {
            SetupError::uncertain(
                "agent_setup_replace",
                "replacement lost its previously reviewed setup receipt",
            )
        })?;
        if let Some(prior_mcp) = prior.mcp.as_ref()
            && let Err(error) = unregister_mcp(
                environment,
                plan,
                options.scope,
                project_root.as_deref(),
                prior_mcp,
            )
        {
            if rollback_installed_skill(
                &anchored_parent.directory,
                skill_name,
                &backup_name,
                replacing,
            )
            .is_err()
            {
                return Err(SetupError::uncertain(
                    "agent_setup_rollback",
                    "MCP update failed and the prior skill could not be restored",
                ));
            }
            return Err(error);
        }
    }
    if let Err(error) = register_mcp(
        environment,
        plan,
        options.scope,
        project_root.as_deref(),
        &expected_mcp,
    ) {
        if error.uncertain {
            return Err(error);
        }
        if replacing {
            let prior = prior_receipt.as_ref().expect("replacement receipt exists");
            if prior.mcp.as_ref().is_some_and(|prior_mcp| {
                register_mcp(
                    environment,
                    plan,
                    options.scope,
                    project_root.as_deref(),
                    prior_mcp,
                )
                .is_err()
            }) {
                return Err(SetupError::uncertain(
                    "agent_setup_mcp_rollback",
                    "new MCP registration failed and the prior registration could not be restored",
                ));
            }
        }
        if rollback_installed_skill(
            &anchored_parent.directory,
            skill_name,
            &backup_name,
            replacing,
        )
        .is_err()
        {
            return Err(SetupError::uncertain(
                "agent_setup_rollback",
                "MCP registration failed and the prior skill could not be restored",
            ));
        }
        return Err(error);
    }

    let receipt = Receipt {
        schema_version: 2,
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
        project_root: project_root.clone(),
        heyfood_executable: binary.path.clone(),
        heyfood_sha256: binary.sha256.clone(),
        package_version: package.version.to_owned(),
        package_sha256: package.sha256.clone(),
        skill_path: plan.skill_path.clone(),
        files: expected_file_digests(),
        mcp: Some(expected_mcp.clone()),
    };
    let mut bytes = serde_json::to_vec(&receipt)
        .map_err(|error| SetupError::new("agent_setup_receipt", error.to_string()))?;
    bytes.push(b'\n');
    if let Err(error) = replace_private_file_anchored(&plan.receipt_path, &bytes) {
        if error.uncertain {
            return Err(SetupError::uncertain(
                "agent_setup_receipt_outcome_uncertain",
                "the setup receipt may have committed; installed files were preserved for explicit reconciliation",
            ));
        }
        let mcp_rollback = unregister_mcp(
            environment,
            plan,
            options.scope,
            project_root.as_deref(),
            &expected_mcp,
        )
        .and_then(|()| {
            if let Some(prior_mcp) = prior_receipt
                .as_ref()
                .filter(|_| replacing)
                .and_then(|receipt| receipt.mcp.as_ref())
            {
                register_mcp(
                    environment,
                    plan,
                    options.scope,
                    project_root.as_deref(),
                    prior_mcp,
                )
            } else {
                Ok(())
            }
        });
        if mcp_rollback.is_err()
            || hit_test_failpoint("receipt_rollback_remove")
            || rollback_installed_skill(
                &anchored_parent.directory,
                skill_name,
                &backup_name,
                replacing,
            )
            .is_err()
        {
            return Err(SetupError::uncertain(
                "agent_setup_receipt_rollback",
                "receipt commit failed and the prior skill/MCP state could not be restored",
            ));
        }
        return Err(error);
    }
    if replacing
        && anchored_parent
            .directory
            .remove_dir_all(&backup_name)
            .is_err()
    {
        return Err(SetupError::uncertain(
            "agent_setup_cleanup",
            "the new skill and receipt are committed but the prior backup could not be removed",
        ));
    }
    Ok(())
}

fn rollback_installed_skill(
    parent: &CapDir,
    skill_name: &std::ffi::OsStr,
    backup_name: &std::ffi::OsStr,
    replacing: bool,
) -> Result<(), SetupError> {
    if hit_test_failpoint("skill_installed_rollback_remove") {
        return Err(SetupError::new(
            "agent_setup_rollback",
            "injected installed skill rollback failure",
        ));
    }
    parent
        .remove_dir_all(skill_name)
        .map_err(|error| SetupError::new("agent_setup_rollback", error.to_string()))?;
    if replacing {
        parent
            .rename(backup_name, parent, skill_name)
            .map_err(|error| SetupError::new("agent_setup_rollback", error.to_string()))?;
    }
    Ok(())
}

fn rollback_installed_skill_or_uncertain(
    parent: &CapDir,
    skill_name: &std::ffi::OsStr,
    backup_name: &std::ffi::OsStr,
    replacing: bool,
    original: SetupError,
) -> SetupError {
    if rollback_installed_skill(parent, skill_name, backup_name, replacing).is_err() {
        SetupError::uncertain(
            "agent_setup_validation_rollback",
            "installed Agent Skill validation failed and the prior state could not be restored",
        )
    } else {
        original
    }
}

fn rollback_staged_commit(
    parent: &CapDir,
    skill_name: &std::ffi::OsStr,
    stage_name: &std::ffi::OsStr,
    backup_name: &std::ffi::OsStr,
    replacing: bool,
) -> Result<(), SetupError> {
    if parent.symlink_metadata(stage_name).is_ok() {
        parent
            .remove_dir_all(stage_name)
            .map_err(|error| SetupError::new("agent_setup_rollback", error.to_string()))?;
    }
    if replacing {
        if hit_test_failpoint("skill_stage_rollback_restore") {
            return Err(SetupError::new(
                "agent_setup_rollback",
                "injected staged skill rollback failure",
            ));
        }
        parent
            .rename(backup_name, parent, skill_name)
            .map_err(|error| SetupError::new("agent_setup_rollback", error.to_string()))?;
    }
    Ok(())
}

fn rollback_staged_commit_or_uncertain(
    parent: &CapDir,
    skill_name: &std::ffi::OsStr,
    stage_name: &std::ffi::OsStr,
    backup_name: &std::ffi::OsStr,
    replacing: bool,
    original: SetupError,
) -> SetupError {
    if rollback_staged_commit(parent, skill_name, stage_name, backup_name, replacing).is_err() {
        SetupError::uncertain(
            "agent_setup_commit_rollback",
            "Agent Skill commit failed and the prior state could not be restored",
        )
    } else {
        original
    }
}

fn sync_anchored_directory(directory: &CapDir) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        // cap-std intentionally represents directory capabilities with O_PATH
        // on Linux when available. Reopen "." relative to that capability to
        // obtain a read-capable handle that supports fsync without returning
        // to an ambient path.
        directory.open(Path::new("."))?.into_std().sync_all()
    }
    #[cfg(windows)]
    {
        // Windows exposes no portable directory-fsync operation, and cap-std
        // deliberately holds directories without write access. Every receipt
        // byte is flushed before the same-volume atomic rename; the receipt
        // reconciliation protocol treats any crash at that boundary as
        // uncertain. This matches the product persistence durability policy.
        let _ = directory;
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = directory;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "durable directory synchronization is unsupported",
        ))
    }
}

fn replace_private_file_anchored(path: &Path, bytes: &[u8]) -> Result<(), SetupError> {
    let parent = path.parent().ok_or_else(|| {
        SetupError::new(
            "agent_setup_receipt",
            "receipt destination has no parent directory",
        )
    })?;
    let anchored_parent = AnchoredDirectory::open_or_create(parent)?;
    let target = anchored_parent.child_name(path)?;
    let stage = OsString::from(format!(
        ".heyfood.{}.{}.receipt-stage",
        std::process::id(),
        target.to_string_lossy()
    ));
    if anchored_parent.directory.symlink_metadata(&stage).is_ok() {
        return Err(SetupError::new(
            "agent_setup_receipt",
            "receipt staging path already exists",
        ));
    }
    let mut options = CapOpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = anchored_parent
        .directory
        .open_with(&stage, &options)
        .map_err(|error| SetupError::new("agent_setup_receipt", error.to_string()))?;
    let stage_path = parent.join(&stage);
    if let Err(error) = harden_open_file(&file, &stage_path).and_then(|()| {
        file.write_all(bytes)
            .and_then(|()| file.flush())
            .and_then(|()| file.sync_all())
            .map_err(|error| SetupError::new("agent_setup_receipt", error.to_string()))
    }) {
        let _ = anchored_parent.directory.remove_file(&stage);
        return Err(error);
    }
    drop(file);
    if hit_test_failpoint("receipt_before_rename") {
        let _ = anchored_parent.directory.remove_file(&stage);
        return Err(SetupError::new(
            "agent_setup_receipt",
            "injected receipt failure before commit",
        ));
    }
    anchored_parent
        .directory
        .rename(&stage, &anchored_parent.directory, target)
        .map_err(|error| {
            let _ = anchored_parent.directory.remove_file(&stage);
            SetupError::new("agent_setup_receipt", error.to_string())
        })?;
    if hit_test_failpoint("receipt_after_rename") {
        return Err(SetupError::uncertain(
            "agent_setup_receipt_outcome_uncertain",
            "injected interruption after receipt rename",
        ));
    }
    sync_anchored_directory(&anchored_parent.directory).map_err(|_| {
        SetupError::uncertain(
            "agent_setup_receipt_outcome_uncertain",
            "the receipt was renamed but durable directory synchronization failed",
        )
    })
}

struct ReceiptExpectation<'a> {
    options: &'a SetupOptions,
    project_root: Option<&'a Path>,
    skill_path: &'a Path,
    package: &'a SkillPackageIdentity,
    binary: &'a BinaryIdentity,
    probe: &'a HostProbe,
    mcp: &'a McpRegistrationReceipt,
}

fn receipt_matches_current(receipt: &Receipt, expected: &ReceiptExpectation<'_>) -> bool {
    receipt.schema_version == 2
        && receipt.host == expected.probe.host
        && receipt.host_executable.as_path()
            == expected
                .probe
                .executable
                .as_deref()
                .unwrap_or(Path::new(""))
        && receipt.host_version.as_str() == expected.probe.version.as_deref().unwrap_or("")
        && receipt.scope == expected.options.scope
        && receipt.project_root.as_deref() == expected.project_root
        && receipt.heyfood_executable == expected.binary.path
        && receipt.heyfood_sha256 == expected.binary.sha256
        && receipt.package_version == expected.package.version
        && receipt.package_sha256 == expected.package.sha256
        && receipt.skill_path == expected.skill_path
        && receipt.mcp.as_ref() == Some(expected.mcp)
}

fn receipt_mcp_matches_probe(receipt: &Receipt, probe: &McpProbe) -> bool {
    match (&receipt.mcp, probe) {
        (Some(expected), McpProbe::Present(actual)) => expected == actual,
        (None, McpProbe::Missing) => receipt.schema_version == 1,
        _ => false,
    }
}

fn expected_mcp_scope(host: Host, scope: SetupScope) -> &'static str {
    match (host, scope) {
        (Host::Codex, SetupScope::User) => "user",
        (Host::Codex, SetupScope::Project) => "unsupported",
        (Host::Claude, SetupScope::User) => "user",
        (Host::Claude, SetupScope::Project) => "project",
    }
}

fn expected_mcp_registration(
    host: Host,
    scope: SetupScope,
    binary: &BinaryIdentity,
) -> McpRegistrationReceipt {
    McpRegistrationReceipt {
        name: "heyfood".to_owned(),
        transport: "stdio".to_owned(),
        command: binary.path.clone(),
        arguments: vec!["mcp".to_owned(), "serve".to_owned()],
        environment: BTreeMap::new(),
        environment_policy_sha256: hex(&Sha256::digest(
            include_bytes!(
                "../../../docs/release-evidence/agent-native-phase0/mcp-environment-policy.json"
            )
            .as_slice(),
        )),
        configuration_scope: expected_mcp_scope(host, scope).to_owned(),
    }
}

fn probe_mcp_registration(
    environment: &SetupEnvironment,
    host: &HostProbe,
    scope: SetupScope,
    project_root: Option<&Path>,
    receipt: Option<&Receipt>,
) -> McpProbe {
    if environment.host_commands == HostCommandMode::Simulate {
        return receipt
            .and_then(|receipt| receipt.mcp.clone())
            .map_or(McpProbe::Missing, McpProbe::Present);
    }
    if host.host == Host::Codex && scope == SetupScope::Project {
        return McpProbe::Unavailable;
    }
    let Some(executable) = host.executable.as_deref() else {
        return McpProbe::Unavailable;
    };
    let arguments = match host.host {
        Host::Codex => vec![
            OsString::from("mcp"),
            OsString::from("get"),
            OsString::from("heyfood"),
            OsString::from("--json"),
        ],
        Host::Claude => vec![
            OsString::from("mcp"),
            OsString::from("get"),
            OsString::from("heyfood"),
        ],
    };
    let output = match bounded_host_command(
        executable,
        &arguments,
        project_root,
        HOST_PROBE_TIMEOUT,
        HOST_COMMAND_OUTPUT_LIMIT,
    ) {
        Ok(output) => output,
        Err(()) => return McpProbe::Unavailable,
    };
    let combined = [output.stdout.as_slice(), output.stderr.as_slice()].concat();
    let combined = String::from_utf8_lossy(&combined);
    if !output.success {
        return if combined.contains("No MCP server") || combined.contains("No MCP server named") {
            McpProbe::Missing
        } else {
            McpProbe::Unavailable
        };
    }
    match host.host {
        Host::Codex => parse_codex_mcp_probe(&output.stdout),
        Host::Claude => parse_claude_mcp_probe(&output.stdout, scope),
    }
}

fn parse_codex_mcp_probe(bytes: &[u8]) -> McpProbe {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return McpProbe::Unavailable;
    };
    let Some(transport) = value
        .get("transport")
        .and_then(serde_json::Value::as_object)
    else {
        return McpProbe::Unavailable;
    };
    if transport.get("type").and_then(serde_json::Value::as_str) != Some("stdio") {
        return McpProbe::Unavailable;
    }
    let Some(command) = transport
        .get("command")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
    else {
        return McpProbe::Unavailable;
    };
    let Some(arguments) = transport
        .get("args")
        .and_then(serde_json::Value::as_array)
        .and_then(|values| {
            values
                .iter()
                .map(|value| value.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
        })
    else {
        return McpProbe::Unavailable;
    };
    let environment_empty = transport.get("env").is_none_or(serde_json::Value::is_null)
        && transport
            .get("env_vars")
            .and_then(serde_json::Value::as_array)
            .is_none_or(Vec::is_empty);
    if !environment_empty {
        return McpProbe::Unavailable;
    }
    McpProbe::Present(McpRegistrationReceipt {
        name: "heyfood".to_owned(),
        transport: "stdio".to_owned(),
        command,
        arguments,
        environment: BTreeMap::new(),
        environment_policy_sha256: hex(&Sha256::digest(
            include_bytes!(
                "../../../docs/release-evidence/agent-native-phase0/mcp-environment-policy.json"
            )
            .as_slice(),
        )),
        configuration_scope: "user".to_owned(),
    })
}

fn parse_claude_mcp_probe(bytes: &[u8], scope: SetupScope) -> McpProbe {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return McpProbe::Unavailable;
    };
    let field = |name: &str| {
        text.lines().find_map(|line| {
            let line = line.trim();
            line.strip_prefix(name).map(str::trim).map(str::to_owned)
        })
    };
    let Some(command) = field("Command:").map(PathBuf::from) else {
        return McpProbe::Unavailable;
    };
    let Some(arguments) = field("Args:").map(|args| {
        args.split_ascii_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    }) else {
        return McpProbe::Unavailable;
    };
    let scope_value = field("Scope:").unwrap_or_default();
    let expected_scope = expected_mcp_scope(Host::Claude, scope);
    if !scope_value.to_ascii_lowercase().starts_with(expected_scope)
        || field("Environment:").is_none_or(|value| !value.is_empty())
    {
        return McpProbe::Unavailable;
    }
    McpProbe::Present(McpRegistrationReceipt {
        name: "heyfood".to_owned(),
        transport: "stdio".to_owned(),
        command,
        arguments,
        environment: BTreeMap::new(),
        environment_policy_sha256: hex(&Sha256::digest(
            include_bytes!(
                "../../../docs/release-evidence/agent-native-phase0/mcp-environment-policy.json"
            )
            .as_slice(),
        )),
        configuration_scope: expected_scope.to_owned(),
    })
}

fn register_mcp(
    environment: &SetupEnvironment,
    plan: &HostSetupPlan,
    scope: SetupScope,
    project_root: Option<&Path>,
    identity: &McpRegistrationReceipt,
) -> Result<(), SetupError> {
    if environment.host_commands == HostCommandMode::Simulate {
        return Ok(());
    }
    let executable = plan.host_executable.as_deref().ok_or_else(|| {
        SetupError::new(
            "agent_setup_host_command",
            "compatible host executable is unavailable",
        )
    })?;
    let arguments = match plan.host {
        Host::Codex => vec![
            OsString::from("mcp"),
            OsString::from("add"),
            OsString::from("heyfood"),
            OsString::from("--"),
            identity.command.as_os_str().to_owned(),
            OsString::from("mcp"),
            OsString::from("serve"),
        ],
        Host::Claude => vec![
            OsString::from("mcp"),
            OsString::from("add"),
            OsString::from("--transport"),
            OsString::from("stdio"),
            OsString::from("--scope"),
            OsString::from(expected_mcp_scope(Host::Claude, scope)),
            OsString::from("heyfood"),
            OsString::from("--"),
            identity.command.as_os_str().to_owned(),
            OsString::from("mcp"),
            OsString::from("serve"),
        ],
    };
    let output = bounded_host_command(
        executable,
        &arguments,
        project_root,
        HOST_COMMAND_TIMEOUT,
        HOST_COMMAND_OUTPUT_LIMIT,
    );
    let probe = probe_mcp_registration(
        environment,
        &HostProbe {
            host: plan.host,
            executable: Some(executable.to_owned()),
            version: plan.host_version.clone(),
        },
        scope,
        project_root,
        None,
    );
    if probe == McpProbe::Present(identity.clone()) {
        return Ok(());
    }
    match (output, probe) {
        (Ok(output), McpProbe::Missing) if !output.success => Err(SetupError::new(
            "agent_setup_mcp_register",
            "the host rejected MCP registration without changing configuration",
        )),
        _ => Err(SetupError::uncertain(
            "agent_setup_mcp_register",
            "MCP registration could not be verified after the host-owned command",
        )),
    }
}

fn unregister_mcp(
    environment: &SetupEnvironment,
    plan: &HostSetupPlan,
    scope: SetupScope,
    project_root: Option<&Path>,
    prior: &McpRegistrationReceipt,
) -> Result<(), SetupError> {
    if environment.host_commands == HostCommandMode::Simulate {
        return Ok(());
    }
    let executable = plan.host_executable.as_deref().ok_or_else(|| {
        SetupError::new(
            "agent_setup_host_command",
            "compatible host executable is unavailable",
        )
    })?;
    let arguments = match plan.host {
        Host::Codex => vec![
            OsString::from("mcp"),
            OsString::from("remove"),
            OsString::from("heyfood"),
        ],
        Host::Claude => vec![
            OsString::from("mcp"),
            OsString::from("remove"),
            OsString::from("--scope"),
            OsString::from(expected_mcp_scope(Host::Claude, scope)),
            OsString::from("heyfood"),
        ],
    };
    let output = bounded_host_command(
        executable,
        &arguments,
        project_root,
        HOST_COMMAND_TIMEOUT,
        HOST_COMMAND_OUTPUT_LIMIT,
    );
    let probe = probe_mcp_registration(
        environment,
        &HostProbe {
            host: plan.host,
            executable: Some(executable.to_owned()),
            version: plan.host_version.clone(),
        },
        scope,
        project_root,
        None,
    );
    if probe == McpProbe::Missing {
        return Ok(());
    }
    match (output, probe) {
        (Ok(output), McpProbe::Present(current)) if !output.success && current == *prior => {
            Err(SetupError::new(
                "agent_setup_mcp_unregister",
                "the host rejected MCP removal without changing configuration",
            ))
        }
        _ => Err(SetupError::uncertain(
            "agent_setup_mcp_unregister",
            "MCP removal could not be verified after the host-owned command",
        )),
    }
}

struct StagedUninstall {
    skill_parent: CapDir,
    skill_name: OsString,
    skill_tombstone: OsString,
    receipt_parent: CapDir,
    receipt_name: OsString,
    receipt_tombstone: OsString,
}

fn uninstall_hosts_transactionally(
    plans: &[HostSetupPlan],
    environment: &SetupEnvironment,
    scope: SetupScope,
    project_root: Option<&Path>,
) -> Result<(), SetupError> {
    let receipts = plans
        .iter()
        .map(|plan| {
            load_receipt(&plan.receipt_path)?.ok_or_else(|| {
                SetupError::new(
                    "agent_setup_uninstall",
                    "receipt disappeared after the reviewed uninstall plan",
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut staged = Vec::new();
    for plan in plans {
        match stage_uninstall(plan) {
            Ok(item) => staged.push(item),
            Err(error) => {
                if rollback_staged_uninstalls(&staged).is_err() {
                    return Err(SetupError::uncertain(
                        "agent_setup_uninstall_rollback",
                        "uninstall staging failed and restoration could not be verified",
                    ));
                }
                return Err(error);
            }
        }
    }

    let mut removed: Vec<usize> = Vec::new();
    for (plan, receipt) in plans.iter().zip(&receipts) {
        let removal = receipt.mcp.as_ref().map_or(Ok(()), |mcp| {
            unregister_mcp(environment, plan, scope, project_root, mcp)
        });
        if let Err(error) = removal {
            let mut rollback_failed = false;
            for index in removed.into_iter().rev() {
                let prior_plan = &plans[index];
                let prior_receipt = &receipts[index];
                if prior_receipt.mcp.as_ref().is_some_and(|prior_mcp| {
                    register_mcp(environment, prior_plan, scope, project_root, prior_mcp).is_err()
                }) {
                    rollback_failed = true;
                }
            }
            if rollback_staged_uninstalls(&staged).is_err() {
                rollback_failed = true;
            }
            if rollback_failed || error.uncertain {
                return Err(SetupError::uncertain(
                    "agent_setup_uninstall_rollback",
                    "MCP removal failed and complete setup restoration could not be verified",
                ));
            }
            return Err(error);
        }
        removed.push(removed.len());
    }

    for item in &staged {
        if item
            .receipt_parent
            .remove_file(&item.receipt_tombstone)
            .is_err()
            || item
                .skill_parent
                .remove_dir_all(&item.skill_tombstone)
                .is_err()
        {
            return Err(SetupError::uncertain(
                "agent_setup_uninstall_cleanup",
                "all integrations were removed, but private tombstone cleanup could not be verified",
            ));
        }
    }
    Ok(())
}

fn stage_uninstall(plan: &HostSetupPlan) -> Result<StagedUninstall, SetupError> {
    validate_destination(&plan.skill_path)?;
    let skill_parent = plan.skill_path.parent().ok_or_else(|| {
        SetupError::new(
            "agent_setup_path",
            "skill destination has no parent directory",
        )
    })?;
    let receipt_parent = plan.receipt_path.parent().ok_or_else(|| {
        SetupError::new(
            "agent_setup_path",
            "receipt destination has no parent directory",
        )
    })?;
    let skill_parent = AnchoredDirectory::open(skill_parent)?;
    let receipt_parent = AnchoredDirectory::open(receipt_parent)?;
    let skill_name = skill_parent.child_name(&plan.skill_path)?.to_owned();
    let receipt_name = receipt_parent.child_name(&plan.receipt_path)?.to_owned();
    let suffix = format!("{}.{}", plan.host.name(), std::process::id());
    let skill_tombstone = OsString::from(format!(".heyfood.{suffix}.uninstall"));
    let receipt_tombstone = OsString::from(format!(".heyfood.{suffix}.receipt-uninstall"));
    if skill_parent
        .directory
        .symlink_metadata(&skill_tombstone)
        .is_ok()
        || receipt_parent
            .directory
            .symlink_metadata(&receipt_tombstone)
            .is_ok()
    {
        return Err(SetupError::new(
            "agent_setup_uninstall",
            "uninstall staging path already exists",
        ));
    }
    skill_parent
        .directory
        .rename(&skill_name, &skill_parent.directory, &skill_tombstone)
        .map_err(|error| SetupError::new("agent_setup_uninstall", error.to_string()))?;
    let receipt_stage_result =
        if plan.host == Host::Claude && hit_test_failpoint("uninstall_claude_receipt_stage") {
            Err(std::io::Error::other(
                "injected second-host receipt staging failure",
            ))
        } else {
            receipt_parent.directory.rename(
                &receipt_name,
                &receipt_parent.directory,
                &receipt_tombstone,
            )
        };
    if let Err(error) = receipt_stage_result {
        if skill_parent
            .directory
            .rename(&skill_tombstone, &skill_parent.directory, &skill_name)
            .is_err()
        {
            return Err(SetupError::uncertain(
                "agent_setup_uninstall_rollback",
                "receipt staging failed and the skill could not be restored",
            ));
        }
        return Err(SetupError::new("agent_setup_uninstall", error.to_string()));
    }
    Ok(StagedUninstall {
        skill_parent: skill_parent.directory,
        skill_name,
        skill_tombstone,
        receipt_parent: receipt_parent.directory,
        receipt_name,
        receipt_tombstone,
    })
}

fn rollback_staged_uninstalls(staged: &[StagedUninstall]) -> Result<(), SetupError> {
    for item in staged.iter().rev() {
        item.receipt_parent
            .rename(
                &item.receipt_tombstone,
                &item.receipt_parent,
                &item.receipt_name,
            )
            .and_then(|()| {
                item.skill_parent.rename(
                    &item.skill_tombstone,
                    &item.skill_parent,
                    &item.skill_name,
                )
            })
            .map_err(|error| {
                SetupError::new("agent_setup_uninstall_rollback", error.to_string())
            })?;
    }
    Ok(())
}

fn write_skill_files(root: &CapDir, absolute_root: &Path) -> Result<(), SetupError> {
    for (relative, contents) in SKILL_FILES {
        let relative = Path::new(relative);
        let relative_parent = relative
            .parent()
            .expect("every skill file has a parent directory");
        let mut directory = root
            .try_clone()
            .map_err(|error| SetupError::new("agent_setup_write", error.to_string()))?;
        let mut absolute_parent = absolute_root.to_owned();
        for component in relative_parent.components() {
            let Component::Normal(name) = component else {
                return Err(SetupError::new(
                    "agent_setup_write",
                    "embedded skill path is not normalized",
                ));
            };
            absolute_parent.push(name);
            match directory.open_dir_nofollow(name) {
                Ok(next) => directory = next,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    create_private_child_directory(&directory, name)
                        .map_err(|error| SetupError::new("agent_setup_write", error.to_string()))?;
                    directory = directory
                        .open_dir_nofollow(name)
                        .map_err(|error| SetupError::new("agent_setup_write", error.to_string()))?;
                    harden_open_directory(&directory, &absolute_parent)?;
                }
                Err(error) => {
                    return Err(SetupError::new("agent_setup_write", error.to_string()));
                }
            }
        }
        let file_name = relative.file_name().expect("skill path has a file name");
        let mut options = CapOpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = directory
            .open_with(file_name, &options)
            .map_err(|error| SetupError::new("agent_setup_write", error.to_string()))?;
        harden_open_file(&file, &absolute_parent.join(file_name))?;
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
            let canonical_root = fs::canonicalize(root)
                .map_err(|error| SetupError::new("agent_setup_project_root", error.to_string()))?;
            let git = canonical_root.join(".git");
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
            let git = find_executable("git").ok_or_else(|| {
                SetupError::new(
                    "agent_setup_project_root",
                    "project scope requires Git to verify the worktree identity",
                )
            })?;
            let output = bounded_host_command(
                &git,
                &[
                    OsString::from("-C"),
                    canonical_root.as_os_str().to_owned(),
                    OsString::from("rev-parse"),
                    OsString::from("--show-toplevel"),
                ],
                None,
                HOST_COMMAND_TIMEOUT,
                HOST_COMMAND_OUTPUT_LIMIT,
            )
            .map_err(|_| {
                SetupError::new(
                    "agent_setup_project_root",
                    "project root must identify an existing Git worktree",
                )
            })?;
            if !output.success {
                return Err(SetupError::new(
                    "agent_setup_project_root",
                    "project root must identify an existing Git worktree",
                ));
            }
            let reported = String::from_utf8(output.stdout).map_err(|_| {
                SetupError::new(
                    "agent_setup_project_root",
                    "Git returned a non-UTF-8 worktree identity",
                )
            })?;
            let reported = fs::canonicalize(reported.trim()).map_err(|_| {
                SetupError::new(
                    "agent_setup_project_root",
                    "Git returned an invalid worktree identity",
                )
            })?;
            if reported != canonical_root {
                return Err(SetupError::new(
                    "agent_setup_project_root",
                    "project root must be the exact Git worktree top level",
                ));
            }
            Ok(Some(canonical_root))
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
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(SetupError::new("agent_setup_receipt", error.to_string())),
    }
    let parent = path.parent().ok_or_else(|| {
        SetupError::new(
            "agent_setup_receipt",
            "receipt destination has no parent directory",
        )
    })?;
    let anchored_parent = AnchoredDirectory::open(parent)?;
    let name = anchored_parent.child_name(path)?;
    let metadata = anchored_parent
        .directory
        .symlink_metadata(name)
        .map_err(|error| SetupError::new("agent_setup_receipt", error.to_string()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(SetupError::new(
            "agent_setup_receipt",
            "setup receipt must be one regular, unlinked file",
        ));
    }
    let mut options = CapOpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let file = anchored_parent
        .directory
        .open_with(name, &options)
        .map_err(|error| SetupError::new("agent_setup_receipt", error.to_string()))?;
    if cap_file_is_hardlinked(&file)? {
        return Err(SetupError::new(
            "agent_setup_receipt",
            "setup receipt must be one regular, unlinked file",
        ));
    }
    let mut bytes = Vec::new();
    file.take(64 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|error| SetupError::new("agent_setup_receipt", error.to_string()))?;
    let receipt: Receipt = serde_json::from_slice(&bytes)
        .map_err(|_| SetupError::new("agent_setup_receipt", "setup receipt is malformed"))?;
    let supported = matches!(
        (receipt.schema_version, receipt.mcp.is_some()),
        (1, false) | (2, true)
    );
    if !supported {
        return Err(SetupError::new(
            "agent_setup_receipt",
            "setup receipt version is unsupported",
        ));
    }
    Ok(Some(receipt))
}

fn inspect_skill(path: &Path) -> Result<Option<BTreeMap<String, String>>, SetupError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(SetupError::new("agent_setup_inspect", error.to_string())),
    }
    let parent = path.parent().ok_or_else(|| {
        SetupError::new(
            "agent_setup_inspect",
            "skill destination has no parent directory",
        )
    })?;
    let anchored_parent = AnchoredDirectory::open(parent)?;
    let name = anchored_parent.child_name(path)?;
    let metadata = anchored_parent
        .directory
        .symlink_metadata(name)
        .map_err(|error| SetupError::new("agent_setup_inspect", error.to_string()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(SetupError::new(
            "agent_setup_redirect",
            "skill destination must be a regular directory, not a redirect",
        ));
    }
    let directory = anchored_parent
        .directory
        .open_dir_nofollow(name)
        .map_err(|error| SetupError::new("agent_setup_redirect", error.to_string()))?;
    let mut files = BTreeMap::new();
    inspect_directory(&directory, Path::new(""), &mut files)?;
    Ok(Some(files))
}

fn inspect_directory(
    directory: &CapDir,
    relative_root: &Path,
    files: &mut BTreeMap<String, String>,
) -> Result<(), SetupError> {
    for entry in directory
        .entries()
        .map_err(|error| SetupError::new("agent_setup_inspect", error.to_string()))?
    {
        let entry =
            entry.map_err(|error| SetupError::new("agent_setup_inspect", error.to_string()))?;
        let name = entry.file_name();
        let metadata = directory
            .symlink_metadata(&name)
            .map_err(|error| SetupError::new("agent_setup_inspect", error.to_string()))?;
        if metadata.file_type().is_symlink() {
            return Err(SetupError::new(
                "agent_setup_redirect",
                "installed skill contains a symlink or reparse point",
            ));
        }
        if metadata.is_dir() {
            let child = directory
                .open_dir_nofollow(&name)
                .map_err(|error| SetupError::new("agent_setup_inspect", error.to_string()))?;
            inspect_directory(&child, &relative_root.join(&name), files)?;
        } else if metadata.is_file() {
            let mut options = CapOpenOptions::new();
            options.read(true).follow(FollowSymlinks::No);
            let file = directory
                .open_with(&name, &options)
                .map_err(|error| SetupError::new("agent_setup_inspect", error.to_string()))?;
            if cap_file_is_hardlinked(&file)? {
                return Err(SetupError::new(
                    "agent_setup_redirect",
                    "installed skill contains a non-regular or linked file",
                ));
            }
            let relative = relative_root
                .join(&name)
                .to_string_lossy()
                .replace('\\', "/");
            files.insert(relative, sha256_cap_file(file)?);
        } else {
            return Err(SetupError::new(
                "agent_setup_redirect",
                "installed skill contains a non-regular or linked file",
            ));
        }
    }
    Ok(())
}

fn sha256_cap_file(mut file: CapFile) -> Result<String, SetupError> {
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

#[cfg(unix)]
fn cap_file_is_hardlinked(file: &CapFile) -> Result<bool, SetupError> {
    use cap_std::fs::MetadataExt as _;
    file.metadata()
        .map(|metadata| metadata.nlink() > 1)
        .map_err(|error| SetupError::new("agent_setup_inspect", error.to_string()))
}

#[cfg(windows)]
fn cap_file_is_hardlinked(file: &CapFile) -> Result<bool, SetupError> {
    let file = file
        .try_clone()
        .map(CapFile::into_std)
        .map_err(|error| SetupError::new("agent_setup_inspect", error.to_string()))?;
    heyfood_windows_file::file_identity(&file)
        .map(|identity| identity.number_of_links > 1)
        .map_err(|error| SetupError::new("agent_setup_inspect", error.to_string()))
}

#[cfg(not(any(unix, windows)))]
fn cap_file_is_hardlinked(_file: &CapFile) -> Result<bool, SetupError> {
    Ok(true)
}

fn validate_destination(path: &Path) -> Result<(), SetupError> {
    let _ = inspect_skill(path)?;
    Ok(())
}

fn validate_existing_directory(path: &Path) -> Result<(), SetupError> {
    AnchoredDirectory::open(path).map(|_| ())
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
    let version = executable
        .as_ref()
        .and_then(|executable| bounded_host_version(executable));
    HostProbe {
        host,
        executable,
        version,
    }
}

fn bounded_host_version(executable: &Path) -> Option<String> {
    let output = bounded_host_command(
        executable,
        &[OsString::from("--version")],
        None,
        HOST_PROBE_TIMEOUT,
        HOST_PROBE_OUTPUT_LIMIT,
    )
    .ok()?;
    if !output.success {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

struct BoundedCommandOutput {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn bounded_host_command(
    executable: &Path,
    arguments: &[OsString],
    current_dir: Option<&Path>,
    timeout: Duration,
    output_limit: u64,
) -> Result<BoundedCommandOutput, ()> {
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(current_dir) = current_dir {
        command.current_dir(current_dir);
    }
    let mut child = command.spawn().map_err(|_| ())?;
    let stdout = child.stdout.take().ok_or(())?;
    let stderr = child.stderr.take().ok_or(())?;
    let spawn_reader = |reader: Box<dyn Read + Send>| {
        let (sender, receiver) = mpsc::sync_channel(1);
        let handle = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = reader
                .take(output_limit + 1)
                .read_to_end(&mut bytes)
                .map(|_| bytes);
            let _ = sender.send(result);
        });
        (receiver, handle)
    };
    let (stdout_receiver, stdout_reader) = spawn_reader(Box::new(stdout));
    let (stderr_receiver, stderr_reader) = spawn_reader(Box::new(stderr));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => std::thread::sleep(LOCK_RETRY),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(());
            }
        }
    };
    let stdout = stdout_receiver
        .recv_timeout(Duration::from_millis(250))
        .map_err(|_| ())?
        .map_err(|_| ())?;
    let stderr = stderr_receiver
        .recv_timeout(Duration::from_millis(250))
        .map_err(|_| ())?
        .map_err(|_| ())?;
    let _ = stdout_reader.join();
    let _ = stderr_reader.join();
    if stdout.len() as u64 > output_limit || stderr.len() as u64 > output_limit {
        return Err(());
    }
    Ok(BoundedCommandOutput {
        success: status.success(),
        stdout,
        stderr,
    })
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

struct SetupLock {
    file: File,
}

impl SetupLock {
    fn acquire(path: &Path) -> Result<Self, SetupError> {
        let parent = path.parent().ok_or_else(|| {
            SetupError::new("agent_setup_lock", "setup lock has no parent directory")
        })?;
        let anchored_parent = AnchoredDirectory::open_or_create(parent)?;
        let name = anchored_parent.child_name(path)?;
        let mut options = CapOpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        options.follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let open_started = Instant::now();
        let file = loop {
            match anchored_parent.directory.open_with(name, &options) {
                Ok(file) => break file,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::Interrupted
                    ) && open_started.elapsed() < LOCK_TIMEOUT =>
                {
                    // A concurrent creator can briefly expose an absent
                    // directory entry between its no-follow create/open
                    // syscalls. Retry only through the already-open parent
                    // handle; never fall back to path traversal.
                    std::thread::sleep(LOCK_RETRY);
                }
                Err(error) => {
                    return Err(SetupError::new("agent_setup_lock", error.to_string()));
                }
            }
        };
        harden_open_file(&file, path)?;
        if cap_file_is_hardlinked(&file)? {
            return Err(SetupError::new(
                "agent_setup_lock",
                "setup lock must be one regular, unlinked file",
            ));
        }
        let file = file.into_std();
        let started = Instant::now();
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Self { file }),
                Err(error) if lock_is_contended(&error) => {
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

fn lock_is_contended(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        // LockFileEx reports ERROR_LOCK_VIOLATION for an occupied byte range,
        // while Rust currently classifies Win32 error 33 as `Other`.
        const ERROR_LOCK_VIOLATION: i32 = 33;
        error.raw_os_error() == Some(ERROR_LOCK_VIOLATION)
    }
    #[cfg(not(windows))]
    false
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
            host_commands: HostCommandMode::Simulate,
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
            expected_plan_sha256: None,
        }
    }

    fn authorize(mut request: SetupOptions, environment: &SetupEnvironment) -> SetupOptions {
        request.mode = SetupMode::DryRun;
        request.expected_plan_sha256 = None;
        let digest = build_plan(&request, environment).unwrap().plan_sha256;
        request.mode = SetupMode::Apply;
        request.expected_plan_sha256 = Some(digest);
        request
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
    fn anchored_directory_durability_uses_a_syncable_handle() {
        let root = scratch("directory-sync");
        let directory = AnchoredDirectory::open(&root).unwrap();
        sync_anchored_directory(&directory.directory).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_lock_violation_is_retryable_contention() {
        let error = std::io::Error::from_raw_os_error(33);
        assert_ne!(error.kind(), std::io::ErrorKind::WouldBlock);
        assert!(lock_is_contended(&error));
    }

    #[test]
    fn apply_is_exact_idempotent_and_reversible() {
        let root = scratch("apply");
        let environment = environment(&root);
        let applied = execute_with_environment(
            &authorize(
                options(SetupMode::Apply, SetupOperation::Install),
                &environment,
            ),
            &environment,
        )
        .unwrap();
        assert!(applied.changed);
        let repeated = execute_with_environment(
            &authorize(
                options(SetupMode::Apply, SetupOperation::Install),
                &environment,
            ),
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
            &authorize(
                options(SetupMode::Apply, SetupOperation::Uninstall),
                &environment,
            ),
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
            &authorize(
                options(SetupMode::Apply, SetupOperation::Install),
                &environment,
            ),
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
                &authorize(
                    options(SetupMode::Apply, SetupOperation::Install),
                    &environment,
                ),
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
            &authorize(
                options(SetupMode::Apply, SetupOperation::Install),
                &environment,
            ),
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
            &authorize(
                options(SetupMode::Apply, SetupOperation::Install),
                &environment,
            ),
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

        let mut replace = options(SetupMode::DryRun, SetupOperation::Install);
        replace.target = SetupTarget::Codex;
        replace.replace = true;
        let replace = authorize(replace, &environment);
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
        fs::create_dir_all(&project).unwrap();
        let fake = root.join("fake-repo");
        fs::create_dir_all(fake.join(".git")).unwrap();
        request.project_root = Some(fake);
        assert!(execute_with_environment(&request, &environment).is_err());
        assert!(
            Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(&project)
                .status()
                .unwrap()
                .success()
        );
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
                &authorize(
                    options(SetupMode::Apply, SetupOperation::Install),
                    &environment,
                ),
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
        let reviewed = authorize(
            options(SetupMode::Apply, SetupOperation::Install),
            &environment,
        );
        let first_reviewed = reviewed.clone();
        let second_reviewed = reviewed;
        let first = std::thread::spawn(move || {
            execute_with_environment(&first_reviewed, &first_environment)
        });
        let second = std::thread::spawn(move || {
            execute_with_environment(&second_reviewed, &second_environment)
        });
        let first = first.join().unwrap();
        let second = second.join().unwrap();
        assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
        let changed = first.err().or_else(|| second.err()).unwrap();
        assert_eq!(
            changed.kind, "agent_setup_plan_changed",
            "unexpected concurrent apply result: {changed:?}"
        );
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
    fn apply_requires_the_exact_reviewed_plan_digest() {
        let root = scratch("plan-digest");
        let environment = environment(&root);
        let missing = execute_with_environment(
            &options(SetupMode::Apply, SetupOperation::Install),
            &environment,
        )
        .unwrap_err();
        assert_eq!(missing.kind, "agent_setup_plan_required");

        let mut changed = options(SetupMode::Apply, SetupOperation::Install);
        changed.expected_plan_sha256 = Some("0".repeat(64));
        let changed = execute_with_environment(&changed, &environment).unwrap_err();
        assert_eq!(changed.kind, "agent_setup_plan_changed");
        assert!(!environment.home_dir.exists());
    }

    #[test]
    fn receipt_v1_skill_can_be_explicitly_migrated_then_uninstalled() {
        let root = scratch("receipt-v1-migration");
        let environment = environment(&root);
        let installed = execute_with_environment(
            &authorize(
                options(SetupMode::Apply, SetupOperation::Install),
                &environment,
            ),
            &environment,
        )
        .unwrap();
        let receipt_path = &installed.hosts[0].receipt_path;
        let mut legacy: serde_json::Value =
            serde_json::from_slice(&fs::read(receipt_path).unwrap()).unwrap();
        legacy["schema_version"] = serde_json::Value::from(1);
        legacy.as_object_mut().unwrap().remove("mcp");
        fs::write(receipt_path, serde_json::to_vec(&legacy).unwrap()).unwrap();

        let blocked = build_plan(
            &options(SetupMode::DryRun, SetupOperation::Install),
            &environment,
        )
        .unwrap();
        assert_eq!(blocked.hosts[0].action, "conflict");
        assert!(
            blocked.hosts[0]
                .user_actions
                .iter()
                .any(|action| action.contains("--replace"))
        );

        let mut replace = options(SetupMode::DryRun, SetupOperation::Install);
        replace.target = SetupTarget::Codex;
        replace.replace = true;
        execute_with_environment(&authorize(replace, &environment), &environment).unwrap();
        let migrated: serde_json::Value =
            serde_json::from_slice(&fs::read(receipt_path).unwrap()).unwrap();
        assert_eq!(migrated["schema_version"], 2);
        assert_eq!(
            migrated["mcp"]["arguments"],
            serde_json::json!(["mcp", "serve"])
        );

        let mut uninstall = options(SetupMode::DryRun, SetupOperation::Uninstall);
        uninstall.target = SetupTarget::Codex;
        execute_with_environment(&authorize(uninstall, &environment), &environment).unwrap();
        assert!(!installed.hosts[0].skill_path.exists());
        assert!(!receipt_path.exists());
    }

    #[test]
    fn uncertain_receipt_commit_is_preserved_for_explicit_reconciliation() {
        let root = scratch("receipt-uncertain");
        let environment = environment(&root);
        set_test_failpoints(&["receipt_after_rename"]);
        let error = execute_with_environment(
            &authorize(
                options(SetupMode::Apply, SetupOperation::Install),
                &environment,
            ),
            &environment,
        )
        .unwrap_err();
        assert!(error.uncertain);
        assert_eq!(error.kind, "agent_setup_receipt_outcome_uncertain");

        let reconciled = execute_with_environment(
            &options(SetupMode::DryRun, SetupOperation::Install),
            &environment,
        )
        .unwrap();
        assert_eq!(reconciled.hosts[0].action, "none");
        assert!(reconciled.hosts[0].receipt_path.is_file());
        assert!(reconciled.hosts[0].skill_path.is_dir());
    }

    #[test]
    fn failed_receipt_commit_restores_prior_replacement_or_reports_uncertainty() {
        for rollback_fails in [false, true] {
            let root = scratch(if rollback_fails {
                "receipt-rollback-uncertain"
            } else {
                "receipt-rollback"
            });
            let environment = environment(&root);
            let installed = execute_with_environment(
                &authorize(
                    options(SetupMode::Apply, SetupOperation::Install),
                    &environment,
                ),
                &environment,
            )
            .unwrap();
            let prior_receipt = fs::read(&installed.hosts[0].receipt_path).unwrap();
            fs::write(&environment.heyfood_executable, b"replacement-binary").unwrap();
            let mut replace = options(SetupMode::DryRun, SetupOperation::Install);
            replace.target = SetupTarget::Codex;
            replace.replace = true;
            set_test_failpoints(if rollback_fails {
                &["receipt_before_rename", "receipt_rollback_remove"]
            } else {
                &["receipt_before_rename"]
            });
            let error = execute_with_environment(&authorize(replace, &environment), &environment)
                .unwrap_err();
            assert_eq!(error.uncertain, rollback_fails);
            if !rollback_fails {
                assert_eq!(
                    fs::read(&installed.hosts[0].receipt_path).unwrap(),
                    prior_receipt
                );
                assert!(installed.hosts[0].skill_path.is_dir());
            }
        }
    }

    #[test]
    fn failed_skill_publish_restores_prior_replacement_or_reports_uncertainty() {
        for rollback_fails in [false, true] {
            let root = scratch(if rollback_fails {
                "publish-rollback-uncertain"
            } else {
                "publish-rollback"
            });
            let environment = environment(&root);
            let mut initial = options(SetupMode::DryRun, SetupOperation::Install);
            initial.target = SetupTarget::Codex;
            let installed =
                execute_with_environment(&authorize(initial, &environment), &environment).unwrap();
            let prior_receipt = fs::read(&installed.hosts[0].receipt_path).unwrap();
            fs::write(&environment.heyfood_executable, b"replacement-binary").unwrap();
            let mut replace = options(SetupMode::DryRun, SetupOperation::Install);
            replace.target = SetupTarget::Codex;
            replace.replace = true;
            set_test_failpoints(if rollback_fails {
                &["skill_commit_publish", "skill_stage_rollback_restore"]
            } else {
                &["skill_commit_publish"]
            });
            let error = execute_with_environment(&authorize(replace, &environment), &environment)
                .unwrap_err();
            assert_eq!(error.uncertain, rollback_fails);
            if rollback_fails {
                assert_eq!(error.kind, "agent_setup_commit_rollback");
            } else {
                assert_eq!(error.kind, "agent_setup_commit");
                assert!(installed.hosts[0].skill_path.is_dir());
                assert_eq!(
                    fs::read(&installed.hosts[0].receipt_path).unwrap(),
                    prior_receipt
                );
            }
        }
    }

    #[test]
    fn failed_post_publish_validation_removes_install_or_reports_uncertainty() {
        for rollback_fails in [false, true] {
            let root = scratch(if rollback_fails {
                "validation-rollback-uncertain"
            } else {
                "validation-rollback"
            });
            let environment = environment(&root);
            let mut install = options(SetupMode::DryRun, SetupOperation::Install);
            install.target = SetupTarget::Codex;
            let skill_path = build_plan(&install, &environment).unwrap().hosts[0]
                .skill_path
                .clone();
            let reviewed = authorize(install, &environment);
            set_test_failpoints(if rollback_fails {
                &[
                    "skill_post_publish_validation",
                    "skill_installed_rollback_remove",
                ]
            } else {
                &["skill_post_publish_validation"]
            });
            let error = execute_with_environment(&reviewed, &environment).unwrap_err();
            assert_eq!(error.uncertain, rollback_fails);
            if rollback_fails {
                assert_eq!(error.kind, "agent_setup_validation_rollback");
                assert!(skill_path.is_dir());
            } else {
                assert_eq!(error.kind, "agent_setup_stage");
                assert!(!skill_path.exists());
            }
        }
    }

    #[test]
    fn multi_host_uninstall_rolls_back_every_staged_host_on_partial_failure() {
        let root = scratch("uninstall-transaction");
        let environment = environment(&root);
        let installed = execute_with_environment(
            &authorize(
                options(SetupMode::Apply, SetupOperation::Install),
                &environment,
            ),
            &environment,
        )
        .unwrap();
        set_test_failpoints(&["uninstall_claude_receipt_stage"]);
        let error = execute_with_environment(
            &authorize(
                options(SetupMode::Apply, SetupOperation::Uninstall),
                &environment,
            ),
            &environment,
        )
        .unwrap_err();
        assert!(!error.uncertain);
        for host in installed.hosts {
            assert!(host.skill_path.is_dir());
            assert!(host.receipt_path.is_file());
        }
    }

    #[cfg(unix)]
    #[test]
    fn installed_skill_receipts_and_lock_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("permissions");
        let environment = environment(&root);
        let installed = execute_with_environment(
            &authorize(
                options(SetupMode::Apply, SetupOperation::Install),
                &environment,
            ),
            &environment,
        )
        .unwrap();
        assert_eq!(
            fs::metadata(environment.state_dir.join("setup.lock"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        for host in installed.hosts {
            assert_eq!(
                fs::metadata(&host.skill_path).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(host.skill_path.join("SKILL.md"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(host.receipt_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn host_version_probe_has_deadline_and_output_bound() {
        use std::os::unix::fs::PermissionsExt;

        fn script(root: &Path, name: &str, body: &str) -> PathBuf {
            let path = root.join(name);
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            path
        }

        let root = scratch("probe-bounds");
        let exact = script(&root, "exact", "printf 'codex-cli 0.145.0-alpha.18\\n'");
        assert_eq!(
            bounded_host_version(&exact).as_deref(),
            Some("codex-cli 0.145.0-alpha.18")
        );
        let oversized = script(&root, "oversized", "head -c 5000 /dev/zero");
        assert!(bounded_host_version(&oversized).is_none());
        let hanging = script(&root, "hanging", "exec sleep 30");
        let started = Instant::now();
        assert!(bounded_host_version(&hanging).is_none());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn host_owned_codex_registration_round_trips_with_receipt_bound_setup() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("codex-mcp-round-trip");
        let mut environment = environment(&root);
        let host_state = root.join("codex-mcp-state.json");
        let script = root.join("codex-host");
        let body = format!(
            r#"#!/bin/sh
set -eu
state='{}'
if [ "$1" = "mcp" ] && [ "$2" = "get" ]; then
  if [ -f "$state" ]; then cat "$state"; exit 0; fi
  echo "Error: No MCP server named 'heyfood' found." >&2
  exit 1
fi
if [ "$1" = "mcp" ] && [ "$2" = "add" ]; then
  command_path="$5"
  printf '{{"name":"heyfood","transport":{{"type":"stdio","command":"%s","args":["mcp","serve"],"env":null,"env_vars":[],"cwd":null}}}}' "$command_path" > "$state"
  exit 0
fi
if [ "$1" = "mcp" ] && [ "$2" = "remove" ]; then
  rm -f "$state"
  exit 0
fi
exit 2
"#,
            host_state.display()
        );
        fs::write(&script, body).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        environment.probes = vec![HostProbe {
            host: Host::Codex,
            executable: Some(script),
            version: Some(CODEX_VERSION.to_owned()),
        }];
        environment.host_commands = HostCommandMode::Execute;
        let mut install = options(SetupMode::DryRun, SetupOperation::Install);
        install.target = SetupTarget::Codex;
        let installed =
            execute_with_environment(&authorize(install, &environment), &environment).unwrap();
        assert!(host_state.is_file());
        assert_eq!(installed.hosts[0].mcp.action, "install");

        let mut repeat = options(SetupMode::DryRun, SetupOperation::Install);
        repeat.target = SetupTarget::Codex;
        assert_eq!(
            execute_with_environment(&repeat, &environment)
                .unwrap()
                .hosts[0]
                .action,
            "none"
        );

        let mut uninstall = options(SetupMode::DryRun, SetupOperation::Uninstall);
        uninstall.target = SetupTarget::Codex;
        execute_with_environment(&authorize(uninstall, &environment), &environment).unwrap();
        assert!(!host_state.exists());
        assert!(!installed.hosts[0].skill_path.exists());
        assert!(!installed.hosts[0].receipt_path.exists());
    }

    #[test]
    fn exact_host_probe_parsers_reject_environment_or_scope_drift() {
        let binary = PathBuf::from("/absolute/heyfood");
        let codex = format!(
            r#"{{"transport":{{"type":"stdio","command":"{}","args":["mcp","serve"],"env":null,"env_vars":[]}}}}"#,
            binary.display()
        );
        let expected = McpRegistrationReceipt {
            name: "heyfood".to_owned(),
            transport: "stdio".to_owned(),
            command: binary.clone(),
            arguments: vec!["mcp".to_owned(), "serve".to_owned()],
            environment: BTreeMap::new(),
            environment_policy_sha256: hex(&Sha256::digest(
                include_bytes!(
                    "../../../docs/release-evidence/agent-native-phase0/mcp-environment-policy.json"
                )
                .as_slice(),
            )),
            configuration_scope: "user".to_owned(),
        };
        assert_eq!(
            parse_codex_mcp_probe(codex.as_bytes()),
            McpProbe::Present(expected.clone())
        );
        assert_eq!(
            parse_codex_mcp_probe(
                codex
                    .replace("\"env\":null", "\"env\":{\"X\":\"1\"}")
                    .as_bytes()
            ),
            McpProbe::Unavailable
        );

        let claude = b"heyfood:\n  Scope: User config\n  Type: stdio\n  Command: /absolute/heyfood\n  Args: mcp serve\n  Environment:\n";
        assert_eq!(
            parse_claude_mcp_probe(claude, SetupScope::User),
            McpProbe::Present(expected)
        );
        assert_eq!(
            parse_claude_mcp_probe(claude, SetupScope::Project),
            McpProbe::Unavailable
        );
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
