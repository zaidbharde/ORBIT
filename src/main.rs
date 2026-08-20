mod app;
mod color;
mod config;
mod glass;
mod pty;
mod section;
mod terminal;
mod theme;

fn main() -> eframe::Result {
    app::run()
}
