use std::fs;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use heyfood_platform::SensitiveExportWriter;

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "heyfood-sensitive-export-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn sensitive_export_is_exclusive_private_and_atomically_replaceable() {
    let directory = TestDirectory::new("replace");
    let target = directory.path().join("grocery.md");

    SensitiveExportWriter::write(&target, b"first export", false).unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"first export");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    let error = SensitiveExportWriter::write(&target, b"must not replace", false).unwrap_err();
    assert_eq!(error.code, "export_exists");
    assert_eq!(fs::read(&target).unwrap(), b"first export");

    SensitiveExportWriter::write(&target, b"replacement export", true).unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"replacement export");
    #[cfg(windows)]
    verify_windows_owner_only(&target);
    assert!(fs::read_dir(directory.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".heyfood-export.")
    }));
}

#[test]
fn concurrent_exclusive_exports_leave_one_complete_file_and_no_staging_files() {
    let directory = TestDirectory::new("concurrent");
    let target = directory.path().join("grocery.txt");
    let barrier = Arc::new(Barrier::new(2));
    let writers = [
        b"first complete export".as_slice(),
        b"second complete export".as_slice(),
    ]
    .into_iter()
    .map(|bytes| {
        let target = target.clone();
        let barrier = barrier.clone();
        thread::spawn(move || {
            barrier.wait();
            SensitiveExportWriter::write(&target, bytes, false)
        })
    })
    .collect::<Vec<_>>();
    let results = writers
        .into_iter()
        .map(|writer| writer.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .filter(|error| error.code == "export_exists")
            .count(),
        1
    );
    let bytes = fs::read(&target).unwrap();
    assert!(bytes == b"first complete export" || bytes == b"second complete export");
    assert!(fs::read_dir(directory.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".heyfood-export.")
    }));
}

#[cfg(unix)]
#[test]
fn sensitive_export_rejects_target_and_parent_symlinks_without_touching_victims() {
    use std::os::unix::fs::symlink;

    let directory = TestDirectory::new("symlink");
    let victim = directory.path().join("victim.md");
    fs::write(&victim, b"victim").unwrap();
    let target_link = directory.path().join("linked.md");
    symlink(&victim, &target_link).unwrap();

    let error = SensitiveExportWriter::write(&target_link, b"secret", true).unwrap_err();
    assert_eq!(error.code, "export_redirect");
    assert_eq!(fs::read(&victim).unwrap(), b"victim");

    let real_parent = directory.path().join("real");
    fs::create_dir(&real_parent).unwrap();
    let linked_parent = directory.path().join("linked-parent");
    symlink(&real_parent, &linked_parent).unwrap();
    let error = SensitiveExportWriter::write(&linked_parent.join("grocery.md"), b"secret", false)
        .unwrap_err();
    assert_eq!(error.code, "export_parent");
    assert!(!real_parent.join("grocery.md").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn sensitive_export_accepts_non_utf8_file_names() {
    use std::os::unix::ffi::OsStringExt;

    let directory = TestDirectory::new("non-utf8");
    let target = directory
        .path()
        .join(std::ffi::OsString::from_vec(b"grocery-\xff.json".to_vec()));
    SensitiveExportWriter::write(&target, br#"{"safe":true}"#, false).unwrap();
    assert_eq!(fs::read(target).unwrap(), br#"{"safe":true}"#);
}

#[cfg(windows)]
#[test]
fn sensitive_export_rejects_windows_reparse_targets_and_parents() {
    let directory = TestDirectory::new("reparse");
    let real_target = directory.path().join("real-target");
    let real_parent = directory.path().join("real-parent");
    fs::create_dir(&real_target).unwrap();
    fs::create_dir(&real_parent).unwrap();
    let target_junction = directory.path().join("target-junction");
    let parent_junction = directory.path().join("parent-junction");
    create_windows_junction(&target_junction, &real_target);
    create_windows_junction(&parent_junction, &real_parent);

    let error =
        SensitiveExportWriter::write(&target_junction, b"must not publish", true).unwrap_err();
    assert_eq!(error.code, "export_redirect");
    let error = SensitiveExportWriter::write(
        &parent_junction.join("grocery.json"),
        b"must not publish",
        false,
    )
    .unwrap_err();
    assert_eq!(error.code, "export_parent");
    assert!(!real_parent.join("grocery.json").exists());
}

#[cfg(windows)]
#[test]
fn sensitive_export_replaces_a_hard_link_without_mutating_its_other_name() {
    let directory = TestDirectory::new("hard-link");
    let victim = directory.path().join("victim.json");
    let target = directory.path().join("grocery.json");
    fs::write(&victim, b"victim").unwrap();
    fs::hard_link(&victim, &target).unwrap();

    SensitiveExportWriter::write(&target, b"private export", true).unwrap();
    assert_eq!(fs::read(&victim).unwrap(), b"victim");
    assert_eq!(fs::read(&target).unwrap(), b"private export");
    verify_windows_owner_only(&target);
}

#[cfg(windows)]
#[test]
fn failed_windows_publish_cleans_owner_only_staging_file() {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    let directory = TestDirectory::new("locked-target");
    let target = directory.path().join("grocery.json");
    fs::write(&target, b"original").unwrap();
    let locked = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&target)
        .unwrap();

    let error = SensitiveExportWriter::write(&target, b"must not replace", true).unwrap_err();
    assert_eq!(error.code, "export_write");
    drop(locked);
    assert_eq!(fs::read(&target).unwrap(), b"original");
    assert_no_export_staging(directory.path());
}

#[cfg(windows)]
fn create_windows_junction(path: &Path, target: &Path) {
    let output = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "New-Item -ItemType Junction -Path $env:HEYFOOD_LINK -Target $env:HEYFOOD_LINK_TARGET -ErrorAction Stop | Out-Null",
        ])
        .env("HEYFOOD_LINK", path)
        .env("HEYFOOD_LINK_TARGET", target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(windows)]
fn verify_windows_owner_only(path: &Path) {
    let output = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            r#"
$ErrorActionPreference = 'Stop'
$expected = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
$acl = [System.IO.File]::GetAccessControl($env:HEYFOOD_ACL_TARGET)
if (-not $acl.AreAccessRulesProtected) { throw 'DACL is not protected' }
if ($acl.GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $expected.Value) { throw 'owner mismatch' }
$rules = @($acl.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier]))
if ($rules.Count -ne 1) { throw 'unexpected ACE count' }
$rule = $rules[0]
if ($rule.IdentityReference.Value -ne $expected.Value) { throw 'foreign ACE' }
if ($rule.IsInherited) { throw 'inherited ACE' }
if ($rule.AccessControlType -ne [System.Security.AccessControl.AccessControlType]::Allow) { throw 'deny ACE' }
if ($rule.FileSystemRights -ne [System.Security.AccessControl.FileSystemRights]::FullControl) { throw 'owner lacks full control' }
"#,
        ])
        .env("HEYFOOD_ACL_TARGET", path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(windows)]
fn assert_no_export_staging(directory: &Path) {
    assert!(fs::read_dir(directory).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".heyfood-export.")
    }));
}
