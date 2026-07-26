use misy::{MisyCore, MisyPaths, tui};
use std::{error::Error, path::Path};

fn main() -> Result<(), Box<dyn Error>> {
    let core = MisyCore::discover(MisyPaths::from_home()?, Path::new("plugins/providers"))?;
    tui::run(core)?;
    Ok(())
}
