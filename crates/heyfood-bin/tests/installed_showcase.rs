//! Installed-artifact qualification harness.
//!
//! This test is ignored during ordinary Cargo runs because it requires an
//! exact packaged archive and checksum manifest. Native CLI CI invokes it
//! explicitly against an archive produced by the same job. The spawned user
//! executable is always extracted from that archive; `CARGO_BIN_EXE_heyfood`
//! is intentionally forbidden here.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use heyfood_platform::{NativeAuthStore, WindowsCredentialStore};
use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

const TEST_PROMPT: &str = "Plan a synthetic dinner for installed-artifact qualification.";
const TEST_RESPONSE: &str = "Installed artifact first turn complete.";
const TEST_ACCOUNT: &str = "showcase-user";
const FULL_SCOPE: &str = "account:link account:delete knowledge:read menu:read menu:watch recommend:read recipes:read recipes:write claims:read_derived profile:read profile:write meals:read meals:write audio:transcribe health:read integrations:manage grocery:read grocery:write";
const SHOWCASE_STAGES: [&str; 12] = [
    "menu-watch.locate",
    "menu-watch.verify",
    "menu-watch.watch",
    "menu-watch.diff",
    "dinner-planner.plan",
    "dinner-planner.compare",
    "dinner-planner.shop",
    "dinner-planner.remember",
    "voice-meal-log.record",
    "voice-meal-log.transcribe",
    "voice-meal-log.review",
    "voice-meal-log.log",
];

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must follow the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "heyfood-installed-showcase-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create isolated showcase directory");
        Self(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(windows)]
struct WindowsCredentialCleanup(PathBuf);

#[cfg(windows)]
impl Drop for WindowsCredentialCleanup {
    fn drop(&mut self) {
        let _ = NativeAuthStore::open(&self.0).and_then(|store| store.delete());
        let _ = WindowsCredentialStore::open(&self.0).and_then(|store| store.delete());
    }
}

#[derive(Clone, Debug)]
struct RequestEvidence {
    method: String,
    path: String,
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

#[derive(Default)]
struct TerminalCapture {
    bytes: Mutex<Vec<u8>>,
    changed: Condvar,
}

impl TerminalCapture {
    fn append(&self, bytes: &[u8]) {
        self.bytes
            .lock()
            .expect("lock terminal capture")
            .extend_from_slice(bytes);
        self.changed.notify_all();
    }

    fn wait_for(&self, needle: &[u8], timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let mut bytes = self.bytes.lock().map_err(|_| "terminal capture poisoned")?;
        loop {
            if bytes.windows(needle.len()).any(|window| window == needle) {
                return Ok(());
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(format!(
                    "terminal output did not contain {:?}; observed {:?}",
                    String::from_utf8_lossy(needle),
                    String::from_utf8_lossy(&bytes)
                ));
            }
            let remaining = deadline.saturating_duration_since(now);
            let (next, _) = self
                .changed
                .wait_timeout(bytes, remaining.min(Duration::from_millis(100)))
                .map_err(|_| "terminal capture poisoned")?;
            bytes = next;
        }
    }

    fn snapshot(&self) -> Vec<u8> {
        self.bytes.lock().expect("lock terminal capture").clone()
    }
}

#[test]
#[ignore = "requires an exact packaged archive supplied by Native CLI CI"]
fn installed_archive_registration_and_first_turn() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("build showcase fixture runtime");
    runtime.block_on(run_installed_archive_registration_and_first_turn());
}

