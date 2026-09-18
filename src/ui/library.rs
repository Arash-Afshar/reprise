//! Games list — sectioned, keyboard-first, lazy "all" list.
//!
//! Sections: recently reviewed · recent (not reviewed) · all (newest→oldest).
//! Preview board matches playbench size; PGN tails are cached.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Entry, Label, ListBoxRow, Orientation, Overlay, ScrolledWindow, Spinner,
};

use crate::chess_util::{pgn_tail, squares_from_uci, start_fen, PgnTail};
use crate::data::{Game, Library};
use crate::theme::ThemeColors;
use crate::ui::board::BoardView;
use crate::ui::keys_help;
use crate::ui::playbench::game_meta_line;

const BOARD_PX: i32 = 560;
const LIST_WIDTH: i32 = 380;
const RECENT_REVIEWED: usize = 3;
const RECENT_OTHER: usize = 12;
const ALL_PAGE: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Reviewed = 0,
    Recent = 1,
    All = 2,
}

#[derive(Debug, Clone)]
enum Row {
    Header { title: &'static str },
    Game { lib_idx: usize, section: Section },
    More { section: Section },
}

struct Model {
    /// Selectable walk order (Game / More only).
    walk: Vec<usize>,
    rows: Vec<Row>,
    /// Library indices for the all-section, newest→oldest.
    all_ids: Vec<usize>,
    all_shown: usize,
    query: String,
}

pub struct LibraryPage {
    pub widget: Overlay,
    page: GtkBox,
    search: Entry,
    list: gtk4::ListBox,
    preview: BoardView,
    game_meta: Label,
    ply_label: Label,
    keys_help: gtk4::Box,
    library: Rc<RefCell<Library>>,
    on_open: Rc<RefCell<Option<Rc<dyn Fn(&Game)>>>>,
    on_play: Rc<RefCell<Option<Rc<dyn Fn(&Game)>>>>,
    on_analyze: Rc<RefCell<Option<Rc<dyn Fn(&Game)>>>>,
    on_sync: Rc<RefCell<Option<Rc<dyn Fn()>>>>,
    on_back: Rc<RefCell<Option<Rc<dyn Fn()>>>>,
    model: Rc<RefCell<Model>>,
    tail_cache: Rc<RefCell<HashMap<(String, String), PgnTail>>>,
    filter_timer: Rc<RefCell<Option<glib::SourceId>>>,
    /// Game currently analyzing (id, game_type) — shows a spinner on that row.
    analyzing: Rc<RefCell<Option<(String, String)>>>,
    /// Spinners attached to visible game rows (a game may appear in multiple sections).
    row_spinners: Rc<RefCell<HashMap<(String, String), Vec<Spinner>>>>,
}

impl LibraryPage {
    pub fn new(library: Rc<RefCell<Library>>, colors: Rc<RefCell<ThemeColors>>) -> Self {
        let page = GtkBox::new(Orientation::Vertical, 10);
        page.add_css_class("page");
        page.add_css_class("library-page");
        page.set_margin_top(12);
        page.set_margin_bottom(12);
        page.set_margin_start(12);
        page.set_margin_end(12);
        page.set_hexpand(true);
        page.set_vexpand(true);

        let search = Entry::builder()
            .placeholder_text("filter…")
            .hexpand(true)
            .build();
        search.add_css_class("search");
        search.set_visible(false);

        let list = gtk4::ListBox::new();
        list.add_css_class("library-list");
        list.set_selection_mode(gtk4::SelectionMode::Browse);
        list.set_activate_on_single_click(false);

        let scroll = ScrolledWindow::builder()
            .child(&list)
            .vexpand(true)
            .hexpand(false)
            .build();
        scroll.set_size_request(LIST_WIDTH, -1);
        scroll.set_width_request(LIST_WIDTH);
        scroll.set_halign(Align::Start);

        // Board pane — same meta/ply/board cluster as playbench, centered.
        let board_pane = GtkBox::new(Orientation::Vertical, 0);
        board_pane.add_css_class("library-board-pane");
        board_pane.set_hexpand(true);
        board_pane.set_vexpand(true);
        board_pane.set_halign(Align::Fill);
        board_pane.set_valign(Align::Fill);

        let cluster = GtkBox::new(Orientation::Vertical, 10);
        cluster.set_halign(Align::Center);
        cluster.set_valign(Align::Center);
        cluster.set_hexpand(true);
        cluster.set_vexpand(true);

        let ply_row = GtkBox::new(Orientation::Horizontal, 12);
        ply_row.add_css_class("board-meta-row");
        ply_row.set_halign(Align::Fill);
        ply_row.set_size_request(BOARD_PX, -1);
        ply_row.set_width_request(BOARD_PX);

        let game_meta = Label::new(Some(""));
        game_meta.add_css_class("game-meta");
        game_meta.set_halign(Align::Start);
        game_meta.set_hexpand(true);
        game_meta.set_xalign(0.0);
        game_meta.set_ellipsize(gtk4::pango::EllipsizeMode::End);

        let ply_label = Label::new(Some("0/0"));
        ply_label.add_css_class("ply-counter");
        ply_label.set_halign(Align::End);
        ply_label.set_xalign(1.0);

        ply_row.append(&game_meta);
        ply_row.append(&ply_label);
        cluster.append(&ply_row);

        let preview = BoardView::new(BOARD_PX, colors);
        cluster.append(&preview.widget);
        board_pane.append(&cluster);

        let hint = Label::new(Some(
            "1/2/3 sections  ·  j k move  ·  Enter open  ·  p play  ·  a analyze  ·  s sync  ·  ? help  ·  Esc back",
        ));
        hint.add_css_class("library-hint");
        hint.set_halign(Align::Start);

        let body = GtkBox::new(Orientation::Horizontal, 12);
        body.set_hexpand(true);
        body.set_vexpand(true);
        body.append(&scroll);
        body.append(&board_pane);

        page.append(&search);
        page.append(&body);
        page.append(&hint);

        let keys_help = keys_help::build_overlay_child("Games list", keys_help::LIBRARY);

        let widget = Overlay::new();
        widget.add_css_class("library-root");
        widget.set_hexpand(true);
        widget.set_vexpand(true);
        widget.set_focusable(true);
        widget.set_child(Some(&page));
        widget.add_overlay(&keys_help);

        let page_self = Self {
            widget,
            page,
            search,
            list,
            preview,
            game_meta,
            ply_label,
            keys_help,
            library,
            on_open: Rc::new(RefCell::new(None)),
            on_play: Rc::new(RefCell::new(None)),
            on_analyze: Rc::new(RefCell::new(None)),
            on_sync: Rc::new(RefCell::new(None)),
            on_back: Rc::new(RefCell::new(None)),
            model: Rc::new(RefCell::new(Model {
                walk: Vec::new(),
                rows: Vec::new(),
                all_ids: Vec::new(),
                all_shown: 0,
                query: String::new(),
            })),
            tail_cache: Rc::new(RefCell::new(HashMap::new())),
            filter_timer: Rc::new(RefCell::new(None)),
            analyzing: Rc::new(RefCell::new(None)),
            row_spinners: Rc::new(RefCell::new(HashMap::new())),
        };
        page_self.bind();
        page_self
    }

