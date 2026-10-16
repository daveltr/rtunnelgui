use eframe::egui;
use egui_extras::{Column, TableBuilder};
use uuid::Uuid;

use super::App;
use crate::ports::PortInfo;

impl App {
    pub(super) fn ports_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Filter:");
            ui.text_edit_singleline(&mut self.port_filter);
            ui.checkbox(&mut self.only_mine, "only mine");
        });
        ui.add_space(6.0);
        let mine = self.manager.pids();
        let q = self.port_filter.to_lowercase();
        let rows: Vec<&PortInfo> = self
            .ports
            .iter()
            .filter(|p| !self.only_mine || mine.contains_key(&p.pid))
            .filter(|p| q.is_empty() || p.port.to_string().contains(&q) || p.process.to_lowercase().contains(&q))
            .collect();
        let mut stop: Option<Uuid> = None;
        TableBuilder::new(ui)
            .striped(true)
            .column(Column::auto().at_least(60.0))
            .column(Column::auto().at_least(120.0))
            .column(Column::auto().at_least(140.0))
            .column(Column::auto().at_least(60.0))
            .column(Column::remainder())
            .header(20.0, |mut h| {
                for t in ["Port", "Address", "Process", "PID", "Tunnel"] {
                    h.col(|ui| {
                        ui.strong(t);
                    });
                }
            })
            .body(|mut body| {
                for p in rows {
                    let tunnel = mine.get(&p.pid).copied();
                    body.row(22.0, |mut row| {
                        row.col(|ui| {
                            ui.label(p.port.to_string());
                        });
                        row.col(|ui| {
                            ui.label(p.addr.to_string());
                        });
                        row.col(|ui| {
                            ui.label(if p.process.is_empty() { "unknown" } else { &p.process });
                        });
                        row.col(|ui| {
                            ui.label(p.pid.to_string());
                        });
                        row.col(|ui| {
                            if let Some(id) = tunnel {
                                ui.label(self.tunnel_name(id));
                                if ui.small_button("Stop").clicked() {
                                    stop = Some(id);
                                }
                            }
                        });
                    });
                }
            });
        if let Some(id) = stop {
            self.stop_tunnel(id);
        }
    }
}
