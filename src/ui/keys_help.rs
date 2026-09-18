//! In-app keybindings overlay (Esc to dismiss). Page-specific lists.

use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Label, Orientation, Overlay};

const CHORD_COLUMN: usize = 28;

pub(crate) struct Binding {
    pub chord: &'static str,
    pub action: &'static str,
}

/// Playbench (main board) shortcuts.
pub(crate) const PLAYBENCH: &[Binding] = &[
    Binding {
        chord: "n / p",
        action: "Walk pivotal moments",
    },
    Binding {
        chord: "vb",
        action: "View best line",
    },
    Binding {
        chord: "vp",
        action: "View how punished",
    },
    Binding {
        chord: "← / → / h / l",
        action: "Step through plies",
    },
    Binding {
        chord: "Home / End",
        action: "First / last ply",
    },
    Binding {
        chord: "g<digit>",
        action: "Jump to ply <digit>",
    },
    Binding {
        chord: "a",
        action: "Analyze game",
    },
    Binding {
        chord: "aa",
        action: "Analyze all unanalyzed",
    },
    Binding {
        chord: "s",
        action: "Sync new games",
    },
    Binding {
        chord: "o",
        action: "Games list",
    },
    Binding {
        chord: "d",
        action: "Progress",
    },
    Binding {
        chord: "Ctrl+A",
        action: "Ask default agent",
    },
    Binding {
        chord: "?",
        action: "Keybindings",
    },
    Binding {
        chord: "Esc",
        action: "Close help / exit line",
    },
];

/// Games list shortcuts.
pub(crate) const LIBRARY: &[Binding] = &[
    Binding {
        chord: "1 / 2 / 3",
        action: "Jump section",
    },
    Binding {
        chord: "↑↓ / j k",
        action: "Move selection",
    },
    Binding {
        chord: "Enter",
        action: "Open on playbench",
    },
    Binding {
        chord: "p",
        action: "Open + analyze",
    },
    Binding {
        chord: "a",
        action: "Analyze here",
    },
    Binding {
        chord: "s",
        action: "Sync new games",
    },
    Binding {
        chord: "/",
        action: "Filter",
    },
    Binding {
        chord: "?",
        action: "Keybindings",
    },
    Binding {
        chord: "Esc",
        action: "Back / close help",
    },
];

/// Progress dashboard shortcuts.
pub(crate) const PROGRESS: &[Binding] = &[
    Binding {
        chord: "?",
        action: "Keybindings",
    },
    Binding {
        chord: "Esc",
        action: "Back to playbench",
    },
];

fn format_row(chord: &str, action: &str) -> String {
    let pad = CHORD_COLUMN.saturating_sub(chord.chars().count()).max(1);
    format!("{chord}{:pad$}→  {action}", "")
}

/// Build a help panel for the given bindings (add to an Overlay; start hidden).
pub(crate) fn build_overlay_child(title: &str, bindings: &[Binding]) -> gtk4::Box {
    let root = GtkBox::new(Orientation::Vertical, 0);
    root.add_css_class("keys-help-root");
    root.set_hexpand(true);
    root.set_vexpand(true);
    root.set_visible(false);
    root.set_can_target(true);

    let layer = Overlay::new();
    layer.set_hexpand(true);
    layer.set_vexpand(true);

    let dim = GtkBox::new(Orientation::Vertical, 0);
    dim.add_css_class("keys-help-dim");
    dim.set_hexpand(true);
    dim.set_vexpand(true);
    layer.set_child(Some(&dim));

    let card = GtkBox::new(Orientation::Vertical, 12);
    card.add_css_class("keys-help-card");
    card.set_halign(Align::Center);
    card.set_valign(Align::Center);

    let title_lbl = Label::new(Some(title));
    title_lbl.add_css_class("keys-help-title");
    title_lbl.set_halign(Align::Start);
    card.append(&title_lbl);

    let list = GtkBox::new(Orientation::Vertical, 2);
    list.add_css_class("keys-help-list");
    for b in bindings {
        let row = Label::new(Some(&format_row(b.chord, b.action)));
        row.add_css_class("keys-help-row");
        row.set_halign(Align::Start);
        row.set_xalign(0.0);
        list.append(&row);
    }
    card.append(&list);

    let hint = Label::new(Some("Esc to close"));
    hint.add_css_class("keys-help-hint");
    hint.set_halign(Align::Start);
    card.append(&hint);

    layer.add_overlay(&card);
    root.append(&layer);
    root
}
