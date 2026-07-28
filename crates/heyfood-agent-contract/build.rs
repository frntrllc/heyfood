use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

fn command(root: &Path, program: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(arguments)
        .current_dir(root)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn watch_git_identity(root: &Path) {
    if let Some(head) = command(root, "git", &["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head}");
    }
    if let Some(reference) = command(root, "git", &["symbolic-ref", "-q", "HEAD"])
        && let Some(path) = command(root, "git", &["rev-parse", "--git-path", &reference])
    {
        println!("cargo:rerun-if-changed={path}");
    }
}

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let watched = [
        "docs/AGENT_INTEGRATION.md",
        "docs/AGENT_SAFETY.md",
        "schemas/v1/heyfood-agent-manifest.schema.json",
        "schemas/v1/heyfood-agent-schema-index.schema.json",
        "schemas/v1/heyfood-agent-doctor.schema.json",
        "schemas/v1/heyfood-output.schema.json",
        "schemas/v1/agent-proposal-presentation.schema.json",
    ];
    for relative in watched {
        println!("cargo:rerun-if-changed={}", root.join(relative).display());
    }
    watch_git_identity(root);
    println!("cargo:rerun-if-env-changed=HEYFOOD_DISTRIBUTION_CHANNEL");

    let source_commit = command(root, "git", &["rev-parse", "HEAD"])
        .filter(|value| value.len() == 40)
        .unwrap_or_else(|| "0000000000000000000000000000000000000000".to_owned());
    let source_tree = command(root, "git", &["rev-parse", "HEAD^{tree}"])
        .filter(|value| value.len() == 40)
        .unwrap_or_else(|| "0000000000000000000000000000000000000000".to_owned());
    let dirty = command(
        root,
        "git",
        &["status", "--porcelain", "--untracked-files=normal"],
    )
    .is_none_or(|value| !value.is_empty());
    let target = env::var("TARGET").unwrap_or_else(|_| "unknown-target".to_owned());
    let toolchain = env::var("RUSTC")
        .ok()
        .and_then(|rustc| command(root, &rustc, &["--version"]))
        .unwrap_or_else(|| "rustc-unknown".to_owned());
    let distribution_channel =
        env::var("HEYFOOD_DISTRIBUTION_CHANNEL").unwrap_or_else(|_| "development".to_owned());

    let mut features = env::vars()
        .filter_map(|(name, value)| {
            (value == "1")
                .then(|| name.strip_prefix("CARGO_FEATURE_").map(str::to_owned))
                .flatten()
        })
        .map(|name| name.to_ascii_lowercase().replace('_', "-"))
        .collect::<Vec<_>>();
    features.sort();

    let mut digest = Sha256::new();
    for value in [
        source_commit.as_bytes(),
        source_tree.as_bytes(),
        target.as_bytes(),
        toolchain.as_bytes(),
        distribution_channel.as_bytes(),
    ] {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value);
    }
    for feature in &features {
        digest.update((feature.len() as u64).to_be_bytes());
        digest.update(feature.as_bytes());
    }
    for relative in watched {
        let bytes = fs::read(root.join(relative)).expect("read embedded contract input");
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }

    println!("cargo:rustc-env=HEYFOOD_BUILD_SOURCE_COMMIT={source_commit}");
    println!("cargo:rustc-env=HEYFOOD_BUILD_SOURCE_TREE={source_tree}");
    println!("cargo:rustc-env=HEYFOOD_BUILD_DIRTY={dirty}");
    println!("cargo:rustc-env=HEYFOOD_BUILD_TARGET={target}");
    println!("cargo:rustc-env=HEYFOOD_BUILD_TOOLCHAIN={toolchain}");
    println!("cargo:rustc-env=HEYFOOD_BUILD_DISTRIBUTION_CHANNEL={distribution_channel}");
    println!(
        "cargo:rustc-env=HEYFOOD_BUILD_FEATURES={}",
        features.join(",")
    );
    println!(
        "cargo:rustc-env=HEYFOOD_BUILD_INPUT_DIGEST={:x}",
        digest.finalize()
    );
}