async fn run_installed_archive_registration_and_first_turn() {
    let archive = required_absolute_path("HEYFOOD_SHOWCASE_ARCHIVE");
    let manifest = required_absolute_path("HEYFOOD_SHOWCASE_MANIFEST");
    let evidence_directory = required_absolute_path("HEYFOOD_SHOWCASE_EVIDENCE_DIR");
    let expected_target = required_env("HEYFOOD_SHOWCASE_TARGET");
    let expected_version = required_env("HEYFOOD_SHOWCASE_VERSION");
    assert_semver(&expected_version);
    fs::create_dir_all(&evidence_directory).expect("create showcase evidence directory");

    let expected_archive_name = archive_name(&expected_version, &expected_target);
    assert_eq!(
        archive.file_name().and_then(|value| value.to_str()),
        Some(expected_archive_name.as_str()),
        "showcase archive name must identify the exact version and target"
    );
    let archive_digest = sha256_file(&archive);
    assert_manifest_digest(&manifest, &expected_archive_name, archive_digest.as_str());

    let extraction = TempRoot::new("extraction");
    let expected_binary_name = if expected_target.ends_with("-windows-msvc") {
        "heyfood.exe"
    } else {
        "heyfood"
    };
    assert_archive_policy(&archive, expected_binary_name);
    extract_archive(&archive, &extraction.0);
    let installed_binary = extraction.0.join(expected_binary_name);
    assert_single_installed_executable(&extraction.0, &installed_binary);
    let installed_binary = installed_binary
        .canonicalize()
        .expect("canonicalize installed showcase executable");
    assert!(
        installed_binary.starts_with(
            extraction
                .0
                .canonicalize()
                .expect("canonicalize extraction root")
        ),
        "installed executable must remain under the clean extraction root"
    );
    assert_not_repository_binary(&installed_binary);
    let executable_digest = sha256_file(&installed_binary);
    assert_installed_version(&installed_binary, &expected_version);

    let user = TempRoot::new("user");
    #[cfg(windows)]
    let _credential_cleanup = WindowsCredentialCleanup(user.0.clone());
    let (base_url, request_receiver, server) = start_fixture_service().await;
    let terminal = run_first_user_pty(&installed_binary, &user.0, &base_url).await;
    let requests = collect_request_evidence(request_receiver).await;
    server.await.expect("join showcase fixture service");
    assert_request_sequence(&requests);
    assert_terminal_contract(&terminal);

    let terminal_digest = sha256_bytes(&terminal);
    let terminal_path = evidence_directory.join("first-run.ansi");
    fs::write(&terminal_path, &terminal).expect("write privacy-safe ANSI evidence");
    let evidence = json!({
        "schema_version": 1,
        "qualification": "installed-artifact-foundation",
        "release_gate_complete": false,
        "archive": {
            "file_name": expected_archive_name,
            "sha256": archive_digest,
            "target": expected_target,
            "version": expected_version
        },
        "executable": {
            "file_name": expected_binary_name,
            "sha256": executable_digest,
            "source_checkout_binary": false
        },
        "environment": {
            "clean_user_state": true,
            "pty": true,
            "columns": 80,
            "rows": 30,
            "synthetic_backend": true
        },
        "foundation_stages": [
            {
                "id": "clean-user-registration",
                "status": "passed",
                "assertions": [
                    "device_registration_executed",
                    "account_bound_credentials_persisted",
                    "registration_handed_off_to_tui"
                ]
            },
            {
                "id": "first-streamed-turn",
                "status": "passed",
                "assertions": [
                    "prompt_dispatched_from_installed_tui",
                    "streamed_result_rendered",
                    "terminal_restored_on_exit"
                ]
            }
        ],
        "requests": requests.iter().map(|request| json!({
            "method": request.method,
            "path": request.path
        })).collect::<Vec<_>>(),
        "terminal": {
            "file_name": "first-run.ansi",
            "sha256": terminal_digest,
            "contains_credentials": false
        },
        "showcase": {
            "passed_stage_ids": [],
            "remaining_stage_ids": SHOWCASE_STAGES,
            "note": "Foundation evidence does not close any landing-page showcase stage."
        }
    });
    fs::write(
        evidence_directory.join("installed-foundation.json"),
        serde_json::to_vec_pretty(&evidence).expect("serialize installed evidence"),
    )
    .expect("write installed evidence");
}

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be configured"))
}

