use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gio::prelude::*;
use gtk4::{CssProvider, STYLE_PROVIDER_PRIORITY_APPLICATION};
use serde::Deserialize;

/// Omarchy palette from `~/.local/state/omarchy/current/theme/colors.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct ThemeColors {
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default = "accent_fallback")]
    pub accent: String,
    #[serde(default = "selection_fallback")]
    pub selection: String,
    #[serde(default = "muted_fallback")]
    pub muted: String,
    #[serde(default = "bg_fallback")]
    pub background: String,
    #[serde(default = "dark_bg_fallback")]
    pub dark_background: String,
    #[serde(default = "darker_bg_fallback")]
    pub darker_background: String,
    #[serde(default = "lighter_bg_fallback")]
    pub lighter_background: String,
    #[serde(default = "fg_fallback")]
    pub foreground: String,
    #[serde(default = "dark_fg_fallback")]
    pub dark_foreground: String,
    #[serde(default = "light_fg_fallback")]
    pub light_foreground: String,
    #[serde(default = "bright_fg_fallback")]
    pub bright_foreground: String,
    #[serde(default = "yellow_fallback")]
    pub yellow: String,
    #[serde(default = "orange_fallback")]
    pub orange: String,
    #[serde(default = "red_fallback")]
    pub red: String,
}

fn default_mode() -> String {
    "dark".into()
}
fn accent_fallback() -> String {
    "#e68e0d".into()
}
fn selection_fallback() -> String {
    "#2a2a2a".into()
}
fn muted_fallback() -> String {
    "#333333".into()
}
fn bg_fallback() -> String {
    "#121212".into()
}
fn dark_bg_fallback() -> String {
    "#0d0d0d".into()
}
fn darker_bg_fallback() -> String {
    "#090909".into()
}
fn lighter_bg_fallback() -> String {
    "#1e1e1e".into()
}
fn fg_fallback() -> String {
    "#bebebe".into()
}
fn dark_fg_fallback() -> String {
    "#555555".into()
}
fn light_fg_fallback() -> String {
    "#8a8a8d".into()
}
fn bright_fg_fallback() -> String {
    "#eaeaea".into()
}
fn yellow_fallback() -> String {
    "#dbbc7f".into()
}
fn orange_fallback() -> String {
    "#e09d7f".into()
}
fn red_fallback() -> String {
    "#e67e80".into()
}

impl Default for ThemeColors {
    fn default() -> Self {
        Self {
            mode: default_mode(),
            accent: accent_fallback(),
            selection: selection_fallback(),
            muted: muted_fallback(),
            background: bg_fallback(),
            dark_background: dark_bg_fallback(),
            darker_background: darker_bg_fallback(),
            lighter_background: lighter_bg_fallback(),
            foreground: fg_fallback(),
            dark_foreground: dark_fg_fallback(),
            light_foreground: light_fg_fallback(),
            bright_foreground: bright_fg_fallback(),
            yellow: yellow_fallback(),
            orange: orange_fallback(),
            red: red_fallback(),
        }
    }
}

impl ThemeColors {
    /// Parse `#rrggbb` into 0..1 RGB for Cairo.
    pub fn rgb(hex: &str) -> (f64, f64, f64) {
        let h = hex.trim().trim_start_matches('#');
        if h.len() < 6 {
            return (0.5, 0.5, 0.5);
        }
        let parse = |i: usize| {
            u8::from_str_radix(&h[i..i + 2], 16)
                .map(|v| v as f64 / 255.0)
                .unwrap_or(0.5)
        };
        (parse(0), parse(2), parse(4))
    }
}

pub fn omarchy_current_dir() -> PathBuf {
    dirs_state_home().join("omarchy/current")
}

