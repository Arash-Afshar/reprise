use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Align, Button, Label, Orientation, ScrolledWindow};

use crate::data::{Game, Library};

pub struct HomePage {
    pub widget: gtk4::Box,
    list: gtk4::Box,
    status: Label,
    library: Rc<RefCell<Library>>,
    on_open: Rc<dyn Fn(&Game)>,
}

impl HomePage {
    pub fn new(library: Rc<RefCell<Library>>, on_open: Rc<dyn Fn(&Game)>) -> Self {
        let widget = gtk4::Box::new(Orientation::Vertical, 18);
        widget.add_css_class("page");
        widget.set_margin_top(28);
        widget.set_margin_bottom(28);
        widget.set_margin_start(32);
        widget.set_margin_end(32);

        let eyebrow = Label::new(Some("REPRISE"));
        eyebrow.add_css_class("eyebrow");
        eyebrow.set_halign(Align::Start);

        let title = Label::new(Some("Still open"));
        title.add_css_class("title");
        title.set_halign(Align::Start);

        let blurb = Label::new(Some(
            "Not a scorecard. A short walk through the pivotal moments that decided each game.",
        ));
        blurb.add_css_class("blurb");
        blurb.set_halign(Align::Start);
        blurb.set_wrap(true);

        let status = Label::new(None);
        status.add_css_class("muted");
        status.set_halign(Align::Start);

        let list = gtk4::Box::new(Orientation::Vertical, 8);
        list.add_css_class("queue");

        let scroll = ScrolledWindow::builder()
            .child(&list)
            .vexpand(true)
            .hexpand(true)
            .build();

        widget.append(&eyebrow);
        widget.append(&title);
        widget.append(&blurb);
        widget.append(&status);
        widget.append(&scroll);

        let page = Self {
            widget,
            list,
            status,
            library,
            on_open,
        };
        page.refresh();
        page
    }

    pub fn refresh(&self) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }

        let lib = self.library.borrow();
        let open = lib.still_open(8);
        let analyzed = lib.games.iter().filter(|g| g.is_analyzed()).count();
        self.status.set_text(&format!(
            "{} games · {} analyzed · showing {} still open",
            lib.games.len(),
            analyzed,
            open.len()
        ));

        if open.is_empty() {
            let empty = Label::new(Some(
                "Nothing queued. Hit Refresh to pull new games, or open the library.",
            ));
            empty.add_css_class("muted");
            empty.set_halign(Align::Start);
            self.list.append(&empty);
            return;
        }

        for game in open {
            let row = build_queue_row(game, self.on_open.clone());
            self.list.append(&row);
        }
    }
}

fn build_queue_row(game: &Game, on_open: Rc<dyn Fn(&Game)>) -> Button {
    let pivotal_moments = game.pivotal_moments().len();
    let subtitle = if game.is_analyzed() {
        if pivotal_moments == 0 {
            "Analyzed · no pivotal moments — a quiet game".to_string()
        } else {
            format!(
                "{} · {} pivotal moment{}",
                game.outcome_for_user(),
                pivotal_moments,
                if pivotal_moments == 1 { "" } else { "s" }
            )
        }
    } else {
        "Waiting for analysis".to_string()
    };

    let when = game
        .played_at
        .map(|d| d.format("%b %d").to_string())
        .unwrap_or_else(|| "—".into());

    let head = Label::new(Some(&format!("{}  ·  {}", game.title(), when)));
    head.set_halign(Align::Start);
    head.add_css_class("row-title");

    let sub = Label::new(Some(&subtitle));
    sub.set_halign(Align::Start);
    sub.add_css_class("muted");

    let col = gtk4::Box::new(Orientation::Vertical, 2);
    col.append(&head);
    col.append(&sub);
    col.set_margin_top(8);
    col.set_margin_bottom(8);
    col.set_margin_start(4);
    col.set_margin_end(4);

    let btn = Button::new();
    btn.add_css_class("queue-row");
    btn.set_child(Some(&col));
    btn.set_halign(Align::Fill);

    let game = game.clone();
    btn.connect_clicked(move |_| on_open(&game));
    btn
}
