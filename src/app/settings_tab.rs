use eframe::egui;

use super::App;
use crate::tunnel;

fn set_launch_at_login(enable: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let al = auto_launch::AutoLaunchBuilder::new()
        .set_app_name("SSH Tunnel Manager")
        .set_app_path(&exe.to_string_lossy())
        .build()
        .map_err(|e| e.to_string())?;
    if enable { al.enable() } else { al.disable() }.map_err(|e| e.to_string())
}

fn open_folder(p: &std::path::Path) {
    let _ = std::fs::create_dir_all(p);
    let opener = if cfg!(target_os = "macos") { "open" } else if cfg!(windows) { "explorer" } else { "xdg-open" };
    let _ = std::process::Command::new(opener).arg(p).spawn();
}

impl App {
    pub(super) fn settings_tab(&mut self, ui: &mut egui::Ui) {
        let before = self.settings.clone();
        egui::Grid::new("settings").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("ssh binary");
            ui.vertical(|ui| {
                let mut path = self.settings.ssh_path.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                let detected = tunnel::detect_ssh().map(|p| p.display().to_string()).unwrap_or_else(|| "not found on PATH".into());
                ui.add(egui::TextEdit::singleline(&mut path).hint_text(format!("auto-detect ({detected})")).desired_width(360.0));
                self.settings.ssh_path = Some(path.trim().into()).filter(|p: &std::path::PathBuf| !p.as_os_str().is_empty());
            });
            ui.end_row();

            ui.label("SSH_AUTH_SOCK");
            let mut sock = self.settings.ssh_auth_sock.clone().unwrap_or_default();
            ui.add(egui::TextEdit::singleline(&mut sock).hint_text("inherit (macOS: asks launchd)").desired_width(360.0));
            self.settings.ssh_auth_sock = Some(sock.trim().to_string()).filter(|s| !s.is_empty());
            ui.end_row();

            ui.label("");
            ui.checkbox(&mut self.settings.start_minimized, "Start minimized to tray");
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut self.settings.launch_at_login, "Launch at login");
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut self.settings.reconnect_default, "Reconnect automatically by default (new forwards)");
            ui.end_row();
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Open config folder").clicked() {
                if let Ok(p) = crate::store::config_dir() {
                    open_folder(&p);
                }
            }
            if ui.button("Open data folder (history, logs)").clicked() {
                if let Ok(p) = crate::store::data_dir() {
                    open_folder(&p);
                }
            }
        });
        ui.small("Changes to the ssh path apply to tunnels started afterwards.");

        // Persist on change (text fields save per keystroke; the file is tiny).
        let s = &self.settings;
        let changed = s.ssh_path != before.ssh_path
            || s.ssh_auth_sock != before.ssh_auth_sock
            || s.start_minimized != before.start_minimized
            || s.launch_at_login != before.launch_at_login
            || s.reconnect_default != before.reconnect_default;
        if changed {
            if s.launch_at_login != before.launch_at_login {
                if let Err(e) = set_launch_at_login(s.launch_at_login) {
                    self.notice = Some(format!("launch at login: {e}"));
                    self.settings.launch_at_login = before.launch_at_login;
                }
            }
            self.save_settings();
        }
    }
}