fn required_absolute_path(name: &str) -> PathBuf {
    let path = PathBuf::from(required_env(name));
    assert!(path.is_absolute(), "{name} must be absolute");
    path
}

fn assert_semver(version: &str) {
    let parts = version.split('.').collect::<Vec<_>>();
    assert_eq!(parts.len(), 3, "version must use MAJOR.MINOR.PATCH");
    assert!(
        parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())),
        "version must use MAJOR.MINOR.PATCH"
    );
}

fn archive_name(version: &str, target: &str) -> String {
    let suffix = match target {
        "aarch64-apple-darwin"
        | "x86_64-apple-darwin"
        | "aarch64-unknown-linux-gnu"
        | "x86_64-unknown-linux-gnu" => "tar.gz",
        "x86_64-pc-windows-msvc" => "zip",
        _ => panic!("unsupported showcase target: {target}"),
    };
    format!("heyfood-v{version}-{target}.{suffix}")
}

fn sha256_file(path: &Path) -> String {
    let bytes = fs::read(path).expect("read file for SHA-256");
    sha256_bytes(&bytes)
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn assert_manifest_digest(manifest: &Path, file_name: &str, digest: &str) {
    let contents = fs::read_to_string(manifest).expect("read showcase checksum manifest");
    let matches = contents
        .lines()
        .filter_map(|line| {
            let (observed_digest, observed_name) = line.split_once(char::is_whitespace)?;
            let observed_name = observed_name.trim_start_matches([' ', '*']);
            (observed_name == file_name).then_some(observed_digest)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        matches,
        [digest],
        "checksum manifest must bind the exact showcase archive once"
    );
}

fn assert_archive_policy(archive: &Path, expected_binary_name: &str) {
    let output = Command::new("tar")
        .args(["-tf"])
        .arg(archive)
        .stdin(Stdio::null())
        .output()
        .expect("list installed showcase archive");
    assert!(
        output.status.success(),
        "archive listing failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let listing = String::from_utf8(output.stdout).expect("archive listing must be UTF-8");
    let entries = listing
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(
        entries,
        [expected_binary_name],
        "archive must contain exactly the native executable at its root"
    );
}

fn extract_archive(archive: &Path, destination: &Path) {
    let output = Command::new("tar")
        .args(["-xf"])
        .arg(archive)
        .arg("-C")
        .arg(destination)
        .stdin(Stdio::null())
        .output()
        .expect("extract installed showcase archive");
    assert!(
        output.status.success(),
        "archive extraction failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_single_installed_executable(root: &Path, expected: &Path) {
    let entries = fs::read_dir(root)
        .expect("read extraction root")
        .map(|entry| entry.expect("read extraction entry").path())
        .collect::<Vec<_>>();
    assert_eq!(entries, [expected], "extraction must produce one entry");
    let metadata = fs::symlink_metadata(expected).expect("inspect installed executable");
    assert!(
        metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
        "installed executable must be a regular file, not a link"
    );
}

fn assert_not_repository_binary(installed_binary: &Path) {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .expect("canonicalize package source");
    let workspace = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("resolve workspace source");
    assert!(
        !installed_binary.starts_with(workspace.join("target")),
        "source-checkout Cargo binaries are forbidden"
    );
    assert!(
        !installed_binary.starts_with(workspace),
        "installed executable must not reside in the source checkout"
    );
}

fn assert_installed_version(binary: &Path, expected_version: &str) {
    let output = Command::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .expect("execute installed binary version");
    assert!(output.status.success(), "installed binary version failed");
    assert_eq!(
        String::from_utf8(output.stdout)
            .expect("installed version output is UTF-8")
            .trim(),
        format!("heyfood {expected_version}")
    );
    assert!(output.stderr.is_empty());
}

async fn start_fixture_service() -> (
    String,
    mpsc::Receiver<RequestEvidence>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind installed showcase fixture service");
    let base_url = format!(
        "http://{}",
        listener
            .local_addr()
            .expect("resolve showcase fixture address")
    );
    let verification_uri = format!("{base_url}/authorize?flow=device");
    let (request_sender, request_receiver) = mpsc::channel(16);
    let server = tokio::spawn(async move {
        for _ in 0..8 {
            let (mut socket, _) = listener.accept().await.expect("accept showcase request");
            let request = read_http_request(&mut socket).await;
            request_sender
                .send(RequestEvidence {
                    method: request.method.clone(),
                    path: request.path.clone(),
                })
                .await
                .expect("record showcase request");
            respond_to_showcase_request(&mut socket, request, &verification_uri).await;
        }
    });
    (base_url, request_receiver, server)
}

async fn read_http_request(socket: &mut TcpStream) -> HttpRequest {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 2048];
        let count = socket.read(&mut chunk).await.expect("read request bytes");
        assert_ne!(count, 0, "request ended before headers completed");
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break index + 4;
        }
        assert!(bytes.len() <= 64 * 1024, "request headers are too large");
    };
    let headers =
        String::from_utf8(bytes[..header_end].to_vec()).expect("request headers are UTF-8");
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().expect("valid content length"))
            })
        })
        .unwrap_or_default();
    assert!(content_length <= 64 * 1024, "request body is too large");
    while bytes.len() < header_end + content_length {
        let mut chunk = vec![0_u8; header_end + content_length - bytes.len()];
        let count = socket.read(&mut chunk).await.expect("read request body");
        assert_ne!(count, 0, "request ended before body completed");
        bytes.extend_from_slice(&chunk[..count]);
    }
    let request_line = headers.lines().next().expect("request line");
    let mut fields = request_line.split_whitespace();
    let method = fields.next().expect("request method").to_owned();
    let path = fields
        .next()
        .expect("request path")
        .split('?')
        .next()
        .expect("request route")
        .to_owned();
    HttpRequest {
        method,
        path,
        body: bytes[header_end..header_end + content_length].to_vec(),
    }
}

