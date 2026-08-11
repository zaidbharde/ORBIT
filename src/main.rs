mod app;
mod config;
mod pty;
mod terminal;

fn main() -> eframe::Result {
    app::run()
}
