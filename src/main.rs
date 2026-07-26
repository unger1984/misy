//! Command-line entry point for the Misy terminal client.

mod cli;

use cli::Arguments;
use misy::{MisyCore, tui};
use std::{error::Error, path::Path};

fn main() -> Result<(), Box<dyn Error>> {
    let bundled_providers = Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/providers");
    let paths = Arguments::from_env()?.paths()?;
    let core = MisyCore::discover(paths.clone(), bundled_providers)?;
    tui::run(core, &paths)?;
    Ok(())
}
