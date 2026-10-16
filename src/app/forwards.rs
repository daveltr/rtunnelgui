use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use uuid::Uuid;

use super::{is_active, App, Draft, GroupDraft};
use crate::hosts::KnownHost;
use crate::model::{Forward, Group, Kind, Status};
use crate::{ports, tunnel};

/// "▾" menu listing known hosts matching what's typed; picking one fills the field.
fn host_menu(ui: &mut egui::Ui, hosts: &[KnownHost], field: &mut String) {
    ui.menu_button("▾", |ui| {
        let q = field.to_lowercase();
        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
            let mut any = false;
            for h in hosts.iter().filter(|h| q.is_empty() || h.alias.to_lowercase().contains(&q)).take(40) {
                any = true;
                if ui.button(format!("{}  ({}:{})", h.alias, h.hostname, h.port)).clicked() {
                    *field = h.alias.clone();
                    ui.close();
                }
            }
            if !any {
                ui.label("no matching hosts");
            }
        });
    });
}

fn identity_row(ui: &mut egui::Ui, field: &mut String) {
    ui.horizontal(|ui| {
        ui.text_edit_singleline(field);
        if ui.button("Browse…").clicked() {
            if let Some(p) = rfd::FileDialog::new().set_directory(directories::BaseDirs::new().map(|d| d.home_dir().join(".ssh")).unwrap_or_default()).pick_file() {
                *field = p.to_string_lossy().into_owned();
            }
        }
    });
}

