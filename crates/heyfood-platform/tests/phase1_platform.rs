use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use heyfood_core::{AccountId, ClientConfig, ConfigRevision, NetworkPolicy, ServiceUrl};
use heyfood_platform::{
    NativeConfigStore, NativePaths, ProxyConfiguration, TerminalKind, TlsRootPolicy,
    TtyCapabilities,
};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "heyfood-phase1-platform-{name}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn config() -> ClientConfig {
    ClientConfig {
        active_context: "production".into(),
        api_url: ServiceUrl::parse("https://api.hello.food", NetworkPolicy::HTTPS_ONLY).unwrap(),
        auth_url: ServiceUrl::parse(
            "https://auth.hello.food/authorize",
            NetworkPolicy::HTTPS_ONLY,
        )
        .unwrap(),
        revision: ConfigRevision::new(1),
    }
}

#[test]
fn native_paths_separate_state_and_make_account_paths_opaque() {
    let root = TempRoot::new("paths");
    let paths = NativePaths::under(&root.0);
    assert_eq!(paths.config_dir(), root.0.as_path());
    assert_ne!(paths.data_dir(), paths.cache_dir());
    assert_ne!(paths.runtime_dir(), paths.data_dir());

    let first = paths.account_state_dir(&AccountId::parse("account-a").unwrap());
    let second = paths.account_state_dir(&AccountId::parse("account-b").unwrap());
    assert_ne!(first, second);
    assert!(!first.to_string_lossy().contains("account-a"));
}

#[test]
fn native_config_claims_legacy_unbound_state_once_and_rejects_other_accounts() {
    let root = TempRoot::new("account-binding");
    NativeConfigStore::open(&root.0, config(), NetworkPolicy::HTTPS_ONLY).unwrap();
    NativeConfigStore::open_account_bound(
        &root.0,
        AccountId::parse("account-a").unwrap(),
        config(),
        NetworkPolicy::HTTPS_ONLY,
    )
    .unwrap();
    let document = std::fs::read_to_string(root.0.join("config.native")).unwrap();
    assert!(document.starts_with("schema=2\naccount="));

    NativeConfigStore::open_account_bound(
        &root.0,
        AccountId::parse("account-a").unwrap(),
        config(),
        NetworkPolicy::HTTPS_ONLY,
    )
    .unwrap();
    let error = NativeConfigStore::open_account_bound(
        &root.0,
        AccountId::parse("account-b").unwrap(),
        config(),
        NetworkPolicy::HTTPS_ONLY,
    )
    .unwrap_err();
    assert_eq!(error.code, "config_account_conflict");
}

#[test]
fn proxy_and_custom_ca_inputs_are_validated_without_a_tls_bypass() {
    let proxy = ProxyConfiguration::from_values(
        Some("http://proxy.example:8080"),
        Some("https://secure-proxy.example"),
        Some("localhost,.hello.food"),
    )
    .unwrap();
    assert_eq!(
        proxy.http_proxy.unwrap().as_url().host_str(),
        Some("proxy.example")
    );
    assert!(
        ProxyConfiguration::from_values(Some("http://user:secret@proxy.example"), None, None)
            .is_err()
    );

    let root = TempRoot::new("ca");
    let bundle = root.0.join("enterprise.pem");
    std::fs::write(&bundle, "-----BEGIN CERTIFICATE-----\nfixture\n").unwrap();
    assert_eq!(
        TlsRootPolicy::with_custom_ca(&bundle)
            .unwrap()
            .custom_ca_bundle,
        Some(bundle)
    );
    assert!(TlsRootPolicy::with_custom_ca(&root.0).is_err());
}

#[test]
fn tty_policy_uses_classic_or_device_flow_for_redirected_and_remote_sessions() {
    let local = TtyCapabilities {
        stdin: true,
        stdout: true,
        stderr: true,
        remote_session: false,
    };
    assert_eq!(local.kind(), TerminalKind::Interactive);
    assert!(local.supports_loopback_browser_flow());

    let remote = TtyCapabilities {
        remote_session: true,
        ..local
    };
    assert_eq!(remote.kind(), TerminalKind::RemoteInteractive);
    assert!(!remote.supports_loopback_browser_flow());

    let redirected = TtyCapabilities {
        stdout: false,
        ..local
    };
    assert_eq!(redirected.kind(), TerminalKind::Redirected);
    assert!(!redirected.supports_loopback_browser_flow());
}