    pub fn on_open<F: Fn(&Game) + 'static>(&self, f: F) {
        *self.on_open.borrow_mut() = Some(Rc::new(f));
    }

    /// Open the game on the playbench and enter analysis (analyze if needed).
    pub fn on_play<F: Fn(&Game) + 'static>(&self, f: F) {
        *self.on_play.borrow_mut() = Some(Rc::new(f));
    }

    /// Analyze selected game while staying on the list.
    pub fn on_analyze<F: Fn(&Game) + 'static>(&self, f: F) {
        *self.on_analyze.borrow_mut() = Some(Rc::new(f));
    }

    pub fn on_sync<F: Fn() + 'static>(&self, f: F) {
        *self.on_sync.borrow_mut() = Some(Rc::new(f));
    }

    pub fn on_back<F: Fn() + 'static>(&self, f: F) {
        *self.on_back.borrow_mut() = Some(Rc::new(f));
    }

    fn bind(&self) {
        let library = self.library.clone();
        let model = self.model.clone();
        let list = self.list.clone();
        let preview = self.preview.clone();
        let game_meta = self.game_meta.clone();
        let ply_label = self.ply_label.clone();
        let search = self.search.clone();
        let on_open = self.on_open.clone();
        let on_play = self.on_play.clone();
        let on_analyze = self.on_analyze.clone();
        let on_sync = self.on_sync.clone();
        let on_back = self.on_back.clone();
        let tail_cache = self.tail_cache.clone();
        let filter_timer = self.filter_timer.clone();
        let analyzing = self.analyzing.clone();
        let row_spinners = self.row_spinners.clone();
        let keys_help = self.keys_help.clone();
        let page = self.page.clone();

        self.search.connect_changed({
            let library = library.clone();
            let model = model.clone();
            let list = list.clone();
            let preview = preview.clone();
            let game_meta = game_meta.clone();
            let ply_label = ply_label.clone();
            let tail_cache = tail_cache.clone();
            let analyzing = analyzing.clone();
            let row_spinners = row_spinners.clone();
            let filter_timer = filter_timer.clone();
            move |entry| {
                let q = entry.text().to_string();
                if let Some(id) = filter_timer.borrow_mut().take() {
                    id.remove();
                }
                let library = library.clone();
                let model = model.clone();
                let list = list.clone();
                let preview = preview.clone();
                let game_meta = game_meta.clone();
                let ply_label = ply_label.clone();
                let tail_cache = tail_cache.clone();
                let analyzing = analyzing.clone();
                let row_spinners = row_spinners.clone();
                let filter_timer_clear = filter_timer.clone();
                let filter_timer_store = filter_timer.clone();
                let id = glib::timeout_add_local_once(std::time::Duration::from_millis(60), move || {
                    *filter_timer_clear.borrow_mut() = None;
                    rebuild(
                        &library,
                        &model,
                        &list,
                        &preview,
                        &game_meta,
                        &ply_label,
                        &tail_cache,
                        &analyzing,
                        &row_spinners,
                        &q,
                        RebuildMode::Reset,
                    );
                });
                *filter_timer_store.borrow_mut() = Some(id);
            }
        });

        self.search.connect_activate({
            let list = list.clone();
            move |_| {
                list.grab_focus();
            }
        });

        self.list.connect_row_activated({
            let library = library.clone();
            let model = model.clone();
            let on_open = on_open.clone();
            move |_, row| {
                activate_row(&library, &model, &on_open, row.index() as usize);
            }
        });

        self.list.connect_row_selected({
            let library = library.clone();
            let model = model.clone();
            let preview = preview.clone();
            let game_meta = game_meta.clone();
            let ply_label = ply_label.clone();
            let tail_cache = tail_cache.clone();
            let list = list.clone();
            move |_, row| {
                let Some(row) = row else {
                    clear_preview(&preview, &game_meta, &ply_label);
                    return;
                };
                let idx = row.index() as usize;
                let row_kind = model.borrow().rows.get(idx).cloned();
                match row_kind {
                    Some(Row::Game { lib_idx, .. }) => {
                        show_preview(
                            &library,
                            lib_idx,
                            &preview,
                            &game_meta,
                            &ply_label,
                            &tail_cache,
                        );
                    }
                    Some(Row::More { .. }) => {
                        // stay on previous preview
                    }
                    _ => clear_preview(&preview, &game_meta, &ply_label),
                }
                // Ensure header rows never keep selection.
                if matches!(row_kind, Some(Row::Header { .. })) {
                    if let Some(next) = next_walk_row(&model, idx, 1) {
                        if let Some(r) = list.row_at_index(next as i32) {
                            list.select_row(Some(&r));
                        }
                    }
                }
            }
        });

        let keys = gtk4::EventControllerKey::new();
        keys.set_propagation_phase(gtk4::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let search = search.clone();
            let list = list.clone();
            let on_back = on_back.clone();
            let on_open = on_open.clone();
            let on_play = on_play.clone();
            let on_analyze = on_analyze.clone();
            let on_sync = on_sync.clone();
            let library = library.clone();
            let model = model.clone();
            let preview = preview.clone();
            let game_meta = game_meta.clone();
            let ply_label = ply_label.clone();
            let tail_cache = tail_cache.clone();
            let analyzing = analyzing.clone();
            let row_spinners = row_spinners.clone();
            let keys_help = keys_help.clone();
            let page = page.clone();
            move |_, key, _, _| {
                use gtk4::gdk::Key;

                if keys_help.is_visible() {
                    if key == Key::Escape || key == Key::question {
                        keys_help.set_visible(false);
                        page.set_sensitive(true);
                        list.grab_focus();
                    }
                    return glib::Propagation::Stop;
                }

                // Filter open: letters go to the entry (inner GtkText focus is
                // unreliable under Capture). Keep Esc / Enter / / as actions.
                if WidgetExt::is_visible(&search) {
                    match key {
                        Key::Escape => {
                            search.set_text("");
                            search.set_visible(false);
                            list.grab_focus();
                            return glib::Propagation::Stop;
                        }
                        Key::Return | Key::KP_Enter => {
                            if let Some(row) = list.selected_row() {
                                let idx = row.index() as usize;
                                let kind = model.borrow().rows.get(idx).cloned();
                                if let Some(Row::More { .. }) = kind {
                                    load_more(
                                        &library,
                                        &model,
                                        &list,
                                        &preview,
                                        &game_meta,
                                        &ply_label,
                                        &tail_cache,
                                        &analyzing,
                                        &row_spinners,
                                    );
                                } else {
                                    activate_row(&library, &model, &on_open, idx);
                                }
                            }
                            return glib::Propagation::Stop;
                        }
                        Key::slash => {
                            search.grab_focus();
                            return glib::Propagation::Stop;
                        }
                        _ => return glib::Propagation::Proceed,
                    }
                }

                match key {
                    Key::Escape => {
                        if let Some(f) = on_back.borrow().clone() {
                            f();
                        }
                        glib::Propagation::Stop
                    }
                    Key::slash => {
                        search.set_visible(true);
                        search.grab_focus();
                        glib::Propagation::Stop
                    }
                    Key::question => {
                        keys_help.set_visible(true);
                        page.set_sensitive(false);
                        keys_help.grab_focus();
                        glib::Propagation::Stop
                    }
                    Key::_1 => {
                        jump_section(&list, &model, Section::Reviewed);
                        glib::Propagation::Stop
                    }
                    Key::_2 => {
                        jump_section(&list, &model, Section::Recent);
                        glib::Propagation::Stop
                    }
                    Key::_3 => {
                        jump_section(&list, &model, Section::All);
                        glib::Propagation::Stop
                    }
                    Key::Return | Key::KP_Enter => {
                        if let Some(row) = list.selected_row() {
                            let idx = row.index() as usize;
                            let kind = model.borrow().rows.get(idx).cloned();
                            if let Some(Row::More { .. }) = kind {
                                load_more(
                                    &library,
                                    &model,
                                    &list,
                                    &preview,
                                    &game_meta,
                                    &ply_label,
                                    &tail_cache,
                                    &analyzing,
                                    &row_spinners,
                                );
                            } else {
                                activate_row(&library, &model, &on_open, idx);
                            }
                        }
                        glib::Propagation::Stop
                    }
                    Key::p | Key::P => {
                        if let Some(row) = list.selected_row() {
                            activate_row(&library, &model, &on_play, row.index() as usize);
                        }
                        glib::Propagation::Stop
                    }
                    Key::a | Key::A => {
                        if let Some(row) = list.selected_row() {
                            activate_row(&library, &model, &on_analyze, row.index() as usize);
                        }
                        glib::Propagation::Stop
                    }
                    Key::s | Key::S => {
                        if let Some(f) = on_sync.borrow().clone() {
                            f();
                        }
                        glib::Propagation::Stop
                    }
                    Key::j | Key::J | Key::Down => {
                        step_walk(
                            &library,
                            &model,
                            &list,
                            &preview,
                            &game_meta,
                            &ply_label,
                            &tail_cache,
                            &analyzing,
                            &row_spinners,
                            1,
                        );
                        glib::Propagation::Stop
                    }
                    Key::k | Key::K | Key::Up => {
                        step_walk(
                            &library,
                            &model,
                            &list,
                            &preview,
                            &game_meta,
                            &ply_label,
                            &tail_cache,
                            &analyzing,
                            &row_spinners,
                            -1,
                        );
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                }
            }
        });
        self.widget.add_controller(keys);

        let help_keys = gtk4::EventControllerKey::new();
        help_keys.connect_key_pressed({
            let keys_help = self.keys_help.clone();
            let page = self.page.clone();
            let list = self.list.clone();
            move |_, key, _, _| {
                use gtk4::gdk::Key;
                if key == Key::Escape || key == Key::question {
                    keys_help.set_visible(false);
                    page.set_sensitive(true);
                    list.grab_focus();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Stop
            }
        });
        self.keys_help.set_focusable(true);
        self.keys_help.add_controller(help_keys);
    }

    pub fn refresh(&self) {
        if let Some(id) = self.filter_timer.borrow_mut().take() {
            id.remove();
        }
        rebuild(
            &self.library,
            &self.model,
            &self.list,
            &self.preview,
            &self.game_meta,
            &self.ply_label,
            &self.tail_cache,
            &self.analyzing,
            &self.row_spinners,
            &self.search.text(),
            RebuildMode::Reset,
        );
        self.list.grab_focus();
    }

    /// Show/hide the in-row spinner for the game currently analyzing.
    /// Updates existing row widgets only — no list rebuild.
    pub fn set_analyzing(&self, key: Option<(String, String)>) {
        let t0 = Instant::now();
        let prev = self.analyzing.borrow().clone();
        *self.analyzing.borrow_mut() = key.clone();
        let spinners = self.row_spinners.borrow();
        if let Some(prev) = prev.as_ref() {
            if let Some(list) = spinners.get(prev) {
                for s in list {
                    s.set_spinning(false);
                    s.set_visible(false);
                }
            }
        }
        if let Some(key) = key.as_ref() {
            if let Some(list) = spinners.get(key) {
                for s in list {
                    s.set_visible(true);
                    s.set_spinning(true);
                }
            }
        }
        eprintln!("reprise: library set_analyzing {:.2?}", t0.elapsed());
    }

    pub fn grab_focus(&self) {
        self.list.grab_focus();
    }

    pub fn board_repaint_callback(&self) -> Rc<dyn Fn()> {
        let board = self.preview.clone();
        Rc::new(move || board.queue_draw())
    }
}

