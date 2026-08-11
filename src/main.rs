mod app;
mod config;
mod pty;
mod terminal;
mod theme;

fn main() -> eframe::Result {
    app::run()
}
