use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Align, Button, Label, Orientation, Stack, StackTransitionType};

use crate::data::{Game, Library};
use crate::theme::{ThemeColors, ThemeHandle};
use crate::ui::home::HomePage;
use crate::ui::library::LibraryPage;
use crate::ui::review::ReviewPage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    Home,
    Library,
    Review,
}

pub struct Shell {
    pub widget: gtk4::Box,
    pub stack: Stack,
    pub home: HomePage,
    pub library: LibraryPage,
    pub review: ReviewPage,
    sync_status: Label,
}

impl Shell {
    pub fn new(
        library: Rc<RefCell<Library>>,
        on_open_game: Rc<dyn Fn(&Game)>,
        theme: &Rc<ThemeHandle>,
    ) -> Self {
        let colors: Rc<RefCell<ThemeColors>> = theme.colors.clone();

        let widget = gtk4::Box::new(Orientation::Vertical, 0);
        widget.add_css_class("shell");

        let top = gtk4::Box::new(Orientation::Horizontal, 10);
        top.add_css_class("topbar");
        top.set_margin_top(10);
        top.set_margin_bottom(6);
        top.set_margin_start(16);
        top.set_margin_end(16);

        let brand = Label::new(Some("Reprise"));
        brand.add_css_class("brand");
        brand.set_halign(Align::Start);

        let nav_home = Button::with_label("Still open");
        let nav_lib = Button::with_label("Library");
        for b in [&nav_home, &nav_lib] {
            b.add_css_class("nav");
        }

        let sync = Button::with_label("Refresh");
        sync.add_css_class("primary");
        sync.set_tooltip_text(Some(
            "Pull new games and analyze as needed (wired in a later chunk)",
        ));

        let sync_status = Label::new(Some("Local library"));
        sync_status.add_css_class("muted");
        sync_status.set_halign(Align::End);
        sync_status.set_hexpand(true);

        top.append(&brand);
        top.append(&nav_home);
        top.append(&nav_lib);
        top.append(&sync_status);
        top.append(&sync);

        let stack = Stack::new();
        stack.set_transition_type(StackTransitionType::Crossfade);
        stack.set_vexpand(true);
        stack.set_hexpand(true);

        let home = HomePage::new(library.clone(), on_open_game.clone());
        let library_page = LibraryPage::new(library.clone(), on_open_game.clone(), colors.clone());
        let review = ReviewPage::new(colors);

        stack.add_named(&home.widget, Some("home"));
        stack.add_named(&library_page.widget, Some("library"));
        stack.add_named(&review.widget, Some("review"));
        stack.set_visible_child_name("home");

        nav_home.connect_clicked({
            let stack = stack.clone();
            move |_| {
                stack.set_visible_child_name("home");
            }
        });
        nav_lib.connect_clicked({
            let stack = stack.clone();
            move |_| {
                stack.set_visible_child_name("library");
            }
        });
        sync.connect_clicked({
            let sync_status = sync_status.clone();
            move |_| {
                sync_status.set_text("Refresh not hooked up yet — next chunk.");
            }
        });

        widget.append(&top);
        widget.append(&stack);

        let shell = Self {
            widget,
            stack,
            home,
            library: library_page,
            review,
            sync_status,
        };

        let lib_cb = shell.library.board_repaint_callback();
        let review_cb = shell.review.board_repaint_callback();
        theme.on_change(move || {
            lib_cb();
            review_cb();
        });

        shell
    }

    pub fn show(&self, nav: Nav) {
        let name = match nav {
            Nav::Home => "home",
            Nav::Library => "library",
            Nav::Review => "review",
        };
        self.stack.set_visible_child_name(name);
    }

    pub fn set_sync_status(&self, text: &str) {
        self.sync_status.set_text(text);
    }
}