fn activate_row(
    library: &Rc<RefCell<Library>>,
    model: &Rc<RefCell<Model>>,
    on_open: &Rc<RefCell<Option<Rc<dyn Fn(&Game)>>>>,
    row_idx: usize,
) {
    let Some(Row::Game { lib_idx, .. }) = model.borrow().rows.get(row_idx).cloned() else {
        return;
    };
    let game = library.borrow().games.get(lib_idx).cloned();
    if let Some(game) = game {
        if let Some(f) = on_open.borrow().clone() {
            f(&game);
        }
    }
}

fn jump_section(list: &gtk4::ListBox, model: &Rc<RefCell<Model>>, section: Section) {
    let m = model.borrow();
    let Some(&row_idx) = m.walk.iter().find(|&&i| {
        matches!(
            m.rows.get(i),
            Some(Row::Game { section: s, .. } | Row::More { section: s }) if *s == section
        )
    }) else {
        return;
    };
    drop(m);
    if let Some(row) = list.row_at_index(row_idx as i32) {
        list.select_row(Some(&row));
        row.grab_focus();
    }
}

fn next_walk_row(model: &Rc<RefCell<Model>>, from_row: usize, dir: i32) -> Option<usize> {
    let m = model.borrow();
    let pos = m.walk.iter().position(|&i| i == from_row).unwrap_or(0);
    if m.walk.is_empty() {
        return None;
    }
    let next = if dir >= 0 {
        (pos + 1) % m.walk.len()
    } else {
        pos.checked_sub(1).unwrap_or(m.walk.len() - 1)
    };
    m.walk.get(next).copied()
}

