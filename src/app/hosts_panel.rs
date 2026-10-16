use eframe::egui::{self, Color32, RichText};

use super::App;
use crate::hosts::HostSource;

impl App {
    pub(super) fn hosts_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Hosts");
        ui.add(egui::TextEdit::singleline(&mut self.host_filter).hint_text("search"));
        ui.small("Click a host to use it as the SSH host.");
        ui.separator();
        let q = self.host_filter.to_lowercase();
        let mut picked = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for h in self.all_hosts().into_iter().filter(|h| q.is_empty() || h.alias.to_lowercase().contains(&q) || h.hostname.to_lowercase().contains(&q)) {
                let (badge, color) = match h.source {
                    HostSource::Config => ("config", Color32::LIGHT_BLUE),
                    HostSource::KnownHosts => ("known_hosts", Color32::GRAY),
                    HostSource::History => ("history", Color32::LIGHT_GREEN),
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(badge).small().color(color));
                    let detail = match &h.user {
                        Some(u) => format!("{u}@{}:{}", h.hostname, h.port),
                        None => format!("{}:{}", h.hostname, h.port),
                    };
                    if ui.link(&h.alias).on_hover_text(detail).clicked() {
                        picked = Some(h.alias.clone());
                    }
                });
            }
        });
        if let Some(a) = picked {
            self.pick_host(a);
        }
    }
}
