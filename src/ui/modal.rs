//! In-app modal overlays (same look as the keybindings help panel).

use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Entry, Label, Orientation, Overlay};

/// Shared dimmed scrim + centered card host. Starts hidden.
pub fn build_host() -> (gtk4::Box, gtk4::Box) {
    let root = GtkBox::new(Orientation::Vertical, 0);
    root.add_css_class("modal-root");
    root.set_hexpand(true);
    root.set_vexpand(true);
    root.set_visible(false);
    root.set_can_target(true);
    root.set_focusable(true);

    let layer = Overlay::new();
    layer.set_hexpand(true);
    layer.set_vexpand(true);

    let dim = GtkBox::new(Orientation::Vertical, 0);
    dim.add_css_class("modal-dim");
    dim.set_hexpand(true);
    dim.set_vexpand(true);
    layer.set_child(Some(&dim));

    let card = GtkBox::new(Orientation::Vertical, 12);
    card.add_css_class("modal-card");
    card.set_halign(Align::Center);
    card.set_valign(Align::Center);

    layer.add_overlay(&card);
    root.append(&layer);
    (root, card)
}

fn clear_card(card: &GtkBox) {
    while let Some(child) = card.first_child() {
        card.remove(&child);
    }
}

/// Username prompt. Calls `on_ok` with a non-empty trimmed name.
pub fn show_username(host: &GtkBox, card: &GtkBox, page: &GtkBox, on_ok: impl Fn(String) + 'static) {
    clear_card(card);

    let title = Label::new(Some("Chess.com username"));
    title.add_css_class("modal-title");
    title.set_halign(Align::Start);
    card.append(&title);

    let blurb = Label::new(Some(
        "Enter your chess.com username so Reprise can sync your games.",
    ));
    blurb.add_css_class("modal-body");
    blurb.set_wrap(true);
    blurb.set_xalign(0.0);
    blurb.set_halign(Align::Fill);
    card.append(&blurb);

    let entry = Entry::builder()
        .placeholder_text("username")
        .activates_default(true)
        .hexpand(true)
        .build();
    entry.add_css_class("modal-entry");
    card.append(&entry);

    let actions = GtkBox::new(Orientation::Horizontal, 8);
    actions.set_halign(Align::End);
    let btn = Button::with_label("Continue");
    btn.add_css_class("primary");
    btn.set_sensitive(false);
    actions.append(&btn);
    card.append(&actions);

    entry.connect_changed({
        let btn = btn.clone();
        move |entry| {
            btn.set_sensitive(!entry.text().trim().is_empty());
        }
    });

    let submit = {
        let host = host.clone();
        let page = page.clone();
        let entry = entry.clone();
        let on_ok = Rc::new(on_ok);
        Rc::new(move || {
            let name = entry.text().trim().to_string();
            if name.is_empty() {
                return;
            }
            host.set_visible(false);
            page.set_sensitive(true);
            on_ok(name);
        })
    };

    btn.connect_clicked({
        let submit = submit.clone();
        move |_| submit()
    });
    entry.connect_activate({
        let submit = submit.clone();
        move |_| submit()
    });

    page.set_sensitive(false);
    host.set_visible(true);
    entry.grab_focus();
}

/// Esc / Enter handlers for the active modal (may differ for confirm dialogs).
#[derive(Clone)]
pub struct KeyHandlers {
    pub on_escape: Rc<dyn Fn()>,
    pub on_enter: Rc<dyn Fn()>,
}

/// Simple notice with an OK action. Esc / Enter / OK all dismiss and run `on_ok`.
pub fn show_notice(
    host: &GtkBox,
    card: &GtkBox,
    page: &GtkBox,
    title: &str,
    detail: &str,
    on_ok: impl FnOnce() + 'static,
) -> KeyHandlers {
    clear_card(card);

    let title_lbl = Label::new(Some(title));
    title_lbl.add_css_class("modal-title");
    title_lbl.set_halign(Align::Start);
    card.append(&title_lbl);

    let body = Label::new(Some(detail));
    body.add_css_class("modal-body");
    body.set_wrap(true);
    body.set_xalign(0.0);
    body.set_halign(Align::Fill);
    card.append(&body);

    let actions = GtkBox::new(Orientation::Horizontal, 8);
    actions.set_halign(Align::End);
    let btn = Button::with_label("OK");
    btn.add_css_class("primary");
    actions.append(&btn);
    card.append(&actions);

    let on_ok = Rc::new(std::cell::RefCell::new(Some(on_ok)));
    let dismiss: Rc<dyn Fn()> = {
        let host = host.clone();
        let page = page.clone();
        let on_ok = on_ok.clone();
        Rc::new(move || {
            if !host.is_visible() {
                return;
            }
            host.set_visible(false);
            page.set_sensitive(true);
            if let Some(f) = on_ok.borrow_mut().take() {
                f();
            }
        })
    };

    btn.connect_clicked({
        let dismiss = dismiss.clone();
        move |_| dismiss()
    });

    page.set_sensitive(false);
    host.set_visible(true);
    btn.grab_focus();
    KeyHandlers {
        on_escape: dismiss.clone(),
        on_enter: dismiss,
    }
}

/// Confirm dialog. Esc / Cancel dismiss without action; Enter / primary runs `on_confirm`.
pub fn show_confirm(
    host: &GtkBox,
    card: &GtkBox,
    page: &GtkBox,
    title: &str,
    detail: &str,
    confirm_label: &str,
    on_confirm: impl FnOnce() + 'static,
) -> KeyHandlers {
    clear_card(card);

    let title_lbl = Label::new(Some(title));
    title_lbl.add_css_class("modal-title");
    title_lbl.set_halign(Align::Start);
    card.append(&title_lbl);

    let body = Label::new(Some(detail));
    body.add_css_class("modal-body");
    body.set_wrap(true);
    body.set_xalign(0.0);
    body.set_halign(Align::Fill);
    card.append(&body);

    let actions = GtkBox::new(Orientation::Horizontal, 8);
    actions.set_halign(Align::End);

    let btn_cancel = Button::with_label("Cancel");
    btn_cancel.add_css_class("ghost");
    actions.append(&btn_cancel);

    let btn_confirm = Button::with_label(confirm_label);
    btn_confirm.add_css_class("primary");
    actions.append(&btn_confirm);
    card.append(&actions);

    let on_confirm = Rc::new(std::cell::RefCell::new(Some(on_confirm)));

    let close_only: Rc<dyn Fn()> = {
        let host = host.clone();
        let page = page.clone();
        let on_confirm = on_confirm.clone();
        Rc::new(move || {
            if !host.is_visible() {
                return;
            }
            host.set_visible(false);
            page.set_sensitive(true);
            // Drop the confirm action so it cannot fire after cancel.
            let _ = on_confirm.borrow_mut().take();
        })
    };

    let confirm: Rc<dyn Fn()> = {
        let host = host.clone();
        let page = page.clone();
        let on_confirm = on_confirm.clone();
        Rc::new(move || {
            if !host.is_visible() {
                return;
            }
            host.set_visible(false);
            page.set_sensitive(true);
            if let Some(f) = on_confirm.borrow_mut().take() {
                f();
            }
        })
    };

    btn_cancel.connect_clicked({
        let close_only = close_only.clone();
        move |_| close_only()
    });
    btn_confirm.connect_clicked({
        let confirm = confirm.clone();
        move |_| confirm()
    });

    page.set_sensitive(false);
    host.set_visible(true);
    btn_confirm.grab_focus();
    KeyHandlers {
        on_escape: close_only,
        on_enter: confirm,
    }
}
