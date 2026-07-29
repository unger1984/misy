//! Provider-host integration tests.

use misy_core::{ProviderCatalog, ProviderHost};
use std::{fs, path::Path};
use tokio::runtime::Handle;

#[path = "provider_host/discovery.rs"]
mod discovery;
#[path = "provider_host/lifecycle.rs"]
mod lifecycle;
#[path = "provider_host/transport.rs"]
mod transport;

// Tests run inside a Tokio runtime, so the host reuses the caller's handle; production hosts get
// the core-owned runtime handle instead.
fn host_for(catalog: ProviderCatalog) -> ProviderHost {
    ProviderHost::with_handle(catalog, Handle::current())
}

fn write_fixture_manifest(root: &Path, id: &str, fixture: &Path, log_file: &Path) {
    let package = root.join(id);
    fs::create_dir_all(&package).expect("package directory");
    let fixture = fixture.to_string_lossy();
    let log_file = log_file.to_string_lossy();
    fs::write(
        package.join("misy-plugin.json"),
        format!(
            r#"{{
  "id": "{id}",
  "display_name": "{id} fixture",
  "version": "1.0.0",
  "kind": "provider",
  "protocol_version": 2,
  "description": "Language-neutral fixture",
  "author": "Misy",
  "homepage": "https://example.test/plugin",
  "repository": "https://example.test/repository",
  "license": "MIT",
  "command": "{fixture}",
  "args": ["{log_file}"],
  "auth_methods": [{{"id": "oauth", "display_name": "Fixture OAuth"}}]
}}"#
        ),
    )
    .expect("manifest");
}
