mod app;
mod config;
mod glass;
mod pty;
mod terminal;
mod theme;
mod workspace;

fn main() -> eframe::Result {
    app::run()
}
