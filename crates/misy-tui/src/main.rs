//! Command-line entry point for the Misy terminal client.

mod cli;

use cli::Arguments;
use misy_core::MisyCore;
use misy_tui::run;
use std::{error::Error, path::Path};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let bundled_providers = bundled_providers();
    let arguments = Arguments::from_env()?;
    let paths = arguments.paths()?;
    let core = MisyCore::discover(paths.clone(), bundled_providers)?;
    run(core, &paths, arguments.session_start()).await?;
    Ok(())
}

/// Resolves repository-bundled providers because this binary lives two levels below the root.
fn bundled_providers() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("plugins/providers")
}
