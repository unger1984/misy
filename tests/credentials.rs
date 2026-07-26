use misy::{CredentialStore, MisyPaths, ProviderId};
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

fn test_root(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("misy-{name}-{}-{unique}", std::process::id()))
}

#[test]
fn credential_store_persists_opaque_provider_json_with_private_permissions() {
    let root = test_root("credentials");
    let paths = MisyPaths::from_root(&root);
    let store = CredentialStore::new(paths.clone());
    let provider = ProviderId::new("codex-subscription");
    let credentials = json!({"access_token": "opaque-token", "expires_at": 42});

    assert_eq!(store.load(&provider).unwrap(), None);
    store.save(&provider, credentials.clone()).unwrap();

    assert_eq!(store.load(&provider).unwrap(), Some(credentials));
    assert!(!root.join("credentials.json.tmp").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(paths.credentials_file())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn credential_store_rejects_an_unsupported_format_version() {
    let root = test_root("credentials-version");
    let paths = MisyPaths::from_root(&root);
    fs::create_dir_all(&root).unwrap();
    fs::write(paths.credentials_file(), r#"{"version":2,"providers":{}}"#).unwrap();

    let error = CredentialStore::new(paths)
        .load(&ProviderId::new("codex"))
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("unsupported credentials version 2")
    );
    fs::remove_dir_all(root).unwrap();
}
