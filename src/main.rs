use std::io;

mod app;
mod tui;

fn main() -> io::Result<()> {
    tui::run_app()
}