fn dirs_state_home() -> PathBuf {
    if let Ok(p) = std::env::var("XDG_STATE_HOME") {
        return PathBuf::from(p);
    }
    home_dir().join(".local/state")
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn colors_toml_path() -> PathBuf {
    omarchy_current_dir().join("theme/colors.toml")
}

pub fn theme_name_path() -> PathBuf {
    omarchy_current_dir().join("theme.name")
}

pub fn load_omarchy_colors() -> ThemeColors {
    let path = colors_toml_path();
    match std::fs::read_to_string(&path) {
        Ok(raw) => match toml::from_str::<ThemeColors>(&raw) {
            Ok(c) => c,
            Err(err) => {
                eprintln!("reprise: bad {}: {err}", path.display());
                ThemeColors::default()
            }
        },
        Err(_) => ThemeColors::default(),
    }
}

fn render_css(colors: &ThemeColors) -> String {
    let skeleton = include_str!("style.css");
    format!(
        r#"/* Generated from Omarchy colors.toml — do not edit by hand */
@define-color reprise_bg {background};
@define-color reprise_bg_dark {dark_background};
@define-color reprise_bg_darker {darker_background};
@define-color reprise_bg_light {lighter_background};
@define-color reprise_fg {foreground};
@define-color reprise_fg_bright {bright_foreground};
@define-color reprise_fg_light {light_foreground};
@define-color reprise_fg_dark {dark_foreground};
@define-color reprise_accent {accent};
@define-color reprise_selection {selection};
@define-color reprise_muted {muted};
@define-color reprise_yellow {yellow};
@define-color reprise_orange {orange};
@define-color reprise_red {red};

{skeleton}
"#,
        background = colors.background,
        dark_background = colors.dark_background,
        darker_background = colors.darker_background,
        lighter_background = colors.lighter_background,
        foreground = colors.foreground,
        bright_foreground = colors.bright_foreground,
        light_foreground = colors.light_foreground,
        dark_foreground = colors.dark_foreground,
        accent = colors.accent,
        selection = colors.selection,
        muted = colors.muted,
        yellow = colors.yellow,
        orange = colors.orange,
        red = colors.red,
        skeleton = skeleton,
    )
}

/// Live Omarchy theme bridge: CSS provider + shared palette + file watches.
pub struct ThemeHandle {
    pub colors: Rc<RefCell<ThemeColors>>,
    provider: CssProvider,
    listeners: Rc<RefCell<Vec<Rc<dyn Fn()>>>>,
    /// Kept alive for the life of the app.
    _monitors: RefCell<Vec<gio::FileMonitor>>,
    reload_pending: Rc<RefCell<Option<glib::SourceId>>>,
}

impl ThemeHandle {
    pub fn install() -> Rc<Self> {
        let colors = Rc::new(RefCell::new(load_omarchy_colors()));
        let provider = CssProvider::new();
        provider.load_from_string(&render_css(&colors.borrow()));
        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().expect("display"),
            &provider,
            STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let handle = Rc::new(Self {
            colors,
            provider,
            listeners: Rc::new(RefCell::new(Vec::new())),
            _monitors: RefCell::new(Vec::new()),
            reload_pending: Rc::new(RefCell::new(None)),
        });

        handle.attach_watchers();
        handle
    }

    pub fn on_change(&self, cb: impl Fn() + 'static) {
        self.listeners.borrow_mut().push(Rc::new(cb));
    }

    pub fn reload(self: &Rc<Self>) {
        let next = load_omarchy_colors();
        *self.colors.borrow_mut() = next;
        self.provider
            .load_from_string(&render_css(&self.colors.borrow()));
        for cb in self.listeners.borrow().iter() {
            cb();
        }
        // Theme directory was replaced — re-arm file monitors.
        self.attach_watchers();
    }

    fn schedule_reload(self: &Rc<Self>) {
        let pending = self.reload_pending.clone();
        if pending.borrow().is_some() {
            return;
        }
        let this = Rc::clone(self);
        let id = glib::timeout_add_local_once(std::time::Duration::from_millis(120), move || {
            *this.reload_pending.borrow_mut() = None;
            this.reload();
        });
        *pending.borrow_mut() = Some(id);
    }

    fn attach_watchers(self: &Rc<Self>) {
        self._monitors.borrow_mut().clear();

        let watch = |this: &Rc<Self>, path: PathBuf| {
            let file = gio::File::for_path(&path);
            if !path.exists() {
                return;
            }
            match file.monitor_file(gio::FileMonitorFlags::WATCH_MOUNTS, gio::Cancellable::NONE) {
                Ok(mon) => {
                    let this_cb = Rc::clone(this);
                    mon.connect_changed(move |_mon, _file, _other, event| {
                        use gio::FileMonitorEvent::*;
                        match event {
                            ChangesDoneHint
                            | Changed
                            | Created
                            | Deleted
                            | Renamed
                            | MovedIn
                            | MovedOut
                            | AttributeChanged => this_cb.schedule_reload(),
                            _ => {}
                        }
                    });
                    this._monitors.borrow_mut().push(mon);
                }
                Err(err) => eprintln!("reprise: cannot watch {}: {err}", path.display()),
            }
        };

        let name_path = theme_name_path();
        if name_path.exists() {
            watch(self, name_path);
        }
        let colors_path = colors_toml_path();
        if colors_path.exists() {
            watch(self, colors_path);
        }

        // Also watch the current/ directory for atomic theme swaps.
        let current = omarchy_current_dir();
        let dir = gio::File::for_path(&current);
        if let Ok(mon) = dir.monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
        {
            let this_cb = Rc::clone(self);
            mon.connect_changed(move |_mon, file, _other, event| {
                use gio::FileMonitorEvent::*;
                let name = file
                    .basename()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if matches!(
                    event,
                    ChangesDoneHint | Changed | Created | Deleted | Renamed | MovedIn | MovedOut
                ) && (name == "theme.name" || name == "theme" || name == "next-theme")
                {
                    this_cb.schedule_reload();
                }
            });
            self._monitors.borrow_mut().push(mon);
        }
    }
}