async fn respond_to_showcase_request(
    socket: &mut TcpStream,
    request: HttpRequest,
    verification_uri: &str,
) {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/v1/auth/capabilities") => {
            respond_json(
                socket,
                json!({
                    "schema_version": 1,
                    "self_registration": {
                        "status": "available",
                        "regions": ["US"],
                        "identity_methods": ["sms", "email"]
                    },
                    "authorization": {
                        "loopback_pkce": true,
                        "device_code": true,
                        "identity_methods": ["sms", "email"]
                    },
                    "profile_readiness": true,
                    "application_capabilities": {
                        "grocery": "v1",
                        "health": "v1",
                        "menu_watch": "v1"
                    }
                }),
            )
            .await;
        }
        ("POST", "/v1/channel/oauth/device/authorize") => {
            let body: Value =
                serde_json::from_slice(&request.body).expect("decode device authorization");
            assert_eq!(body["intent"], "create_account");
            assert_eq!(body["scope"], FULL_SCOPE);
            respond_json(
                socket,
                json!({
                    "device_code": "hf_dc_showcase_01234567890123456789",
                    "user_code": "SHOW-CASE",
                    "verification_uri": verification_uri,
                    "verification_uri_complete": null,
                    "expires_in": 600,
                    "interval": 1
                }),
            )
            .await;
        }
        ("POST", "/v1/channel/oauth/device/token") => {
            respond_json(
                socket,
                json!({
                    "access_token": "hf_ct_showcase",
                    "token_type": "bearer",
                    "refresh_token": "hf_cr_showcase",
                    "expires_in": 3600,
                    "scope": FULL_SCOPE
                }),
            )
            .await;
        }
        ("POST", "/v1/channel/oauth/cli/session") => {
            let body: Value =
                serde_json::from_slice(&request.body).expect("decode CLI session request");
            let device_id = body["device_id"]
                .as_str()
                .expect("CLI session request device ID");
            respond_json(
                socket,
                json!({
                    "user_id": TEST_ACCOUNT,
                    "device_id": device_id,
                    "session_id": "showcase-session",
                    "access_token": "hf_at_showcase",
                    "refresh_token": "hf_rt_showcase",
                    "access_expires_at": "2999-01-01T00:00:00Z",
                    "refresh_expires_at": "2999-02-01T00:00:00Z",
                    "scopes": FULL_SCOPE.split_whitespace().collect::<Vec<_>>(),
                    "is_anonymous": false
                }),
            )
            .await;
        }
        ("GET", "/v1/channel/tools/profile/readiness") => {
            respond_json(
                socket,
                json!({
                    "schema_version": 1,
                    "status": "ready",
                    "member_id": "_self",
                    "has_profile_sync_consent": true,
                    "profile_version": 1
                }),
            )
            .await;
        }
        ("GET", "/v1/profile/consent") => {
            respond_json(
                socket,
                json!({
                    "schema_version": 1,
                    "has_consent": true,
                    "consent_version": 1
                }),
            )
            .await;
        }
        ("GET", "/v1/profile/sync") => {
            respond_json(
                socket,
                json!({
                    "schema_version": 1,
                    "member_id": "_self",
                    "profile_version": 1,
                    "profile_data": {
                        "dietary_preferences": ["vegetarian"],
                        "allergens": []
                    }
                }),
            )
            .await;
        }
        ("POST", "/v1/agent/converse") => {
            let body: Value =
                serde_json::from_slice(&request.body).expect("decode installed first turn");
            assert_eq!(body["query"], TEST_PROMPT);
            assert!(body.get("confirm").is_none());
            respond_sse(socket).await;
        }
        _ => panic!(
            "unexpected installed showcase request: {} {}",
            request.method, request.path
        ),
    }
}

