use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use uuid::Uuid;

use crate::app::{AppEvent, Bus};

#[derive(Clone, Copy, Debug)]
pub enum TrayAction {
    ToggleWindow,
    Show,
    ToggleForward(Uuid),
    ToggleGroup(Uuid),
    Quit,
}

#[derive(Clone, PartialEq)]
pub struct Entry {
    pub id: String, // "f:<uuid>" forward, "g:<uuid>" group
    pub label: String,
    pub checked: bool,
}

#[derive(Clone, PartialEq, Default)]
pub struct TrayState {
    pub entries: Vec<Entry>,
    pub active: usize,
}

fn parse_action(id: &str) -> Option<TrayAction> {
    match id {
        "toggle_window" => Some(TrayAction::ToggleWindow),
        "quit" => Some(TrayAction::Quit),
        _ => {
            let (kind, rest) = id.split_once(':')?;
            let uuid = Uuid::parse_str(rest).ok()?;
            match kind {
                "f" => Some(TrayAction::ToggleForward(uuid)),
                "g" => Some(TrayAction::ToggleGroup(uuid)),
                _ => None,
            }
        }
    }
}

fn icon(active: bool) -> Icon {
    let png: &[u8] = if active { include_bytes!("../assets/icon_active.png") } else { include_bytes!("../assets/icon_idle.png") };
    let img = image::load_from_memory(png).expect("embedded icon").to_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h).expect("icon rgba")
}

fn menu(st: &TrayState) -> Menu {
    let m = Menu::new();
    let _ = m.append(&MenuItem::with_id("toggle_window", "Show / Hide", true, None));
    let _ = m.append(&PredefinedMenuItem::separator());
    for e in &st.entries {
        let _ = m.append(&CheckMenuItem::with_id(e.id.as_str(), &e.label, true, e.checked, None));
    }
    if !st.entries.is_empty() {
        let _ = m.append(&PredefinedMenuItem::separator());
    }
    let _ = m.append(&MenuItem::with_id("quit", "Quit", true, None));
    m
}

fn apply(tray: &TrayIcon, st: &TrayState) {
    tray.set_menu(Some(Box::new(menu(st))));
    tray.set_icon(Some(icon(st.active > 0))).ok();
    tray.set_tooltip(Some(match st.active {
        0 => "SSH Tunnel Manager".to_string(),
        n => format!("{n} tunnel{} active", if n == 1 { "" } else { "s" }),
    }))
    .ok();
}

fn build(st: &TrayState) -> Result<TrayIcon, tray_icon::Error> {
    TrayIconBuilder::new()
        .with_menu(Box::new(menu(st)))
        .with_icon(icon(false))
        .with_tooltip("SSH Tunnel Manager")
        .with_menu_on_left_click(cfg!(target_os = "macos")) // elsewhere left-click toggles the window
        .build()
}

fn install_handlers(bus: Bus) {
    let b = bus.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if let Some(a) = parse_action(&e.id.0) {
            b.send(AppEvent::Tray(a));
        }
    }));
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
            bus.send(AppEvent::Tray(TrayAction::ToggleWindow));
        }
    }));
}

/// macOS/Windows: the icon lives on the UI (main) thread.
#[cfg(not(target_os = "linux"))]
pub struct Tray {
    icon: TrayIcon,
    last: Option<TrayState>,
}

#[cfg(not(target_os = "linux"))]
impl Tray {
    pub fn new(bus: Bus) -> Option<Tray> {
        install_handlers(bus);
        match build(&TrayState::default()) {
            Ok(icon) => Some(Tray { icon, last: None }),
            Err(e) => {
                tracing::warn!("tray unavailable: {e}");
                None
            }
        }
    }

    pub fn update(&mut self, st: TrayState) {
        if self.last.as_ref() != Some(&st) {
            apply(&self.icon, &st);
            self.last = Some(st);
        }
    }
}

/// Linux: GTK and the tray run on a dedicated thread; the UI sends state over a channel.
#[cfg(target_os = "linux")]
pub struct Tray {
    tx: crossbeam_channel::Sender<TrayState>,
    last: Option<TrayState>,
}

#[cfg(target_os = "linux")]
impl Tray {
    pub fn new(bus: Bus) -> Option<Tray> {
        use std::time::Duration;
        let (tx, rx) = crossbeam_channel::unbounded::<TrayState>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<bool>();
        install_handlers(bus);
        std::thread::spawn(move || {
            let built = gtk::init().ok().and_then(|_| build(&TrayState::default()).ok());
            let _ = ready_tx.send(built.is_some());
            let Some(tray) = built else { return };
            gtk::glib::timeout_add_local(Duration::from_millis(200), move || {
                while let Ok(st) = rx.try_recv() {
                    apply(&tray, &st);
                }
                gtk::glib::ControlFlow::Continue
            });
            gtk::main();
        });
        match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(true) => Some(Tray { tx, last: None }),
            _ => {
                tracing::warn!("tray unavailable (no GTK / AppIndicator?)");
                None
            }
        }
    }

    pub fn update(&mut self, st: TrayState) {
        if self.last.as_ref() != Some(&st) {
            let _ = self.tx.send(st.clone());
            self.last = Some(st);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_round_trip() {
        let id = Uuid::new_v4();
        assert!(matches!(parse_action(&format!("f:{id}")), Some(TrayAction::ToggleForward(x)) if x == id));
        assert!(matches!(parse_action(&format!("g:{id}")), Some(TrayAction::ToggleGroup(x)) if x == id));
        assert!(matches!(parse_action("quit"), Some(TrayAction::Quit)));
        assert!(parse_action("x:nope").is_none());
    }
}
