//! Credential-store integration tests.

use misy_core::{CredentialStore, MisyPaths, ProviderId};
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

fn test_root(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time must be after the Unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("misy-{name}-{}-{unique}", std::process::id()))
}

#[test]
fn credential_store_persists_opaque_provider_json_with_private_permissions() {
    let root = test_root("credentials");
    let paths = MisyPaths::from_root(&root);
    let store = CredentialStore::new(paths.clone());
    let provider = ProviderId::new("openai");
    let credentials = json!({"access_token": "opaque-token", "expires_at": 42});

    assert_eq!(store.load(&provider).expect("load empty credentials"), None);
    store
        .save(&provider, credentials.clone())
        .expect("save credentials");

    assert_eq!(
        store.load(&provider).expect("load credentials"),
        Some(credentials)
    );
    let contents = fs::read(paths.credentials_file()).expect("read credentials file");
    let document: serde_json::Value =
        serde_json::from_slice(&contents).expect("credentials document");
    assert!(
        document["providers"].get("openai").is_some(),
        "provider id must stay a plain string key: {document}"
    );
    assert!(!root.join("credentials.json.tmp").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(paths.credentials_file())
                .expect("read credential file metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    fs::remove_dir_all(root).expect("remove credential test directory");
}

#[test]
fn credential_store_rejects_an_unsupported_format_version() {
    let root = test_root("credentials-version");
    let paths = MisyPaths::from_root(&root);
    fs::create_dir_all(&root).expect("create credential test directory");
    fs::write(paths.credentials_file(), r#"{"version":2,"providers":{}}"#)
        .expect("write unsupported credentials");

    let error = CredentialStore::new(paths)
        .load(&ProviderId::new("codex"))
        .expect_err("unsupported credentials must fail");

    assert!(
        error
            .to_string()
            .contains("unsupported credentials version 2")
    );
    fs::remove_dir_all(root).expect("remove credential test directory");
}

#[test]
fn credential_store_replaces_existing_provider_credentials() {
    let root = test_root("credentials-overwrite");
    let paths = MisyPaths::from_root(&root);
    let store = CredentialStore::new(paths.clone());
    let provider = ProviderId::new("openai");

    store
        .save(&provider, json!({"access_token": "old-token"}))
        .expect("save initial credentials");
    store
        .save(&provider, json!({"access_token": "replacement-token"}))
        .expect("replace credentials");

    assert_eq!(
        store.load(&provider).expect("load replacement credentials"),
        Some(json!({"access_token": "replacement-token"}))
    );
    assert!(
        !fs::read_to_string(paths.credentials_file())
            .expect("read replacement credentials")
            .contains("old-token")
    );
    fs::remove_dir_all(root).expect("remove credential test directory");
}