async fn respond_json(socket: &mut TcpStream, body: Value) {
    let body = serde_json::to_vec(&body).expect("encode fixture JSON");
    respond(socket, "application/json", &body).await;
}

async fn respond_sse(socket: &mut TcpStream) {
    let body = format!(
        "event: partial\ndata: {{\"text\":\"{TEST_RESPONSE}\"}}\n\nevent: result\ndata: {{\"message\":\"{TEST_RESPONSE}\",\"conversation_id\":\"showcase-conversation\"}}\n\n"
    );
    respond(socket, "text/event-stream", body.as_bytes()).await;
}

async fn respond(socket: &mut TcpStream, content_type: &str, body: &[u8]) {
    socket
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .expect("write fixture response headers");
    socket
        .write_all(body)
        .await
        .expect("write fixture response body");
    socket.shutdown().await.expect("close fixture response");
}

async fn run_first_user_pty(installed_binary: &Path, user_root: &Path, base_url: &str) -> Vec<u8> {
    let installed_binary = installed_binary.to_owned();
    let user_root = user_root.to_owned();
    let base_url = base_url.to_owned();
    tokio::task::spawn_blocking(move || {
        run_first_user_pty_blocking(&installed_binary, &user_root, &base_url)
    })
    .await
    .expect("join installed PTY driver")
}

