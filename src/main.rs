mod app;
mod config;
mod glass;
mod pty;
mod terminal;
mod theme;

fn main() -> eframe::Result {
    app::run()
}