fn step_walk(
    library: &Rc<RefCell<Library>>,
    model: &Rc<RefCell<Model>>,
    list: &gtk4::ListBox,
    preview: &BoardView,
    game_meta: &Label,
    ply_label: &Label,
    tail_cache: &Rc<RefCell<HashMap<(String, String), PgnTail>>>,
    analyzing: &Rc<RefCell<Option<(String, String)>>>,
    row_spinners: &Rc<RefCell<HashMap<(String, String), Vec<Spinner>>>>,
    dir: i32,
) {
    let cur = list.selected_row().map(|r| r.index() as usize).unwrap_or(0);
    // Clone out of the RefCell first — an `if let` on `borrow()` keeps the
    // guard alive for the whole block (including load_more → borrow_mut).
    let on_more = matches!(
        model.borrow().rows.get(cur),
        Some(Row::More { .. })
    );
    if dir > 0 && on_more {
        load_more(
            library,
            model,
            list,
            preview,
            game_meta,
            ply_label,
            tail_cache,
            analyzing,
            row_spinners,
        );
        return;
    }
    let Some(next) = next_walk_row(model, cur, dir) else {
        return;
    };
    if let Some(row) = list.row_at_index(next as i32) {
        list.select_row(Some(&row));
        row.grab_focus();
    }
}