fn run_first_user_pty_blocking(
    installed_binary: &Path,
    user_root: &Path,
    base_url: &str,
) -> Vec<u8> {
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 30,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open installed showcase PTY");
    let mut command = CommandBuilder::new(installed_binary);
    command.args(["register", "--device", "--no-browser", "--timeout", "10"]);
    for name in [
        "HEYFOOD_API_KEY",
        "HEYFOOD_API_URL",
        "HEYFOOD_CREDENTIAL_STORE",
        "HEYFOOD_STATE_DIR",
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "ALL_PROXY",
        "NO_COLOR",
    ] {
        command.env_remove(name);
    }
    command.env("HEYFOOD_API_URL", base_url);
    command.env("HEYFOOD_API_KEY", "showcase-api-key");
    command.env("HEYFOOD_STATE_DIR", user_root);
    #[cfg(not(windows))]
    command.env("HEYFOOD_CREDENTIAL_STORE", "file");
    #[cfg(windows)]
    command.env("HEYFOOD_CREDENTIAL_STORE", "native");
    command.env("HOME", user_root);
    command.env("XDG_CONFIG_HOME", user_root);
    command.env("XDG_DATA_HOME", user_root.join("data"));
    command.env("XDG_CACHE_HOME", user_root.join("cache"));
    command.env("USERPROFILE", user_root);
    command.env("APPDATA", user_root.join("appdata"));
    command.env("LOCALAPPDATA", user_root.join("local-appdata"));
    command.env("NO_PROXY", "127.0.0.1,localhost");
    command.env("TERM", "xterm-256color");

    let mut child = pair
        .slave
        .spawn_command(command)
        .expect("spawn installed showcase executable");
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().expect("clone PTY reader");
    let writer = Arc::new(Mutex::new(
        pair.master.take_writer().expect("take PTY writer"),
    ));
    let capture = Arc::new(TerminalCapture::default());
    let reader_capture = Arc::clone(&capture);
    let cursor_writer = Arc::clone(&writer);
    let reader_task = std::thread::spawn(move || {
        let mut cursor_query_replied = false;
        loop {
            let mut chunk = [0_u8; 4096];
            let count = reader.read(&mut chunk).expect("read installed PTY");
            if count == 0 {
                break;
            }
            reader_capture.append(&chunk[..count]);
            if !cursor_query_replied
                && reader_capture
                    .snapshot()
                    .windows(b"\x1b[6n".len())
                    .any(|window| window == b"\x1b[6n")
            {
                let mut writer = cursor_writer.lock().expect("lock cursor reply");
                writer
                    .write_all(b"\x1b[1;1R")
                    .expect("reply to terminal cursor query");
                writer.flush().expect("flush cursor reply");
                cursor_query_replied = true;
            }
        }
    });

    capture
        .wait_for(b"hey.food", Duration::from_secs(20))
        .unwrap_or_else(|message| terminate_and_panic(&mut *child, message));
    {
        let mut writer = writer.lock().expect("lock installed PTY input");
        writer
            .write_all(format!("{TEST_PROMPT}\r").as_bytes())
            .expect("submit installed first turn");
        writer.flush().expect("flush installed first turn");
    }
    capture
        .wait_for(b"complete.", Duration::from_secs(20))
        .unwrap_or_else(|message| terminate_and_panic(&mut *child, message));
    {
        let mut writer = writer.lock().expect("lock installed PTY exit");
        writer.write_all(&[4]).expect("send Ctrl+D");
        writer.flush().expect("flush Ctrl+D");
    }
    let status = wait_for_child(&mut *child, Duration::from_secs(15));
    drop(pair.master);
    let _ = reader_task.join();
    assert!(
        status.success(),
        "installed TUI exited unsuccessfully: {status:?}"
    );
    capture.snapshot()
}

fn terminate_and_panic(child: &mut dyn Child, message: String) -> ! {
    let _ = child.kill();
    let _ = child.wait();
    panic!("{message}")
}

fn wait_for_child(child: &mut dyn Child, timeout: Duration) -> portable_pty::ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("poll installed PTY child") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let status = child.wait().expect("wait after installed PTY timeout");
            panic!("installed TUI did not exit within {timeout:?}: {status:?}");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

async fn collect_request_evidence(
    mut receiver: mpsc::Receiver<RequestEvidence>,
) -> Vec<RequestEvidence> {
    let mut requests = Vec::new();
    while let Some(request) = receiver.recv().await {
        requests.push(request);
    }
    requests
}

