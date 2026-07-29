//! Manifest discovery and validation tests.

use misy_core::{PROVIDER_PROTOCOL_VERSION, ProviderCatalog, ProviderDiscoveryError, ProviderId};
use std::{fs, path::Path};

fn write_manifest(root: &Path, id: &str, protocol_version: u32) {
    let package = root.join(id);
    fs::create_dir_all(&package).expect("package directory");
    fs::write(
        package.join("misy-plugin.json"),
        format!(
            r#"{{
  "id": "{id}",
  "display_name": "{id} fixture",
  "version": "1.2.3",
  "kind": "provider",
  "protocol_version": {protocol_version},
  "description": "A fixture provider",
  "author": "Misy",
  "homepage": "https://example.test/plugin",
  "repository": "https://example.test/repository",
  "license": "MIT",
  "command": "fixture-provider",
  "args": [],
  "auth_methods": [{{"id": "oauth", "display_name": "Fixture OAuth"}}]
}}"#
        ),
    )
    .expect("manifest");
}

#[test]
fn discovery_reads_self_contained_provider_packages() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let installed = temporary.path().join("installed");
    write_manifest(&bundled, "bundled-provider", PROVIDER_PROTOCOL_VERSION);
    write_manifest(&installed, "installed-provider", PROVIDER_PROTOCOL_VERSION);

    let catalog = ProviderCatalog::discover(&bundled, &installed).expect("valid packages");

    assert_eq!(catalog.len(), 2);
    assert_eq!(
        catalog
            .get(&ProviderId::new("bundled-provider"))
            .expect("bundled provider")
            .manifest()
            .capabilities,
        Default::default()
    );
    assert!(
        catalog
            .get(&ProviderId::new("installed-provider"))
            .is_some()
    );
}

#[test]
fn discovery_reads_versioned_optional_capabilities() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let installed = temporary.path().join("installed");
    write_manifest(&bundled, "usage-provider", PROVIDER_PROTOCOL_VERSION);
    let manifest = bundled.join("usage-provider/misy-plugin.json");
    let contents = fs::read_to_string(&manifest).expect("manifest");
    fs::write(
        &manifest,
        contents.replace(
            "  \"description\": \"A fixture provider\",\n",
            concat!(
                "  \"description\": \"A fixture provider\",\n",
                "  \"capabilities\": {\"usage\": {\"version\": 1}},\n",
            ),
        ),
    )
    .expect("usage capability");

    let catalog = ProviderCatalog::discover(&bundled, &installed).expect("valid capability");
    let package = catalog
        .get(&ProviderId::new("usage-provider"))
        .expect("usage provider");

    assert!(package.manifest().supports_capability("usage", 1));
    assert!(!package.manifest().supports_capability("usage", 2));
}

#[test]
fn discovery_rejects_duplicate_and_incompatible_packages() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let installed = temporary.path().join("installed");
    write_manifest(&bundled, "duplicate", PROVIDER_PROTOCOL_VERSION);
    write_manifest(&installed, "duplicate", PROVIDER_PROTOCOL_VERSION);

    assert!(matches!(
        ProviderCatalog::discover(&bundled, &installed),
        Err(ProviderDiscoveryError::DuplicateProvider(_))
    ));

    fs::remove_dir_all(&installed).expect("remove duplicate");
    write_manifest(&installed, "incompatible", PROVIDER_PROTOCOL_VERSION + 1);
    assert!(matches!(
        ProviderCatalog::discover(&bundled, &installed),
        Err(ProviderDiscoveryError::UnsupportedProtocol { .. })
    ));
}

#[test]
fn discovery_rejects_manifest_without_required_metadata() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let installed = temporary.path().join("installed");
    write_manifest(&bundled, "metadata", PROVIDER_PROTOCOL_VERSION);
    let manifest = bundled.join("metadata/misy-plugin.json");
    let contents = fs::read_to_string(&manifest).expect("manifest");
    fs::write(
        &manifest,
        contents.replace(
            "\"description\": \"A fixture provider\"",
            "\"description\": \"\"",
        ),
    )
    .expect("empty metadata");

    assert!(matches!(
        ProviderCatalog::discover(&bundled, &installed),
        Err(ProviderDiscoveryError::InvalidManifestValue { .. })
    ));
}

#[test]
fn discovery_requires_display_name_and_authentication_methods() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let installed = temporary.path().join("installed");
    write_manifest(&bundled, "missing-display-name", PROVIDER_PROTOCOL_VERSION);
    let manifest = bundled.join("missing-display-name/misy-plugin.json");
    let contents = fs::read_to_string(&manifest).expect("manifest");
    fs::write(
        &manifest,
        contents.replace(
            "  \"display_name\": \"missing-display-name fixture\",\n",
            "",
        ),
    )
    .expect("missing display name");

    assert!(matches!(
        ProviderCatalog::discover(&bundled, &installed),
        Err(ProviderDiscoveryError::InvalidManifest { .. })
    ));

    fs::remove_dir_all(bundled.join("missing-display-name")).expect("remove invalid manifest");
    write_manifest(&bundled, "missing-auth-methods", PROVIDER_PROTOCOL_VERSION);
    let manifest = bundled.join("missing-auth-methods/misy-plugin.json");
    let contents = fs::read_to_string(&manifest).expect("manifest");
    fs::write(
        &manifest,
        contents.replace(
            "  \"auth_methods\": [{\"id\": \"oauth\", \"display_name\": \"Fixture OAuth\"}]\n",
            "  \"auth_methods\": []\n",
        ),
    )
    .expect("empty auth methods");

    assert!(matches!(
        ProviderCatalog::discover(&bundled, &installed),
        Err(ProviderDiscoveryError::InvalidManifestValue { .. })
    ));
}

#[test]
fn discovery_rejects_manifest_without_args() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let installed = temporary.path().join("installed");
    write_manifest(&bundled, "missing-args", PROVIDER_PROTOCOL_VERSION);
    let manifest = bundled.join("missing-args/misy-plugin.json");
    let contents = fs::read_to_string(&manifest).expect("manifest");
    fs::write(
        &manifest,
        contents.replace(
            "  \"command\": \"fixture-provider\",\n  \"args\": [],\n",
            "  \"command\": \"fixture-provider\",\n",
        ),
    )
    .expect("missing args");

    assert!(matches!(
        ProviderCatalog::discover(&bundled, &installed),
        Err(ProviderDiscoveryError::InvalidManifest { .. })
    ));
}