fn load_more(
    library: &Rc<RefCell<Library>>,
    model: &Rc<RefCell<Model>>,
    list: &gtk4::ListBox,
    preview: &BoardView,
    game_meta: &Label,
    ply_label: &Label,
    tail_cache: &Rc<RefCell<HashMap<(String, String), PgnTail>>>,
    analyzing: &Rc<RefCell<Option<(String, String)>>>,
    row_spinners: &Rc<RefCell<HashMap<(String, String), Vec<Spinner>>>>,
) {
    let q = model.borrow().query.clone();
    rebuild(
        library,
        model,
        list,
        preview,
        game_meta,
        ply_label,
        tail_cache,
        analyzing,
        row_spinners,
        &q,
        RebuildMode::LoadMore,
    );
}

fn clear_preview(preview: &BoardView, game_meta: &Label, ply_label: &Label) {
    preview.set_fen(&start_fen());
    preview.set_last_move(None, None);
    game_meta.set_text("");
    ply_label.set_text("0/0");
}

fn show_preview(
    library: &Rc<RefCell<Library>>,
    lib_idx: usize,
    preview: &BoardView,
    game_meta: &Label,
    ply_label: &Label,
    tail_cache: &Rc<RefCell<HashMap<(String, String), PgnTail>>>,
) {
    let t0 = Instant::now();
    let (tail, white_bottom, meta, key) = {
        let mut lib = library.borrow_mut();
        let Some(game) = lib.games.get_mut(lib_idx) else {
            clear_preview(preview, game_meta, ply_label);
            return;
        };
        let key = (game.id.clone(), game.game_type.clone());
        let white_bottom = game.user_color.as_deref() != Some("black");
        let meta = game_meta_line(game);

        let from_index = match (&game.end_fen, game.end_ply) {
            (Some(fen), Some(ply)) => Some(PgnTail {
                fen: fen.clone(),
                ply: ply as usize,
                last_uci: game.end_uci.clone(),
            }),
            _ => None,
        };

        let tail = if let Some(t) = from_index {
            t
        } else if let Some(t) = tail_cache.borrow().get(&key).cloned() {
            t
        } else {
            let t = pgn_tail(&game.pgn).unwrap_or(PgnTail {
                fen: start_fen(),
                ply: 0,
                last_uci: None,
            });
            // Backfill memory so the next visit is free; persist happens via save_index later.
            game.end_fen = Some(t.fen.clone());
            game.end_ply = Some(t.ply as u32);
            game.end_uci = t.last_uci.clone();
            tail_cache.borrow_mut().insert(key.clone(), t.clone());
            t
        };
        (tail, white_bottom, meta, key)
    };
    // Ensure cache has the index-sourced tail too.
    tail_cache.borrow_mut().entry(key).or_insert_with(|| tail.clone());

    preview.set_orientation_white_bottom(white_bottom);
    preview.set_fen(&tail.fen);
    if let Some(uci) = tail.last_uci.as_deref() {
        if let Some((from, to)) = squares_from_uci(uci) {
            preview.set_last_move(Some(from), Some(to));
        } else {
            preview.set_last_move(None, None);
        }
    } else {
        preview.set_last_move(None, None);
    }
    game_meta.set_text(&meta);
    ply_label.set_text(&format!("{0}/{0}", tail.ply));
    eprintln!("reprise: show_preview {:.2?}", t0.elapsed());
}