impl App {
    pub(super) fn groups_section(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Favorite groups");
            if ui.button("➕ Add group").clicked() {
                self.group_draft = Some(GroupDraft::new(self.settings.reconnect_default));
            }
        });
        let mut action: Option<(&str, Uuid)> = None;
        for g in &self.groups {
            let fwds = g.forwards();
            let st = |f: &Forward| self.status.get(&f.id).cloned().unwrap_or(Status::Stopped);
            let running = fwds.iter().filter(|f| matches!(st(f), Status::Running { .. })).count();
            let active = fwds.iter().filter(|f| is_active(&st(f))).count();
            let ports: Vec<String> = g.ports.iter().map(|p| if p.local == p.remote { p.local.to_string() } else { format!("{}:{}", p.local, p.remote) }).collect();
            ui.horizontal(|ui| {
                let color = if running == fwds.len() { Color32::GREEN } else if active > 0 { Color32::YELLOW } else { Color32::GRAY };
                ui.label(RichText::new("●").color(color));
                ui.strong(&g.name);
                ui.label(format!("{} — ports {} ({running}/{} running)", g.ssh_host, ports.join(", "), fwds.len()));
                if active < fwds.len() && ui.button("Start").clicked() {
                    action = Some(("start", g.id));
                }
                if active > 0 && ui.button("Stop").clicked() {
                    action = Some(("stop", g.id));
                }
                if ui.button("Edit").clicked() {
                    action = Some(("edit", g.id));
                }
                if ui.button("Delete").clicked() {
                    action = Some(("delete", g.id));
                }
            });
            for f in &fwds {
                if let Status::Failed(e) = st(f) {
                    ui.colored_label(Color32::LIGHT_RED, format!("    {}: {e}", f.name));
                }
            }
        }
        ui.separator();
        match action {
            Some(("start", id)) => self.start_group(id),
            Some(("stop", id)) => self.stop_group(id),
            Some(("edit", id)) => self.group_draft = self.groups.iter().find(|g| g.id == id).map(GroupDraft::from),
            Some(("delete", id)) => self.confirm_delete_group = Some(id),
            _ => {}
        }
    }

    pub(super) fn forwards_tab(&mut self, ui: &mut egui::Ui) {
        self.groups_section(ui);
        ui.horizontal(|ui| {
            ui.heading("Forwards");
            if ui.button("➕ Add forward").clicked() {
                self.draft = Some(Draft::new(self.settings.reconnect_default));
            }
            ui.toggle_value(&mut self.show_hosts, "Hosts panel");
        });
        ui.add_space(6.0);

        let mut action: Option<(&str, Uuid)> = None;
        TableBuilder::new(ui)
            .striped(true)
            .column(Column::exact(24.0))
            .column(Column::auto().at_least(120.0))
            .column(Column::remainder().at_least(240.0))
            .column(Column::auto())
            .header(20.0, |mut h| {
                h.col(|_| {});
                h.col(|ui| {
                    ui.strong("Name");
                });
                h.col(|ui| {
                    ui.strong("Forward");
                });
                h.col(|ui| {
                    ui.strong("");
                });
            })
            .body(|mut body| {
                for f in &self.forwards {
                    let st = self.status.get(&f.id).cloned().unwrap_or(Status::Stopped);
                    body.row(26.0, |mut row| {
                        row.col(|ui| {
                            let (color, tip) = match &st {
                                Status::Stopped => (Color32::GRAY, "stopped".to_string()),
                                Status::Starting => (Color32::YELLOW, "starting".into()),
                                Status::Reconnecting { attempt } => (Color32::YELLOW, format!("reconnecting (attempt {attempt})")),
                                Status::Running { pid, since } => (Color32::GREEN, format!("running (pid {pid}) since {}", since.format("%H:%M:%S"))),
                                Status::Failed(e) => (Color32::RED, e.clone()),
                            };
                            ui.label(RichText::new("●").color(color)).on_hover_text(tip);
                        });
                        row.col(|ui| {
                            ui.label(&f.name);
                        });
                        row.col(|ui| {
                            ui.label(f.summary());
                        });
                        row.col(|ui| {
                            if is_active(&st) {
                                if ui.button("Stop").clicked() {
                                    action = Some(("stop", f.id));
                                }
                            } else if ui.button("Start").clicked() {
                                action = Some(("start", f.id));
                            }
                            if ui.button("Edit").clicked() {
                                action = Some(("edit", f.id));
                            }
                            if ui.button("Delete").clicked() {
                                action = Some(("delete", f.id));
                            }
                        });
                    });
                }
            });

        // Failed forwards: show the error under the table so it's readable, not hover-only.
        for f in &self.forwards {
            if let Some(Status::Failed(e)) = self.status.get(&f.id) {
                ui.colored_label(Color32::LIGHT_RED, format!("{}: {e}", f.name));
            }
        }

        match action {
            Some(("start", id)) => self.start(id),
            Some(("stop", id)) => self.stop_tunnel(id),
            Some(("edit", id)) => self.draft = self.forward(id).map(Draft::from),
            Some(("delete", id)) => self.confirm_delete = Some(id),
            _ => {}
        }
    }

    pub(super) fn draft_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.draft.take() else { return };
        let hosts = self.all_hosts();
        let targets = self.targets();
        let mut open = true;
        let (mut save, mut cancel) = (false, false);
        egui::Window::new(if d.is_new { "Add forward" } else { "Edit forward" }).open(&mut open).collapsible(false).show(ctx, |ui| {
            egui::Grid::new("draft").num_columns(2).show(ui, |ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut d.name);
                ui.end_row();
                ui.label("Type");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut d.kind, Kind::Local, "Local (-L)");
                    ui.radio_value(&mut d.kind, Kind::Remote, "Remote (-R)");
                    ui.radio_value(&mut d.kind, Kind::Dynamic, "Dynamic (-D)");
                });
                ui.end_row();
                ui.label("Bind address");
                ui.add(egui::TextEdit::singleline(&mut d.bind_addr).hint_text("127.0.0.1"));
                ui.end_row();
                ui.label("Bind port");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut d.bind_port).desired_width(70.0));
                    if ui.button("Pick free").clicked() {
                        if let Some(p) = ports::pick_free() {
                            d.bind_port = p.to_string();
                        }
                    }
                });
                ui.end_row();
                if d.kind != Kind::Dynamic {
                    ui.label("Target host");
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut d.target_host);
                        ui.menu_button("▾", |ui| {
                            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                                if targets.is_empty() {
                                    ui.label("no previous targets");
                                }
                                for (h, p) in &targets {
                                    if ui.button(format!("{h}:{p}")).clicked() {
                                        d.target_host = h.clone();
                                        d.target_port = p.to_string();
                                        ui.close();
                                    }
                                }
                            });
                        });
                    });
                    ui.end_row();
                    ui.label("Target port");
                    ui.add(egui::TextEdit::singleline(&mut d.target_port).desired_width(70.0));
                    ui.end_row();
                }
                ui.label("SSH host");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut d.ssh_host).hint_text("alias or user@host"));
                    host_menu(ui, &hosts, &mut d.ssh_host);
                });
                ui.end_row();
                ui.label("SSH port");
                ui.add(egui::TextEdit::singleline(&mut d.ssh_port).desired_width(70.0).hint_text("22"));
                ui.end_row();
                ui.label("Identity file");
                identity_row(ui, &mut d.identity);
                ui.end_row();
                ui.label("Extra options");
                ui.add(egui::TextEdit::singleline(&mut d.extra_opts).hint_text("-J bastion"));
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut d.auto_start, "Start on launch");
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut d.auto_reconnect, "Reconnect automatically");
                ui.end_row();
            });

            let parsed = d.to_forward(&self.forwards);
            if let Ok(f) = &parsed {
                if f.kind != Kind::Remote {
                    for w in self.conflicts(&[f.bind_port], &[f.id]) {
                        ui.colored_label(Color32::YELLOW, w);
                    }
                }
            }

            ui.separator();
            match &parsed {
                Ok(f) => {
                    let cmd = format!("ssh {}", tunnel::build_args(f).join(" "));
                    ui.label("Command:");
                    ui.add(egui::Label::new(RichText::new(&cmd).monospace()).wrap());
                    if ui.small_button("Copy").clicked() {
                        ctx.copy_text(cmd);
                    }
                }
                Err(e) => {
                    ui.colored_label(Color32::LIGHT_RED, e);
                }
            }
            ui.horizontal(|ui| {
                save = ui.add_enabled(parsed.is_ok(), egui::Button::new("Save")).clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });

        if save {
            if let Ok(f) = d.to_forward(&self.forwards) {
                match self.forwards.iter_mut().find(|x| x.id == f.id) {
                    Some(slot) => *slot = f,
                    None => {
                        self.status.insert(f.id, Status::Stopped);
                        self.forwards.push(f);
                    }
                }
                self.save();
                return;
            }
        }
        if open && !cancel {
            self.draft = Some(d);
        }
    }

    pub(super) fn group_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.group_draft.take() else { return };
        let hosts = self.all_hosts();
        let mut open = true;
        let (mut save, mut cancel) = (false, false);
        egui::Window::new(if d.is_new { "Add group" } else { "Edit group" }).open(&mut open).collapsible(false).show(ctx, |ui| {
            egui::Grid::new("gdraft").num_columns(2).show(ui, |ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut d.name);
                ui.end_row();
                ui.label("SSH host");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut d.ssh_host).hint_text("alias or user@host"));
                    host_menu(ui, &hosts, &mut d.ssh_host);
                });
                ui.end_row();
                ui.label("Ports");
                ui.add(egui::TextEdit::singleline(&mut d.ports).hint_text("3000 9002 80   (or 8080:80 local:remote)"));
                ui.end_row();
                ui.label("SSH port");
                ui.add(egui::TextEdit::singleline(&mut d.ssh_port).desired_width(70.0).hint_text("22"));
                ui.end_row();
                ui.label("Identity file");
                identity_row(ui, &mut d.identity);
                ui.end_row();
                ui.label("Extra options");
                ui.add(egui::TextEdit::singleline(&mut d.extra_opts).hint_text("-J bastion"));
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut d.auto_start, "Start on launch");
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut d.auto_reconnect, "Reconnect automatically");
                ui.end_row();
            });
            let parsed = d.to_group(&self.groups);
            match &parsed {
                Ok(g) => {
                    let fwds = g.forwards();
                    let locals: Vec<u16> = g.ports.iter().map(|p| p.local).collect();
                    let own: Vec<Uuid> = fwds.iter().map(|f| f.id).collect();
                    for w in self.conflicts(&locals, &own) {
                        ui.colored_label(Color32::YELLOW, w);
                    }
                }
                Err(e) => {
                    ui.colored_label(Color32::LIGHT_RED, e);
                }
            }
            ui.horizontal(|ui| {
                save = ui.add_enabled(parsed.is_ok(), egui::Button::new("Save")).clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        if save {
            if let Ok(g) = d.to_group(&self.groups) {
                match self.groups.iter().position(|x| x.id == g.id) {
                    Some(i) => {
                        // Ports removed by the edit must not keep running.
                        let keep: Vec<Uuid> = g.forwards().iter().map(|f| f.id).collect();
                        for f in self.groups[i].forwards().into_iter().filter(|f| !keep.contains(&f.id)) {
                            self.stop_tunnel(f.id);
                        }
                        self.groups[i] = g;
                    }
                    None => self.groups.push(g),
                }
                self.save_groups();
                return;
            }
        }
        if open && !cancel {
            self.group_draft = Some(d);
        }
    }

    pub(super) fn delete_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.confirm_delete else { return };
        let name = self.forward(id).map(|f| f.name.clone()).unwrap_or_default();
        let mut close = false;
        egui::Window::new("Delete forward").collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label(format!("Delete \"{name}\"? A running tunnel will be stopped."));
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    self.stop_tunnel(id);
                    self.forwards.retain(|f| f.id != id);
                    self.status.remove(&id);
                    self.save();
                    close = true;
                }
                close |= ui.button("Cancel").clicked();
            });
        });
        if close {
            self.confirm_delete = None;
        }
    }

    pub(super) fn delete_group_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.confirm_delete_group else { return };
        let Some(g) = self.groups.iter().find(|g| g.id == id).cloned() else {
            self.confirm_delete_group = None;
            return;
        };
        let mut close = false;
        egui::Window::new("Delete group").collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label(format!("Delete group \"{}\"? Its running tunnels will be stopped.", g.name));
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    self.delete_group(&g);
                    close = true;
                }
                close |= ui.button("Cancel").clicked();
            });
        });
        if close {
            self.confirm_delete_group = None;
        }
    }

    fn delete_group(&mut self, g: &Group) {
        for f in g.forwards() {
            self.stop_tunnel(f.id);
            self.status.remove(&f.id);
        }
        self.groups.retain(|x| x.id != g.id);
        self.save_groups();
    }
}
