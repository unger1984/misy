//! Configuration-store integration tests.

use misy_core::{Config, ConfigStore, MisyPaths, ModelId, ModelRef, ProviderId};
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
fn config_store_uses_injected_root_and_round_trips_versioned_default_model() {
    let root = test_root("config");
    let paths = MisyPaths::from_root(&root);
    let store = ConfigStore::new(paths.clone());
    let config = Config {
        default_model: Some(ModelRef::new(
            ProviderId::new("openai"),
            ModelId::new("gpt-5"),
        )),
        ..Config::default()
    };

    assert_eq!(store.load().expect("load defaults"), Config::default());
    store.save(&config).expect("save config");

    assert_eq!(store.load().expect("load config"), config);
    let on_disk = fs::read_to_string(paths.config_file()).expect("read config file");
    assert!(on_disk.contains("version = 1"));
    assert!(!root.join("config.toml.tmp").exists());

    fs::remove_dir_all(root).expect("remove config test directory");
}

#[test]
fn config_store_rejects_an_unsupported_format_version() {
    let root = test_root("config-version");
    let paths = MisyPaths::from_root(&root);
    fs::create_dir_all(&root).expect("create config test directory");
    fs::write(paths.config_file(), "version = 2\n").expect("write unsupported config");

    let error = ConfigStore::new(paths)
        .load()
        .expect_err("unsupported config must fail");

    assert!(error.to_string().contains("unsupported config version 2"));
    fs::remove_dir_all(root).expect("remove config test directory");
}

#[test]
fn config_store_refuses_to_write_an_unsupported_format_version() {
    let root = test_root("config-write-version");
    let store = ConfigStore::new(MisyPaths::from_root(&root));
    let unsupported = Config {
        version: 2,
        default_model: None,
    };

    let error = store
        .save(&unsupported)
        .expect_err("unsupported config must not save");

    assert!(error.to_string().contains("unsupported config version 2"));
    assert!(!root.join("config.toml").exists());
}

#[cfg(unix)]
#[test]
fn config_store_keeps_the_data_directory_private() {
    use std::os::unix::fs::PermissionsExt;

    let root = test_root("config-dir-mode");
    fs::create_dir_all(&root).expect("create config test directory");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755))
        .expect("relax test directory permissions");
    let store = ConfigStore::new(MisyPaths::from_root(&root));

    store.save(&Config::default()).expect("save config");

    assert_eq!(
        fs::metadata(&root)
            .expect("read data directory metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    fs::remove_dir_all(root).expect("remove config test directory");
}

#[test]
fn config_store_replaces_an_existing_config_file() {
    let root = test_root("config-overwrite");
    let paths = MisyPaths::from_root(&root);
    let store = ConfigStore::new(paths.clone());
    let first = Config {
        default_model: Some(ModelRef::new(
            ProviderId::new("first-provider"),
            ModelId::new("first-model"),
        )),
        ..Config::default()
    };
    let replacement = Config {
        default_model: Some(ModelRef::new(
            ProviderId::new("replacement-provider"),
            ModelId::new("replacement-model"),
        )),
        ..Config::default()
    };

    store.save(&first).expect("save first config");
    store.save(&replacement).expect("replace config");

    assert_eq!(store.load().expect("load replacement"), replacement);
    assert!(
        !fs::read_to_string(paths.config_file())
            .expect("read replacement config")
            .contains("first-provider")
    );
    fs::remove_dir_all(root).expect("remove config test directory");
}
