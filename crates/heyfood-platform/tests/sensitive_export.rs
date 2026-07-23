use std::fs;
use std::path::{Path, PathBuf};
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
