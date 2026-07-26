//! Command-line entry point for the Misy terminal client.

use misy::{MisyCore, MisyPaths, tui};
use std::{error::Error, path::Path};

fn main() -> Result<(), Box<dyn Error>> {
    let bundled_providers = Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/providers");
    let paths = MisyPaths::from_home()?;
    let core = MisyCore::discover(paths.clone(), bundled_providers)?;
    tui::run(core, &paths)?;
    Ok(())
}