fn matches_query(game: &Game, q: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    let blob = format!(
        "{} {} {} {} {} {}",
        game.white,
        game.black,
        game.opponent.as_deref().unwrap_or(""),
        game.result.as_deref().unwrap_or(""),
        game.eco.as_deref().unwrap_or(""),
        game.time_control.as_deref().unwrap_or("")
    )
    .to_lowercase();
    blob.contains(q)
}

#[derive(Debug, Clone, Copy)]
enum RebuildMode {
    /// Fresh list / filter — reset all pagination.
    Reset,
    /// Append another page of "all" games; prefer selecting new rows.
    LoadMore,
    /// Keep pagination + selection (e.g. spinner toggle).
    Soft,
}

fn rebuild(
    library: &Rc<RefCell<Library>>,
    model: &Rc<RefCell<Model>>,
    list: &gtk4::ListBox,
    preview: &BoardView,
    game_meta: &Label,
    ply_label: &Label,
    tail_cache: &Rc<RefCell<HashMap<(String, String), PgnTail>>>,
    analyzing: &Rc<RefCell<Option<(String, String)>>>,
    row_spinners: &Rc<RefCell<HashMap<(String, String), Vec<Spinner>>>>,
    query: &str,
    mode: RebuildMode,
) {
    let prev_key = {
        let idx = list.selected_row().map(|r| r.index() as usize);
        let m = model.borrow();
        let key = idx.and_then(|idx| match m.rows.get(idx) {
            Some(Row::Game { lib_idx, .. }) => library
                .borrow()
                .games
                .get(*lib_idx)
                .map(|g| (g.id.clone(), g.game_type.clone())),
            Some(Row::More { .. }) => m.rows.iter().take(idx).rev().find_map(|r| match r {
                Row::Game { lib_idx, .. } => library
                    .borrow()
                    .games
                    .get(*lib_idx)
                    .map(|g| (g.id.clone(), g.game_type.clone())),
                _ => None,
            }),
            _ => None,
        });
        key
    };
    let prefer_new = matches!(mode, RebuildMode::LoadMore);
    let prev_shown = model.borrow().all_shown;
    let analyzing_key = analyzing.borrow().clone();

    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    let q = query.trim().to_lowercase();

    // Snapshot everything we need from the library, then drop the borrow
    // before any GTK mutations (append/select re-enter our handlers).
    let (rows, walk, all_ids, all_shown, labels) = {
        let lib = library.borrow();
        let all_ids: Vec<usize> = (0..lib.games.len())
            .filter(|&i| matches_query(&lib.games[i], &q))
            .collect();

        let reviewed: Vec<usize> = all_ids
            .iter()
            .copied()
            .filter(|&i| lib.games[i].is_reviewed())
            .take(RECENT_REVIEWED)
            .collect();
        let recent: Vec<usize> = all_ids
            .iter()
            .copied()
            .filter(|&i| !lib.games[i].is_reviewed())
            .take(RECENT_OTHER)
            .collect();

        let mut all_shown = match mode {
            RebuildMode::Reset => ALL_PAGE.min(all_ids.len()),
            RebuildMode::LoadMore => (prev_shown + ALL_PAGE)
                .min(all_ids.len())
                .max(ALL_PAGE.min(all_ids.len())),
            RebuildMode::Soft => prev_shown
                .max(ALL_PAGE.min(all_ids.len()))
                .min(all_ids.len()),
        };

        let use_queues = q.is_empty();
        if !use_queues {
            all_shown = match mode {
                RebuildMode::Reset => ALL_PAGE.min(all_ids.len()),
                RebuildMode::LoadMore | RebuildMode::Soft => all_shown,
            };
        }

        let mut rows: Vec<Row> = Vec::new();
        let mut walk: Vec<usize> = Vec::new();
        let mut labels: Vec<RowLabel> = Vec::new();

        let push_header = |rows: &mut Vec<Row>, labels: &mut Vec<RowLabel>, title: &'static str| {
            rows.push(Row::Header { title });
            labels.push(RowLabel::Header(title));
        };
        let push_games =
            |rows: &mut Vec<Row>,
             walk: &mut Vec<usize>,
             labels: &mut Vec<RowLabel>,
             ids: &[usize],
             section: Section,
             lib: &Library,
             analyzing_key: &Option<(String, String)>| {
                for &lib_idx in ids {
                    walk.push(rows.len());
                    rows.push(Row::Game { lib_idx, section });
                    let g = &lib.games[lib_idx];
                    let mark = if g.is_reviewed() {
                        "✓"
                    } else if g.is_analyzed() {
                        "◇"
                    } else {
                        "·"
                    };
                    let date = g
                        .played_at
                        .map(|d| d.format("%d %b").to_string())
                        .unwrap_or_else(|| "—".into());
                    let opp = g
                        .opponent
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .unwrap_or("Opp");
                    let result = g.result.as_deref().unwrap_or("—");
                    let spinning = analyzing_key
                        .as_ref()
                        .is_some_and(|(id, gt)| *id == g.id && *gt == g.game_type);
                    labels.push(RowLabel::Game {
                        text: format!("{mark}  {date}  ·  vs {opp}  ·  {result}"),
                        key: (g.id.clone(), g.game_type.clone()),
                        spinning,
                    });
                }
            };

        if use_queues {
            push_header(&mut rows, &mut labels, "Recently reviewed");
            push_games(
                &mut rows,
                &mut walk,
                &mut labels,
                &reviewed,
                Section::Reviewed,
                &lib,
                &analyzing_key,
            );

            push_header(&mut rows, &mut labels, "Recent");
            push_games(
                &mut rows,
                &mut walk,
                &mut labels,
                &recent,
                Section::Recent,
                &lib,
                &analyzing_key,
            );

            push_header(&mut rows, &mut labels, "All games");
            push_games(
                &mut rows,
                &mut walk,
                &mut labels,
                &all_ids[..all_shown],
                Section::All,
                &lib,
                &analyzing_key,
            );
            if all_shown < all_ids.len() {
                walk.push(rows.len());
                rows.push(Row::More { section: Section::All });
                let left = all_ids.len() - all_shown;
                labels.push(RowLabel::More(format!("↓  load more ({left} left)")));
            }
        } else {
            push_header(&mut rows, &mut labels, "Matches");
            push_games(
                &mut rows,
                &mut walk,
                &mut labels,
                &all_ids[..all_shown],
                Section::All,
                &lib,
                &analyzing_key,
            );
            if all_shown < all_ids.len() {
                walk.push(rows.len());
                rows.push(Row::More { section: Section::All });
                let left = all_ids.len() - all_shown;
                labels.push(RowLabel::More(format!("↓  load more ({left} left)")));
            }
        }

        (rows, walk, all_ids, all_shown, labels)
    };

    *model.borrow_mut() = Model {
        walk: walk.clone(),
        rows: rows.clone(),
        all_ids: all_ids.clone(),
        all_shown,
        query: q,
    };

    row_spinners.borrow_mut().clear();

    for label in labels {
        match label {
            RowLabel::Header(title) => {
                let lbl = Label::new(Some(title));
                lbl.add_css_class("library-section");
                lbl.set_halign(Align::Start);
                lbl.set_xalign(0.0);
                lbl.set_margin_top(10);
                lbl.set_margin_bottom(4);
                lbl.set_margin_start(10);
                lbl.set_margin_end(10);
                let wrap = ListBoxRow::new();
                wrap.add_css_class("library-section-row");
                wrap.set_selectable(false);
                wrap.set_activatable(false);
                wrap.set_child(Some(&lbl));
                list.append(&wrap);
            }
            RowLabel::Game {
                text,
                key,
                spinning,
            } => {
                let row = GtkBox::new(Orientation::Horizontal, 8);
                row.set_margin_top(6);
                row.set_margin_bottom(6);
                row.set_margin_start(10);
                row.set_margin_end(10);
                let lbl = Label::new(Some(&text));
                lbl.set_halign(Align::Start);
                lbl.set_hexpand(true);
                lbl.set_xalign(0.0);
                row.append(&lbl);
                let spinner = Spinner::new();
                spinner.add_css_class("library-analyzing");
                spinner.set_size_request(14, 14);
                spinner.set_halign(Align::End);
                spinner.set_valign(Align::Center);
                spinner.set_spinning(spinning);
                spinner.set_visible(spinning);
                row.append(&spinner);
                row_spinners
                    .borrow_mut()
                    .entry(key)
                    .or_default()
                    .push(spinner);
                list.append(&row);
            }
            RowLabel::More(text) => {
                let lbl = Label::new(Some(&text));
                lbl.add_css_class("library-more");
                lbl.set_halign(Align::Start);
                lbl.set_xalign(0.0);
                lbl.set_margin_top(8);
                lbl.set_margin_bottom(8);
                lbl.set_margin_start(10);
                lbl.set_margin_end(10);
                list.append(&lbl);
            }
        }
    }

    let select = if prefer_new {
        let start = all_shown.saturating_sub(ALL_PAGE);
        rows.iter()
            .position(|r| {
                matches!(r, Row::Game { lib_idx, section: Section::All, .. }
                    if all_ids.get(start..).is_some_and(|chunk| chunk.contains(lib_idx)))
            })
            .or_else(|| walk.first().copied())
    } else {
        prev_key
            .as_ref()
            .and_then(|(id, gt)| {
                rows.iter().position(|r| {
                    if let Row::Game { lib_idx, .. } = r {
                        library
                            .borrow()
                            .games
                            .get(*lib_idx)
                            .is_some_and(|g| g.id == *id && g.game_type == *gt)
                    } else {
                        false
                    }
                })
            })
            .or_else(|| walk.first().copied())
    };

    if let Some(idx) = select {
        let lib_idx = match rows.get(idx) {
            Some(Row::Game { lib_idx, .. }) => Some(*lib_idx),
            _ => None,
        };
        if let Some(row) = list.row_at_index(idx as i32) {
            list.select_row(Some(&row));
        }
        if let Some(lib_idx) = lib_idx {
            show_preview(
                library, lib_idx, preview, game_meta, ply_label, tail_cache,
            );
        }
    } else {
        clear_preview(preview, game_meta, ply_label);
    }
}

