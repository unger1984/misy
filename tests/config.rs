use misy::{Config, ConfigStore, MisyPaths, ModelId, ModelRef, ProviderId};
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
fn config_store_uses_injected_root_and_round_trips_versioned_default_model() {
    let root = test_root("config");
    let paths = MisyPaths::from_root(&root);
    let store = ConfigStore::new(paths.clone());
    let config = Config::with_default_model(ModelRef::new(
        ProviderId::new("codex-subscription"),
        ModelId::new("gpt-5"),
    ));

    assert_eq!(store.load().unwrap(), Config::default());
    store.save(&config).unwrap();

    assert_eq!(store.load().unwrap(), config);
    let on_disk = fs::read_to_string(paths.config_file()).unwrap();
    assert!(on_disk.contains("version = 1"));
    assert!(!root.join("config.toml.tmp").exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn config_store_rejects_an_unsupported_format_version() {
    let root = test_root("config-version");
    let paths = MisyPaths::from_root(&root);
    fs::create_dir_all(&root).unwrap();
    fs::write(paths.config_file(), "version = 2\n").unwrap();

    let error = ConfigStore::new(paths).load().unwrap_err();

    assert!(error.to_string().contains("unsupported config version 2"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn config_store_refuses_to_write_an_unsupported_format_version() {
    let root = test_root("config-write-version");
    let store = ConfigStore::new(MisyPaths::from_root(&root));
    let unsupported = Config {
        version: 2,
        default_model: None,
    };

    let error = store.save(&unsupported).unwrap_err();

    assert!(error.to_string().contains("unsupported config version 2"));
    assert!(!root.join("config.toml").exists());
}
