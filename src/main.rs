mod app;
mod history;
mod hosts;
mod instance;
mod model;
mod platform;
mod ports;
mod store;
mod tray;
mod tunnel;

use eframe::egui;

fn main() -> eframe::Result {
    // Logs go to <data dir>/app.log; fall back to stderr.
    let _log_guard = match store::data_dir() {
        Ok(dir) => {
            let (w, guard) = tracing_appender::non_blocking(tracing_appender::rolling::never(dir, "app.log"));
            tracing_subscriber::fmt().with_writer(w).with_ansi(false).init();
            Some(guard)
        }
        Err(_) => {
            tracing_subscriber::fmt::init();
            None
        }
    };

    let guard = match instance::acquire() {
        instance::Acquired::AlreadyRunning => return Ok(()),
        instance::Acquired::First(l) => Some(l),
        instance::Acquired::Unguarded => None,
    };

    let start_minimized = store::load::<model::Settings>("settings.json").unwrap_or_default().start_minimized;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 600.0]).with_visible(!start_minimized),
        ..Default::default()
    };
    eframe::run_native("SSH Tunnel Manager", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, guard)))))
}