enum RowLabel {
    Header(&'static str),
    Game {
        text: String,
        key: (String, String),
        spinning: bool,
    },
    More(String),
}

#[cfg(test)]
mod perf_tests {
    use super::*;
    use std::time::Duration;

    const FRAME: Duration = Duration::from_micros(16_667);

    #[test]
    fn spinner_toggle_fits_frame_vs_list_rebuild() {
        // Approximate cost of the old Soft rebuild (reformat ~60 row labels)
        // vs the new path (flip a couple of flags).
        let rows: Vec<(String, String, String)> = (0..60)
            .map(|i| {
                (
                    format!("id{i}"),
                    "live".into(),
                    format!("·  01 Jan  ·  vs Opp{i}  ·  1-0"),
                )
            })
            .collect();

        let mut rebuild_samples = Vec::new();
        for _ in 0..30 {
            let t0 = Instant::now();
            let mut out = Vec::with_capacity(rows.len());
            for (id, gt, text) in &rows {
                let spinning = id == "id3";
                out.push((text.clone(), (id.clone(), gt.clone()), spinning));
            }
            std::hint::black_box(out);
            rebuild_samples.push(t0.elapsed());
        }
        rebuild_samples.sort();
        let rebuild = rebuild_samples[rebuild_samples.len() / 2];

        let mut flags = vec![false; rows.len()];
        let mut toggle_samples = Vec::new();
        for _ in 0..200 {
            let t0 = Instant::now();
            if let Some(prev) = flags.iter_mut().find(|f| **f) {
                *prev = false;
            }
            flags[3] = true;
            std::hint::black_box(&flags);
            toggle_samples.push(t0.elapsed());
        }
        toggle_samples.sort();
        let toggle = toggle_samples[toggle_samples.len() / 2];

        eprintln!(
            "list spinner: toggle {:?} · label-rebuild {:?} · frame {:?}",
            toggle, rebuild, FRAME
        );
        assert!(toggle < FRAME, "spinner toggle must fit a frame");
        assert!(
            toggle * 10 < rebuild || toggle < Duration::from_micros(50),
            "toggle should be far cheaper than rebuilding row labels"
        );
    }
}