fn assert_request_sequence(requests: &[RequestEvidence]) {
    let observed = requests
        .iter()
        .map(|request| (request.method.as_str(), request.path.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        [
            ("GET", "/v1/auth/capabilities"),
            ("POST", "/v1/channel/oauth/device/authorize"),
            ("POST", "/v1/channel/oauth/device/token"),
            ("POST", "/v1/channel/oauth/cli/session"),
            ("GET", "/v1/channel/tools/profile/readiness"),
            ("GET", "/v1/profile/consent"),
            ("GET", "/v1/profile/sync"),
            ("POST", "/v1/agent/converse"),
        ],
        "installed first-user request sequence changed"
    );
}

fn assert_terminal_contract(terminal: &[u8]) {
    for expected in [
        b"Open this URL to continue:".as_slice(),
        b"Approval code: SHOW-CASE".as_slice(),
        b"Your hello.food account is connected.".as_slice(),
        b"hey.food".as_slice(),
        b"\x1b[?1049h".as_slice(),
        b"\x1b[?1049l".as_slice(),
    ] {
        assert!(
            terminal
                .windows(expected.len())
                .any(|window| window == expected),
            "installed terminal evidence omitted {:?}",
            String::from_utf8_lossy(expected)
        );
    }
    assert_semantic_terminal_text(terminal, TEST_PROMPT);
    assert_semantic_terminal_text(terminal, TEST_RESPONSE);
    for forbidden in [
        b"hf_dc_showcase".as_slice(),
        b"hf_ct_showcase".as_slice(),
        b"hf_cr_showcase".as_slice(),
        b"hf_at_showcase".as_slice(),
        b"hf_rt_showcase".as_slice(),
        b"showcase-api-key".as_slice(),
    ] {
        assert!(
            !terminal
                .windows(forbidden.len())
                .any(|window| window == forbidden),
            "terminal evidence contains a fixture credential"
        );
    }
}

fn assert_semantic_terminal_text(terminal: &[u8], expected: &str) {
    let observed = compact_terminal_text(&strip_ansi_sequences(terminal));
    let expected = compact_terminal_text(expected);
    assert!(
        observed.contains(&expected),
        "installed terminal evidence omitted semantic text {expected:?}; observed {observed:?}"
    );
}

fn compact_terminal_text(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn strip_ansi_sequences(value: &[u8]) -> String {
    let mut plain = Vec::with_capacity(value.len());
    let mut index = 0;
    while index < value.len() {
        if value[index] != 0x1b {
            plain.push(value[index]);
            index += 1;
            continue;
        }
        index += 1;
        match value.get(index).copied() {
            Some(b'[') => {
                index += 1;
                while index < value.len() {
                    let byte = value[index];
                    index += 1;
                    if (0x40..=0x7e).contains(&byte) {
                        break;
                    }
                }
            }
            Some(b']') => {
                index += 1;
                while index < value.len() {
                    if value[index] == 0x07 {
                        index += 1;
                        break;
                    }
                    if value[index] == 0x1b && value.get(index + 1) == Some(&b'\\') {
                        index += 2;
                        break;
                    }
                    index += 1;
                }
            }
            Some(_) => index += 1,
            None => {}
        }
    }
    String::from_utf8_lossy(&plain).into_owned()
}

#[test]
fn installed_harness_inventory_matches_showcase_contract() {
    let contract: Value = serde_json::from_str(include_str!(
        "../../../tests/showcase/showcase-contract.v1.json"
    ))
    .expect("decode showcase contract");
    let observed = contract["journeys"]
        .as_array()
        .expect("showcase journeys")
        .iter()
        .flat_map(|journey| {
            let journey_id = journey["id"].as_str().expect("journey ID");
            journey["stages"]
                .as_array()
                .expect("journey stages")
                .iter()
                .map(move |stage| {
                    format!("{journey_id}.{}", stage["id"].as_str().expect("stage ID"))
                })
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        observed,
        SHOWCASE_STAGES
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
    );
}
