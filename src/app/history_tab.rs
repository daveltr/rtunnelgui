use chrono::Local;
use eframe::egui::{self, Color32};
use egui_extras::{Column, TableBuilder};

use super::App;
use crate::history::{self, HistoryEntry};
use crate::model::Forward;

fn fmt_duration(secs: i64) -> String {
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    if h > 0 { format!("{h}h {m}m") } else if m > 0 { format!("{m}m {s}s") } else { format!("{s}s") }
}

impl App {
    fn session_result(&self, e: &HistoryEntry) -> (String, Color32) {
        if self.sessions.get(&e.forward.id).is_some_and(|s| s.session == e.session) {
            return ("running".into(), Color32::GREEN);
        }
        match (&e.ended, &e.error) {
            (None, _) => ("interrupted".into(), Color32::GRAY),
            (_, Some(err)) => (format!("failed: {}", err.lines().last().unwrap_or("")), Color32::LIGHT_RED),
            _ => ("stopped".into(), Color32::GRAY),
        }
    }

    pub(super) fn history_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Filter:");
            ui.text_edit_singleline(&mut self.history_filter);
            if ui.button("Clear history…").clicked() {
                self.confirm_clear_history = true;
            }
        });
        if self.confirm_clear_history {
            ui.horizontal(|ui| {
                ui.colored_label(Color32::YELLOW, "Delete all history?");
                if ui.button("Yes, clear").clicked() {
                    match history::clear() {
                        Ok(()) => self.history.clear(),
                        Err(e) => self.notice = Some(format!("could not clear history: {e:#}")),
                    }
                    self.confirm_clear_history = false;
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_clear_history = false;
                }
            });
        }
        ui.add_space(6.0);

        let q = self.history_filter.to_lowercase();
        let now = Local::now();
        let mut rerun: Option<Forward> = None;
        let rows: Vec<&HistoryEntry> = self
            .history
            .iter()
            .rev()
            .filter(|e| q.is_empty() || e.forward.name.to_lowercase().contains(&q) || e.forward.summary().to_lowercase().contains(&q))
            .collect();
        TableBuilder::new(ui)
            .striped(true)
            .column(Column::auto().at_least(140.0))
            .column(Column::auto().at_least(100.0))
            .column(Column::auto().at_least(220.0))
            .column(Column::auto().at_least(70.0))
            .column(Column::remainder().at_least(120.0))
            .column(Column::auto())
            .header(20.0, |mut h| {
                for t in ["Started", "Name", "Forward", "Duration", "Result", ""] {
                    h.col(|ui| {
                        ui.strong(t);
                    });
                }
            })
            .body(|mut body| {
                for e in rows {
                    let (result, color) = self.session_result(e);
                    let dur = match (e.ended, result.as_str()) {
                        (Some(end), _) => fmt_duration((end - e.started).num_seconds()),
                        (None, "running") => fmt_duration((now - e.started).num_seconds()),
                        _ => "—".into(),
                    };
                    body.row(22.0, |mut row| {
                        row.col(|ui| {
                            ui.label(e.started.format("%Y-%m-%d %H:%M:%S").to_string());
                        });
                        row.col(|ui| {
                            ui.label(&e.forward.name);
                        });
                        row.col(|ui| {
                            ui.label(e.forward.summary());
                        });
                        row.col(|ui| {
                            ui.label(dur);
                        });
                        row.col(|ui| {
                            ui.colored_label(color, &result);
                        });
                        row.col(|ui| {
                            if ui.small_button("Re-run").clicked() {
                                rerun = Some(e.forward.clone());
                            }
                        });
                    });
                }
            });
        if let Some(f) = rerun {
            self.rerun(f);
        }
    }
}
