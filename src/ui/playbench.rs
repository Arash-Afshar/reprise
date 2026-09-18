//! Board playback + pivotal-moment walk.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Result;
use gtk4::prelude::*;
use gtk4::{Align, Label, Orientation, Overlay};

use crate::chess_util::{
    apply_san_line_with_ucis, pgn_sans, squares_from_uci, start_fen,
};
use crate::data::{
    load_game_analysis, persist_reviewed_flag_async, Game, GameAnalysis, Library,
};
use crate::theme::ThemeColors;
use crate::ui::board::BoardView;
use crate::ui::keys_help;
use crate::ui::modal;
use crate::weekly_review;

struct AnalysisLoadMsg {
    load_gen: u64,
    /// `status_gen` when "Loading analysis…" was shown; ignore clear if superseded.
    status_token: u64,
    id: String,
    game_type: String,
    result: Result<Option<GameAnalysis>>,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncUiState {
    /// Idle and current — show checkmark.
    UpToDate,
    /// Click to re-sync — show refresh arrow.
    NeedsRefresh,
    /// Sync in flight.
    Syncing,
}

#[derive(Clone)]
struct PlyInsight {
    ply: usize,
    san: String,
    mine: bool,
    classification: String,
    loss_cp: Option<i32>,
    eval_played: Option<String>,
    eval_best_before: Option<String>,
    best_san: Option<String>,
    best_eval: Option<String>,
    best_uci: Option<String>,
    /// Display string for moment copy.
    best_pv: Option<String>,
    played_continuation: Option<String>,
    /// Raw SANs + FENs for vb/vp — kept on the insight so enter never
    /// re-queries library analysis (which can be absent after navigations).
    fen_before: String,
    fen_after: String,
    best_pv_sans: Option<Vec<String>>,
    punished_pv_sans: Option<Vec<String>>,
    summary: Option<String>,
    why_played: Option<String>,
    why_best: Option<String>,
}

struct Playback {
    fens: Vec<String>,
    ucis: Vec<Option<String>>,
    ply: usize,
    /// Per-move analysis keyed by ply (1 = after White's first move).
    insights: Vec<PlyInsight>,
    /// Ply numbers of user pivotal moments, for n/p navigation.
    pivotal_moments: Vec<usize>,
    pivotal_idx: Option<usize>,
    white_at_bottom: bool,
    /// Dual-board line step-through; `None` = main game only.
    variation: Option<VariationPlayback>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VariationKind {
    Best,
    Punished,
}

struct VariationPlayback {
    kind: VariationKind,
    fens: Vec<String>,
    ucis: Vec<Option<String>>,
    ply: usize,
}

impl VariationKind {
    fn title(self) -> &'static str {
        match self {
            VariationKind::Best => "Best line",
            VariationKind::Punished => "How it is punished",
        }
    }
}

impl Playback {
    fn in_variation(&self) -> bool {
        self.variation.is_some()
    }
}

pub struct Playbench {
    pub widget: Overlay,
    content: gtk4::Box,
    board: BoardView,
    variation_board: BoardView,
    moment_head: Label,
    moment_body: Label,
    pivotal_panel: gtk4::Box,
    game_meta: Label,
    ply_label: Label,
    keys_help: gtk4::Box,
    modal_root: gtk4::Box,
    modal_card: gtk4::Box,
    /// Esc/Enter handlers for the active modal, if any.
    modal_keys: Rc<RefCell<Option<modal::KeyHandlers>>>,
    /// Registered Analyze action (`a` key).
    analyze_action: Rc<RefCell<Option<Rc<dyn Fn()>>>>,
    /// Shared Analyze request (confirms before reanalyze).
    request_analyze: Rc<dyn Fn()>,
    /// Registered Analyze-all action (kept for app wiring; no chrome button).
    analyze_all_action: Rc<RefCell<Option<Rc<dyn Fn()>>>>,
    /// Registered Sync action (`s` key).
    sync_action: Rc<RefCell<Option<Rc<dyn Fn()>>>>,
    /// Open the games list (`o` key).
    open_library_action: Rc<RefCell<Option<Rc<dyn Fn()>>>>,
    /// Open the progress dashboard (`d` key).
    open_progress_action: Rc<RefCell<Option<Rc<dyn Fn()>>>>,
    analyzing_badge: Label,
    /// Game currently being analyzed in the background, if any.
    analyzing: Rc<RefCell<Option<(String, String)>>>,
    /// Bumped on every `refresh_board` to drop stale analysis loads.
    analysis_load_gen: Rc<Cell<u64>>,
    analysis_tx: async_channel::Sender<AnalysisLoadMsg>,
    /// Status badge generation — ephemeral clears only match the latest.
    status_gen: Rc<Cell<u64>>,
    status_timer: Rc<RefCell<Option<glib::SourceId>>>,
    library: Rc<RefCell<Library>>,
    state: Rc<RefCell<Option<Playback>>>,
    /// Exit dual-board variation view (Esc).
    exit_variation: Rc<RefCell<Option<Rc<dyn Fn()>>>>,
    current: Rc<RefCell<Option<(String, String)>>>,
    busy: Rc<RefCell<bool>>,
    sync_state: Rc<RefCell<SyncUiState>>,
}

impl Playbench {
    pub fn new(library: Rc<RefCell<Library>>, colors: Rc<RefCell<ThemeColors>>) -> Self {
        let overlay = Overlay::new();
        overlay.add_css_class("playbench");
        overlay.set_hexpand(true);
        overlay.set_vexpand(true);

        let content = gtk4::Box::new(Orientation::Vertical, 12);
        content.add_css_class("page");
        content.set_margin_top(12);
        content.set_margin_bottom(16);
        content.set_margin_start(16);
        content.set_margin_end(16);
        content.set_hexpand(true);
        content.set_vexpand(true);

        const BOARD_PX: i32 = 560;

        // Stage: boards share a row; pivotal copy sits in the top gap.
        let stage = Overlay::new();
        stage.add_css_class("playbench-stage");
        stage.set_hexpand(true);
        stage.set_vexpand(true);

        let boards_row = gtk4::Box::new(Orientation::Horizontal, 56);
        boards_row.add_css_class("boards-row");
        boards_row.set_halign(Align::Center);
        boards_row.set_valign(Align::Center);
        boards_row.set_hexpand(true);
        boards_row.set_vexpand(true);

        let main_cluster = gtk4::Box::new(Orientation::Vertical, 10);
        main_cluster.add_css_class("main-cluster");
        main_cluster.set_halign(Align::Center);
        main_cluster.set_valign(Align::Start);
        main_cluster.set_hexpand(false);
        main_cluster.set_vexpand(false);

        // Meta (left) + ply (right) sit immediately above the board.
        let ply_row = gtk4::Box::new(Orientation::Horizontal, 12);
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
        ply_label.set_hexpand(false);
        ply_label.set_xalign(1.0);

        ply_row.append(&game_meta);
        ply_row.append(&ply_label);
        main_cluster.append(&ply_row);

        let board = BoardView::new(BOARD_PX, colors.clone());
        main_cluster.append(&board.widget);

        let variation_cluster = gtk4::Box::new(Orientation::Vertical, 10);
        variation_cluster.add_css_class("variation-cluster");
        variation_cluster.set_halign(Align::Center);
        variation_cluster.set_valign(Align::Start);
        variation_cluster.set_hexpand(false);
        variation_cluster.set_vexpand(false);
        variation_cluster.set_visible(false);

        let variation_meta = gtk4::Box::new(Orientation::Horizontal, 12);
        variation_meta.add_css_class("board-meta-row");
        variation_meta.add_css_class("variation-meta-row");
        variation_meta.set_halign(Align::Fill);
        variation_meta.set_size_request(BOARD_PX, -1);
        variation_meta.set_width_request(BOARD_PX);

        let variation_title = Label::new(Some(""));
        variation_title.add_css_class("variation-title");
        variation_title.set_halign(Align::Start);
        variation_title.set_hexpand(true);
        variation_title.set_xalign(0.0);

        let variation_ply = Label::new(Some("0/0"));
        variation_ply.add_css_class("ply-counter");
        variation_ply.set_halign(Align::End);
        variation_ply.set_xalign(1.0);

        variation_meta.append(&variation_title);
        variation_meta.append(&variation_ply);
        variation_cluster.append(&variation_meta);

        let variation_board = BoardView::new(BOARD_PX, colors);
        variation_cluster.append(&variation_board.widget);

        boards_row.append(&main_cluster);
        boards_row.append(&variation_cluster);
        stage.set_child(Some(&boards_row));

        // Pivotal-moment copy in the empty space above the centered board cluster.
        let pivotal_panel = gtk4::Box::new(Orientation::Vertical, 6);
        pivotal_panel.add_css_class("pivotal-panel");
        pivotal_panel.set_halign(Align::Center);
        pivotal_panel.set_valign(Align::Start);
        pivotal_panel.set_hexpand(false);
        pivotal_panel.set_size_request(BOARD_PX, -1);
        pivotal_panel.set_width_request(BOARD_PX);
        pivotal_panel.set_margin_top(8);
        pivotal_panel.set_can_target(false);

        let moment_head = Label::new(Some(""));
        moment_head.add_css_class("moment");
        moment_head.set_wrap(true);
        moment_head.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
        moment_head.set_xalign(0.0);
        moment_head.set_halign(Align::Fill);
        moment_head.set_justify(gtk4::Justification::Left);
        moment_head.set_max_width_chars(1);

        let moment_body = Label::new(Some(""));
        moment_body.add_css_class("moment-detail");
        moment_body.set_wrap(true);
        moment_body.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
        moment_body.set_xalign(0.0);
        moment_body.set_halign(Align::Fill);
        moment_body.set_justify(gtk4::Justification::Left);
        moment_body.set_max_width_chars(1);
        moment_body.set_visible(false);

        pivotal_panel.append(&moment_head);
        pivotal_panel.append(&moment_body);
        stage.add_overlay(&pivotal_panel);

        content.append(&stage);

        let keys_help = keys_help::build_overlay_child("Playbench", keys_help::PLAYBENCH);
        let (modal_root, modal_card) = modal::build_host();
        overlay.set_child(Some(&content));
        overlay.add_overlay(&keys_help);
        overlay.add_overlay(&modal_root);

        let analyzing_badge = Label::new(Some(""));
        analyzing_badge.add_css_class("analyzing-badge");
        analyzing_badge.set_halign(Align::Start);
        analyzing_badge.set_valign(Align::End);
        analyzing_badge.set_margin_start(16);
        analyzing_badge.set_margin_bottom(16);
        analyzing_badge.set_can_target(false);
        analyzing_badge.set_visible(false);
        overlay.add_overlay(&analyzing_badge);

        let state: Rc<RefCell<Option<Playback>>> = Rc::new(RefCell::new(None));
        let exit_variation: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
        let current: Rc<RefCell<Option<(String, String)>>> = Rc::new(RefCell::new(None));
        let analyzing: Rc<RefCell<Option<(String, String)>>> = Rc::new(RefCell::new(None));
        let analysis_load_gen = Rc::new(Cell::new(0u64));
        let (analysis_tx, analysis_rx) = async_channel::unbounded::<AnalysisLoadMsg>();
        let status_gen = Rc::new(Cell::new(0u64));
        let status_timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
        let modal_keys: Rc<RefCell<Option<modal::KeyHandlers>>> = Rc::new(RefCell::new(None));
        let analyze_action: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
        let analyze_all_action: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
        let sync_action: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
        let open_library_action: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
        let open_progress_action: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
        let busy = Rc::new(RefCell::new(false));
        let sync_state = Rc::new(RefCell::new(SyncUiState::Syncing));
        // Armed after `v`; next key must be `p`/`b` (Esc cancels).
        let view_chord = Rc::new(Cell::new(false));
        // Armed after first `a`; second `a` = analyze all, timeout = analyze one.
        let analyze_chord = Rc::new(RefCell::new(AnalyzeChord::default()));

        let request_analyze = {
            let library = library.clone();
            let current = current.clone();
            let busy = busy.clone();
            let analyze_action = analyze_action.clone();
            let modal_root = modal_root.clone();
            let modal_card = modal_card.clone();
            let modal_keys = modal_keys.clone();
            let content = content.clone();
            let keys_help = keys_help.clone();
            Rc::new(move || {
                if *busy.borrow() {
                    return;
                }
                let Some(f) = analyze_action.borrow().clone() else {
                    return;
                };
                let already = {
                    let lib = library.borrow();
                    current
                        .borrow()
                        .as_ref()
                        .and_then(|(id, gt)| lib.find(id, gt))
                        .or_else(|| lib.games.first())
                        .map(|g| g.is_analyzed())
                        .unwrap_or(false)
                };
                if !already {
                    f();
                    return;
                }
                keys_help.set_visible(false);
                let handlers = modal::show_confirm(
                    &modal_root,
                    &modal_card,
                    &content,
                    "Reanalyze this game?",
                    "This game already has analysis. Run Stockfish again and replace it?",
                    "Reanalyze",
                    move || f(),
                );
                *modal_keys.borrow_mut() = Some(handlers);
            }) as Rc<dyn Fn()>
        };

        board.set_fen(&start_fen());

        let set_moment = {
            let moment_head = moment_head.clone();
            let moment_body = moment_body.clone();
            Rc::new(move |head: String, body: String| {
                moment_head.set_text(&head);
                moment_head.set_visible(!head.is_empty());
                moment_body.set_text(&body);
                moment_body.set_visible(!body.is_empty());
            })
        };

        let show_ply = {
            let state = state.clone();
            let board = board.clone();
            let set_moment = set_moment.clone();
            let ply_label = ply_label.clone();
            let variation_cluster = variation_cluster.clone();
            let main_cluster = main_cluster.clone();
            let boards_row = boards_row.clone();
            Rc::new(move |ply: usize| {
                let mut borrow = state.borrow_mut();
                let Some(pb) = borrow.as_mut() else {
                    return;
                };
                // Main-game navigation always leaves dual-board mode.
                let was_varying = pb.variation.take().is_some();
                let max = pb.fens.len().saturating_sub(1);
                let ply = ply.min(max);
                pb.ply = ply;
                pb.pivotal_idx = pb.pivotal_moments.iter().position(|&p| p == ply);
                let fen = pb.fens[ply].clone();
                let uci = if ply == 0 {
                    None
                } else {
                    pb.ucis.get(ply - 1).and_then(|u| u.clone())
                };
                let best_uci = insight_at(pb, ply).and_then(|i| i.best_uci.clone());
                let (head, body) = moment_for_ply(pb);
                let counter = format!("{ply}/{max}");
                drop(borrow);

                if was_varying {
                    main_cluster.set_opacity(1.0);
                    main_cluster.remove_css_class("dimmed");
                    variation_cluster.set_visible(false);
                    boards_row.queue_resize();
                }

                board.set_fen(&fen);
                if let Some(uci) = uci.as_deref() {
                    if let Some((from, to)) = squares_from_uci(uci) {
                        board.set_last_move(Some(from), Some(to));
                    } else {
                        board.set_last_move(None, None);
                    }
                } else {
                    board.set_last_move(None, None);
                }
                if let Some(best) = best_uci.as_deref().filter(|b| Some(*b) != uci.as_deref()) {
                    if let Some((from, to)) = squares_from_uci(best) {
                        board.set_arrow(Some(from), Some(to));
                    } else {
                        board.set_arrow(None, None);
                    }
                } else {
                    board.set_arrow(None, None);
                }
                set_moment(head, body);
                ply_label.set_text(&counter);
            })
        };

        let paint_variation_ui = {
            let variation_cluster = variation_cluster.clone();
            let main_cluster = main_cluster.clone();
            let boards_row = boards_row.clone();
            Rc::new(move |active: bool| {
                if active {
                    main_cluster.set_opacity(0.42);
                    main_cluster.add_css_class("dimmed");
                    variation_cluster.set_visible(true);
                } else {
                    main_cluster.set_opacity(1.0);
                    main_cluster.remove_css_class("dimmed");
                    variation_cluster.set_visible(false);
                }
                boards_row.queue_resize();
            })
        };

        let show_variation_ply = {
            let state = state.clone();
            let variation_board = variation_board.clone();
            let variation_ply = variation_ply.clone();
            Rc::new(move |ply: usize| {
                let mut borrow = state.borrow_mut();
                let Some(pb) = borrow.as_mut() else {
                    return;
                };
                let Some(vp) = pb.variation.as_mut() else {
                    return;
                };
                let max = vp.fens.len().saturating_sub(1);
                let ply = ply.min(max);
                vp.ply = ply;
                let fen = vp.fens[ply].clone();
                let uci = if ply == 0 {
                    None
                } else {
                    vp.ucis.get(ply - 1).and_then(|u| u.clone())
                };
                let counter = format!("{ply}/{max}");
                drop(borrow);

                variation_board.set_fen(&fen);
                if let Some(uci) = uci.as_deref() {
                    if let Some((from, to)) = squares_from_uci(uci) {
                        variation_board.set_last_move(Some(from), Some(to));
                    } else {
                        variation_board.set_last_move(None, None);
                    }
                } else {
                    variation_board.set_last_move(None, None);
                }
                variation_board.set_arrow(None, None);
                variation_ply.set_text(&counter);
            })
        };

        let exit_variation_fn = {
            let state = state.clone();
            let paint_variation_ui = paint_variation_ui.clone();
            let overlay = overlay.clone();
            let view_chord = view_chord.clone();
            Rc::new(move || {
                view_chord.set(false);
                if let Some(pb) = state.borrow_mut().as_mut() {
                    pb.variation = None;
                }
                paint_variation_ui(false);
                overlay.grab_focus();
            }) as Rc<dyn Fn()>
        };
        *exit_variation.borrow_mut() = Some(exit_variation_fn.clone());

        let enter_variation = {
            let state = state.clone();
            let variation_title = variation_title.clone();
            let variation_board = variation_board.clone();
            let paint_variation_ui = paint_variation_ui.clone();
            let show_variation_ply = show_variation_ply.clone();
            let view_chord = view_chord.clone();
            let overlay = overlay.clone();
            Rc::new(move |kind: VariationKind| {
                view_chord.set(false);
                let built = {
                    let borrow = state.borrow();
                    let Some(pb) = borrow.as_ref() else {
                        eprintln!("reprise: vb/vp — no playback");
                        return;
                    };
                    let Some(ins) = insight_at(pb, pb.ply) else {
                        eprintln!("reprise: vb/vp — no insight at ply {}", pb.ply);
                        return;
                    };
                    let line = match variation_line_from_insight(ins, kind) {
                        Some(line) => line,
                        None => {
                            eprintln!(
                                "reprise: vb/vp — no {} line at ply {} ({})",
                                kind.title(),
                                pb.ply,
                                ins.san
                            );
                            return;
                        }
                    };
                    let white_at_bottom = pb.white_at_bottom;
                    (line, white_at_bottom)
                };
                let (line, white_at_bottom) = built;
                let start_ply = 1.min(line.fens.len().saturating_sub(1));
                variation_board.set_orientation_white_bottom(white_at_bottom);
                variation_title.set_text(kind.title());
                if let Some(pb) = state.borrow_mut().as_mut() {
                    pb.variation = Some(line);
                }
                paint_variation_ui(true);
                show_variation_ply(start_ply);
                variation_board.queue_draw();
                overlay.grab_focus();
            })
        };

        let step = {
            let state = state.clone();
            let show_ply = show_ply.clone();
            let show_variation_ply = show_variation_ply.clone();
            let paint_variation_ui = paint_variation_ui.clone();
            Rc::new(move |dir: i32| {
                let varying = state
                    .borrow()
                    .as_ref()
                    .is_some_and(|pb| pb.in_variation());
                if varying {
                    let next = {
                        let borrow = state.borrow();
                        let Some(pb) = borrow.as_ref() else {
                            return;
                        };
                        let Some(vp) = pb.variation.as_ref() else {
                            return;
                        };
                        let max = vp.fens.len().saturating_sub(1) as i32;
                        let next = vp.ply as i32 + dir;
                        if next < 0 || next > max {
                            return;
                        }
                        next as usize
                    };
                    show_variation_ply(next);
                    return;
                }
                // Ensure dual-board chrome is off when stepping the main game.
                paint_variation_ui(false);
                let t0 = Instant::now();
                let next = {
                    let borrow = state.borrow();
                    let Some(pb) = borrow.as_ref() else {
                        return;
                    };
                    let max = pb.fens.len().saturating_sub(1) as i32;
                    let next = pb.ply as i32 + dir;
                    if next < 0 || next > max {
                        return;
                    }
                    next as usize
                };
                show_ply(next);
                eprintln!("reprise: step dir={dir} ply={next} {:.2?}", t0.elapsed());
            })
        };

        let step_pivotal = {
            let state = state.clone();
            let show_ply = show_ply.clone();
            let paint_variation_ui = paint_variation_ui.clone();
            let view_chord = view_chord.clone();
            Rc::new(move |dir: i32| {
                view_chord.set(false);
                // Jumping pivots always returns to the main board.
                if let Some(pb) = state.borrow_mut().as_mut() {
                    pb.variation = None;
                }
                paint_variation_ui(false);
                let target = {
                    let borrow = state.borrow();
                    let Some(pb) = borrow.as_ref() else {
                        return;
                    };
                    match pivotal_target(pb, dir) {
                        Some(ply) => ply,
                        None => return,
                    }
                };
                show_ply(target);
            })
        };

        let toggle_keys = {
            let keys_help = keys_help.clone();
            let content = content.clone();
            let modal_root = modal_root.clone();
            Rc::new(move || {
                if modal_root.is_visible() {
                    return;
                }
                let open = !keys_help.is_visible();
                keys_help.set_visible(open);
                content.set_sensitive(!open);
                if open {
                    keys_help.grab_focus();
                }
            })
        };

        let close_keys = {
            let keys_help = keys_help.clone();
            let content = content.clone();
            let overlay = overlay.clone();
            Rc::new(move || {
                if keys_help.is_visible() {
                    keys_help.set_visible(false);
                    content.set_sensitive(true);
                    overlay.grab_focus();
                }
            })
        };

        // Digits then G (e.g. 10G) jumps to a ply — vim-style. Bare G → last ply.
        let goto_ply = Rc::new(RefCell::new(GotoPlyInput::default()));
        let push_goto_digit = {
            let goto_ply = goto_ply.clone();
            Rc::new(move |digit: char| {
                let mut g = goto_ply.borrow_mut();
                if let Some(id) = g.timer.take() {
                    id.remove();
                }
                let buf = g.digits.get_or_insert_with(String::new);
                if buf.len() >= 4 {
                    return true;
                }
                buf.push(digit);
                drop(g);
                // Drop a stale count if G never arrives.
                let goto_ply_timer = goto_ply.clone();
                let id = glib::timeout_add_local_once(
                    Duration::from_millis(GOTO_PLY_CANCEL_MS),
                    move || {
                        let mut g = goto_ply_timer.borrow_mut();
                        g.timer = None;
                        g.digits = None;
                    },
                );
                goto_ply.borrow_mut().timer = Some(id);
                true
            })
        };
        let commit_goto_ply = {
            let goto_ply = goto_ply.clone();
            let show_ply = show_ply.clone();
            let state = state.clone();
            Rc::new(move || {
                let digits = {
                    let mut g = goto_ply.borrow_mut();
                    if let Some(id) = g.timer.take() {
                        id.remove();
                    }
                    g.digits.take()
                };
                match digits {
                    Some(digits) if !digits.is_empty() => {
                        if let Ok(ply) = digits.parse::<usize>() {
                            show_ply(ply);
                        }
                    }
                    _ => {
                        let last = {
                            let borrow = state.borrow();
                            borrow
                                .as_ref()
                                .map(|pb| pb.fens.len().saturating_sub(1))
                                .unwrap_or(0)
                        };
                        show_ply(last);
                    }
                }
            })
        };
        let cancel_goto_ply = {
            let goto_ply = goto_ply.clone();
            Rc::new(move || {
                let mut g = goto_ply.borrow_mut();
                if let Some(id) = g.timer.take() {
                    id.remove();
                }
                g.digits = None;
            })
        };

        // Esc / ? on the help scrim
        let help_keys = gtk4::EventControllerKey::new();
        help_keys.connect_key_pressed({
            let close_keys = close_keys.clone();
            move |_, key, _, _| {
                use gtk4::gdk::Key;
                if key == Key::Escape || key == Key::question {
                    close_keys();
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Stop // swallow while open
                }
            }
        });
        keys_help.set_focusable(true);
        keys_help.add_controller(help_keys);

        let keys = gtk4::EventControllerKey::new();
        keys.connect_key_pressed({
            let step = step.clone();
            let step_pivotal = step_pivotal.clone();
            let toggle_keys = toggle_keys.clone();
            let close_keys = close_keys.clone();
            let keys_help = keys_help.clone();
            let modal_root = modal_root.clone();
            let modal_keys = modal_keys.clone();
            let show_ply = show_ply.clone();
            let state = state.clone();
            let enter_variation = enter_variation.clone();
            let exit_variation_fn = exit_variation_fn.clone();
            let show_variation_ply = show_variation_ply.clone();
            let view_chord = view_chord.clone();
            let push_goto_digit = push_goto_digit.clone();
            let commit_goto_ply = commit_goto_ply.clone();
            let cancel_goto_ply = cancel_goto_ply.clone();
            let goto_ply = goto_ply.clone();
            let request_analyze = request_analyze.clone();
            let sync_action = sync_action.clone();
            let open_library_action = open_library_action.clone();
            let open_progress_action = open_progress_action.clone();
            let analyze_all_action = analyze_all_action.clone();
            let analyze_chord = analyze_chord.clone();
            let send_to_agent = {
                let playbench_state = state.clone();
                let library = library.clone();
                let current = current.clone();
                Rc::new(move || {
                    let lib = library.borrow();
                    let game = current
                        .borrow()
                        .as_ref()
                        .and_then(|(id, gt)| lib.find(id, gt))
                        .or_else(|| lib.games.first());
                    let Some(game) = game else {
                        eprintln!("reprise: Ctrl+A — no game loaded");
                        return;
                    };
                    let borrow = playbench_state.borrow();
                    let Some(pb) = borrow.as_ref() else {
                        eprintln!("reprise: Ctrl+A — no playback");
                        return;
                    };
                    let prompt = agent_prompt(game, pb);
                    drop(borrow);
                    drop(lib);
                    weekly_review::launch_default_agent(&prompt);
                })
            };
            let send_weekly_review = {
                let library = library.clone();
                let analyzing_badge = analyzing_badge.clone();
                let status_gen = status_gen.clone();
                let status_timer = status_timer.clone();
                Rc::new(move || {
                    // Snapshot path + in-memory games (PGN/meta) for the worker.
                    let lib = library.borrow().clone();
                    if weekly_review::games_this_week(&lib).is_empty() {
                        // Ephemeral status without Playbench method (we're in a closure).
                        if let Some(id) = status_timer.borrow_mut().take() {
                            id.remove();
                        }
                        let token = status_gen.get().wrapping_add(1);
                        status_gen.set(token);
                        analyzing_badge.set_text("No games this week");
                        analyzing_badge.set_visible(true);
                        let badge = analyzing_badge.clone();
                        let status_gen = status_gen.clone();
                        let timer_slot = status_timer.clone();
                        let id = glib::timeout_add_local_once(Duration::from_millis(2500), move || {
                            *timer_slot.borrow_mut() = None;
                            if status_gen.get() == token {
                                badge.set_visible(false);
                            }
                        });
                        *status_timer.borrow_mut() = Some(id);
                        return;
                    }
                    if let Some(id) = status_timer.borrow_mut().take() {
                        id.remove();
                    }
                    let token = status_gen.get().wrapping_add(1);
                    status_gen.set(token);
                    analyzing_badge.set_text("Packing weekly review…");
                    analyzing_badge.set_visible(true);

                    let (tx, rx) = async_channel::bounded::<Result<weekly_review::WeeklyReviewPack, String>>(1);
                    std::thread::spawn(move || {
                        let result = weekly_review::build_pack(&lib)
                            .map_err(|e| format!("{e:#}"));
                        let _ = tx.send_blocking(result);
                    });
                    let analyzing_badge = analyzing_badge.clone();
                    let status_gen = status_gen.clone();
                    let status_timer = status_timer.clone();
                    glib::spawn_future_local(async move {
                        let result = rx.recv().await;
                        if status_gen.get() != token {
                            return;
                        }
                        match result {
                            Ok(Ok(pack)) => {
                                analyzing_badge.set_text(&format!(
                                    "Weekly review · {}/{} · → {}",
                                    pack.analyzed_count,
                                    pack.game_count,
                                    pack.review_relpath
                                ));
                                weekly_review::launch_default_agent(&pack.prompt);
                                let badge = analyzing_badge.clone();
                                let status_gen = status_gen.clone();
                                let timer_slot = status_timer.clone();
                                let done_token = token;
                                let id = glib::timeout_add_local_once(
                                    Duration::from_millis(3000),
                                    move || {
                                        *timer_slot.borrow_mut() = None;
                                        if status_gen.get() == done_token {
                                            badge.set_visible(false);
                                        }
                                    },
                                );
                                *status_timer.borrow_mut() = Some(id);
                            }
                            Ok(Err(err)) => {
                                eprintln!("reprise: weekly review failed: {err}");
                                analyzing_badge.set_text("Weekly review failed");
                                let badge = analyzing_badge.clone();
                                let status_gen = status_gen.clone();
                                let timer_slot = status_timer.clone();
                                let id = glib::timeout_add_local_once(
                                    Duration::from_millis(4000),
                                    move || {
                                        *timer_slot.borrow_mut() = None;
                                        if status_gen.get() == token {
                                            badge.set_visible(false);
                                        }
                                    },
                                );
                                *status_timer.borrow_mut() = Some(id);
                            }
                            Err(_) => {}
                        }
                    });
                })
            };
            move |_, key, _, mods| {
                use gtk4::gdk::{Key, ModifierType};
                if modal_root.is_visible() {
                    if let Some(handlers) = modal_keys.borrow().clone() {
                        if key == Key::Escape {
                            (handlers.on_escape)();
                            *modal_keys.borrow_mut() = None;
                        } else if key == Key::Return || key == Key::KP_Enter {
                            (handlers.on_enter)();
                            *modal_keys.borrow_mut() = None;
                        }
                    }
                    return glib::Propagation::Stop;
                }
                if keys_help.is_visible() {
                    if key == Key::Escape || key == Key::question {
                        close_keys();
                    }
                    return glib::Propagation::Stop;
                }

                let ctrl = mods.contains(ModifierType::CONTROL_MASK);
                if ctrl && (key == Key::a || key == Key::A) {
                    send_to_agent();
                    return glib::Propagation::Stop;
                }
                if ctrl && (key == Key::e || key == Key::E) {
                    send_weekly_review();
                    return glib::Propagation::Stop;
                }
                // Ignore other plain letter chords while Ctrl is held.
                if ctrl {
                    return glib::Propagation::Proceed;
                }

                // `v` then `p`/`b` — while armed, swallow everything else so a
                // spurious key can't cancel the chord and fall through to `p`/`b`.
                if view_chord.get() {
                    clear_analyze_chord(&analyze_chord);
                    return match key {
                        Key::p | Key::P => {
                            view_chord.set(false);
                            enter_variation(VariationKind::Punished);
                            glib::Propagation::Stop
                        }
                        Key::b | Key::B => {
                            view_chord.set(false);
                            enter_variation(VariationKind::Best);
                            glib::Propagation::Stop
                        }
                        Key::v | Key::V => {
                            view_chord.set(true);
                            glib::Propagation::Stop
                        }
                        Key::Escape => {
                            view_chord.set(false);
                            glib::Propagation::Stop
                        }
                        _ => {
                            view_chord.set(false);
                            glib::Propagation::Stop
                        }
                    };
                }

                // `a` then `a` → analyze all; lone `a` (after settle) → analyze one.
                if analyze_chord.borrow().armed {
                    if matches!(key, Key::a | Key::A) {
                        clear_analyze_chord(&analyze_chord);
                        if let Some(f) = analyze_all_action.borrow().clone() {
                            f();
                        }
                        return glib::Propagation::Stop;
                    }
                    if key == Key::Escape {
                        clear_analyze_chord(&analyze_chord);
                        return glib::Propagation::Stop;
                    }
                    // Other key: cancel pending single-analyze; fall through.
                    clear_analyze_chord(&analyze_chord);
                }

                // Variation mode: step side board, Esc to leave, `v` arms switch.
                let varying = state
                    .borrow()
                    .as_ref()
                    .is_some_and(|pb| pb.in_variation());
                if varying {
                    return match key {
                        Key::Right | Key::l => {
                            step(1);
                            glib::Propagation::Stop
                        }
                        Key::Left | Key::h => {
                            step(-1);
                            glib::Propagation::Stop
                        }
                        Key::Home => {
                            show_variation_ply(0);
                            glib::Propagation::Stop
                        }
                        Key::End => {
                            let last = {
                                let borrow = state.borrow();
                                borrow
                                    .as_ref()
                                    .and_then(|pb| pb.variation.as_ref())
                                    .map(|vp| vp.fens.len().saturating_sub(1))
                                    .unwrap_or(0)
                            };
                            show_variation_ply(last);
                            glib::Propagation::Stop
                        }
                        Key::v | Key::V => {
                            view_chord.set(true);
                            glib::Propagation::Stop
                        }
                        Key::Escape => {
                            exit_variation_fn();
                            glib::Propagation::Stop
                        }
                        _ => glib::Propagation::Stop,
                    };
                }

                if let Some(digit) = key_to_digit(key) {
                    push_goto_digit(digit);
                    return glib::Propagation::Stop;
                }

                // `<digits>G` commits; bare `G` → last ply (vim). Esc clears a count.
                if key == Key::G || key == Key::g {
                    let has_count = goto_ply
                        .borrow()
                        .digits
                        .as_ref()
                        .is_some_and(|d| !d.is_empty());
                    if has_count || key == Key::G {
                        commit_goto_ply();
                    }
                    return glib::Propagation::Stop;
                }

                let goto_armed = goto_ply.borrow().digits.is_some();
                if goto_armed {
                    if key == Key::Escape {
                        cancel_goto_ply();
                        return glib::Propagation::Stop;
                    }
                    // Shift (for capital G) must not wipe the count.
                    if is_modifier_key(key) {
                        return glib::Propagation::Stop;
                    }
                    cancel_goto_ply();
                }

                match key {
                    Key::Right | Key::l => {
                        step(1);
                        glib::Propagation::Stop
                    }
                    Key::Left | Key::h => {
                        step(-1);
                        glib::Propagation::Stop
                    }
                    Key::Home => {
                        show_ply(0);
                        glib::Propagation::Stop
                    }
                    Key::End => {
                        let last = {
                            let borrow = state.borrow();
                            borrow
                                .as_ref()
                                .map(|pb| pb.fens.len().saturating_sub(1))
                                .unwrap_or(0)
                        };
                        show_ply(last);
                        glib::Propagation::Stop
                    }
                    Key::n | Key::N => {
                        step_pivotal(1);
                        glib::Propagation::Stop
                    }
                    Key::p | Key::P => {
                        step_pivotal(-1);
                        glib::Propagation::Stop
                    }
                    Key::a | Key::A => {
                        arm_analyze_chord(
                            analyze_chord.clone(),
                            request_analyze.clone(),
                        );
                        glib::Propagation::Stop
                    }
                    Key::s | Key::S => {
                        if let Some(f) = sync_action.borrow().clone() {
                            f();
                        }
                        glib::Propagation::Stop
                    }
                    Key::o | Key::O => {
                        if let Some(f) = open_library_action.borrow().clone() {
                            f();
                        }
                        glib::Propagation::Stop
                    }
                    Key::d | Key::D => {
                        if let Some(f) = open_progress_action.borrow().clone() {
                            f();
                        }
                        glib::Propagation::Stop
                    }
                    Key::v | Key::V => {
                        view_chord.set(true);
                        glib::Propagation::Stop
                    }
                    Key::question => {
                        toggle_keys();
                        glib::Propagation::Stop
                    }
                    Key::Escape => {
                        view_chord.set(false);
                        clear_analyze_chord(&analyze_chord);
                        exit_variation_fn();
                        close_keys();
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                }
            }
        });
        overlay.add_controller(keys);
        overlay.set_focusable(true);

        let playbench = Self {
            widget: overlay,
            content,
            board,
            variation_board,
            moment_head,
            moment_body,
            pivotal_panel,
            game_meta,
            ply_label,
            keys_help,
            modal_root,
            modal_card,
            modal_keys,
            analyze_action,
            request_analyze,
            analyze_all_action,
            sync_action,
            open_library_action,
            open_progress_action,
            analyzing_badge,
            analyzing,
            analysis_load_gen,
            analysis_tx,
            status_gen,
            status_timer,
            library,
            state,
            exit_variation,
            current,
            busy,
            sync_state,
        };
        playbench.attach_analysis_loader(analysis_rx);
        playbench.set_status("Starting…");
        playbench.refresh_analyze_cluster();
        playbench.refresh_board();
        playbench
    }

    fn attach_analysis_loader(&self, rx: async_channel::Receiver<AnalysisLoadMsg>) {
        let library = self.library.clone();
        let state = self.state.clone();
        let current = self.current.clone();
        let analysis_load_gen = self.analysis_load_gen.clone();
        let board = self.board.clone();
        let moment_head = self.moment_head.clone();
        let moment_body = self.moment_body.clone();
        let game_meta = self.game_meta.clone();
        let ply_label = self.ply_label.clone();
        let analyzing = self.analyzing.clone();
        let analyzing_badge = self.analyzing_badge.clone();
        let status_gen = self.status_gen.clone();
        let status_timer = self.status_timer.clone();
        let sync_state = self.sync_state.clone();
        let exit_variation = self.exit_variation.clone();
        glib::spawn_future_local(async move {
            while let Ok(msg) = rx.recv().await {
                let t0 = Instant::now();
                if msg.load_gen != analysis_load_gen.get() {
                    continue;
                }
                if current.borrow().as_ref() != Some(&(msg.id.clone(), msg.game_type.clone())) {
                    continue;
                }
                if let Some(exit) = exit_variation.borrow().clone() {
                    exit();
                }

                let status_still_ours = status_gen.get() == msg.status_token;
                let can_take_badge = analyzing.borrow().is_none()
                    && *sync_state.borrow() != SyncUiState::Syncing
                    && status_still_ours;

                match msg.result {
                    Ok(Some(analysis)) => {
                        let game = {
                            let mut lib = library.borrow_mut();
                            let Some(g) = lib
                                .games
                                .iter_mut()
                                .find(|g| g.id == msg.id && g.game_type == msg.game_type)
                            else {
                                continue;
                            };
                            g.has_analysis = true;
                            g.analysis = Some(analysis);
                            g.clone()
                        };
                        present_game(
                            &game,
                            &board,
                            &state,
                            &moment_head,
                            &moment_body,
                            &game_meta,
                            &ply_label,
                            true,
                        );
                        if can_take_badge {
                            if let Some(id) = status_timer.borrow_mut().take() {
                                id.remove();
                            }
                            status_gen.set(status_gen.get().wrapping_add(1));
                            analyzing_badge.set_visible(false);
                        }
                    }
                    Ok(None) => {
                        if let Some(game) = library
                            .borrow()
                            .find(&msg.id, &msg.game_type)
                            .cloned()
                        {
                            present_game(
                                &game,
                                &board,
                                &state,
                                &moment_head,
                                &moment_body,
                                &game_meta,
                                &ply_label,
                                false,
                            );
                        }
                        if can_take_badge {
                            if let Some(id) = status_timer.borrow_mut().take() {
                                id.remove();
                            }
                            status_gen.set(status_gen.get().wrapping_add(1));
                            analyzing_badge.set_text("No analysis stored");
                            analyzing_badge.set_visible(true);
                        }
                    }
                    Err(err) => {
                        eprintln!("reprise: load analysis failed: {err:#}");
                        if let Some(game) = library
                            .borrow()
                            .find(&msg.id, &msg.game_type)
                            .cloned()
                        {
                            present_game(
                                &game,
                                &board,
                                &state,
                                &moment_head,
                                &moment_body,
                                &game_meta,
                                &ply_label,
                                false,
                            );
                        }
                        if can_take_badge {
                            if let Some(id) = status_timer.borrow_mut().take() {
                                id.remove();
                            }
                            status_gen.set(status_gen.get().wrapping_add(1));
                            analyzing_badge.set_text("Failed to load analysis");
                            analyzing_badge.set_visible(true);
                        }
                    }
                }
                eprintln!(
                    "reprise: analysis load applied on UI thread {:.2?}",
                    t0.elapsed()
                );
            }
        });
    }

    fn cancel_status_timer(&self) {
        if let Some(id) = self.status_timer.borrow_mut().take() {
            id.remove();
        }
    }

    /// Bottom-left status (sync / analyze / load). Never touches pivotal copy.
    pub fn set_status(&self, text: &str) {
        self.cancel_status_timer();
        self.status_gen
            .set(self.status_gen.get().wrapping_add(1));
        self.analyzing_badge.set_text(text);
        self.analyzing_badge.set_visible(true);
    }

    pub fn set_status_ephemeral(&self, text: &str, ms: u64) {
        self.set_status(text);
        let token = self.status_gen.get();
        let badge = self.analyzing_badge.clone();
        let status_gen = self.status_gen.clone();
        let timer_slot = self.status_timer.clone();
        let id = glib::timeout_add_local_once(Duration::from_millis(ms), move || {
            *timer_slot.borrow_mut() = None;
            if status_gen.get() == token {
                badge.set_visible(false);
            }
        });
        *self.status_timer.borrow_mut() = Some(id);
    }

    pub fn clear_status(&self) {
        self.cancel_status_timer();
        self.status_gen
            .set(self.status_gen.get().wrapping_add(1));
        self.analyzing_badge.set_visible(false);
    }

    pub fn set_sync_state(&self, state: SyncUiState) {
        *self.sync_state.borrow_mut() = state;
        self.apply_sync_ui(state);
    }

    fn apply_sync_ui(&self, state: SyncUiState) {
        match state {
            SyncUiState::Syncing => self.set_status("Checking for new games…"),
            SyncUiState::UpToDate => {}
            SyncUiState::NeedsRefresh => {
                self.set_status_ephemeral("Sync failed · press s to retry", 4000)
            }
        }
    }

    pub fn note_up_to_date(&self) {
        self.set_status_ephemeral("Up to date", 2500);
    }

    pub fn note_new_games(&self, n: usize) {
        let label = if n == 1 {
            "1 new game".to_string()
        } else {
            format!("{n} new games")
        };
        self.set_status(&label);
    }

    pub fn is_analyzing(&self) -> bool {
        self.analyzing.borrow().is_some()
    }

    /// Clear badge, or restore sync-checking text if a sync is still in flight.
    pub fn clear_status_or_restore_sync(&self) {
        if *self.sync_state.borrow() == SyncUiState::Syncing {
            self.set_status("Checking for new games…");
        } else {
            self.clear_status();
        }
    }

    pub fn refresh_analyze_cluster(&self) {
        // Analyze-all used to live in the topbar; progress uses the badge now.
    }

    /// Transient progress into the bottom-left status badge.
    pub fn set_analyze_progress(&self, text: &str) {
        self.set_status(text);
    }

    pub fn set_busy(&self, busy: bool) {
        *self.busy.borrow_mut() = busy;
    }

    pub fn is_busy(&self) -> bool {
        *self.busy.borrow()
    }

    pub fn current_game_key(&self) -> Option<(String, String)> {
        self.current.borrow().clone()
    }

    pub fn current_game_pgn(&self) -> Option<String> {
        let key = self.current.borrow().clone()?;
        self.library
            .borrow()
            .find(&key.0, &key.1)
            .map(|g| g.pgn.clone())
    }

    pub fn library_snapshot(&self) -> Library {
        self.library.borrow().clone()
    }

    /// Mark which game is analyzing (or `None` when idle).
    pub fn set_analyzing(&self, key: Option<(String, String)>) {
        *self.analyzing.borrow_mut() = key;
    }

    pub fn refresh_analyzing_badge(&self) {
        // Visibility is owned by set_status / clear_status.
    }

    pub fn unanalyzed_keys(&self) -> Vec<(String, String)> {
        self.library.borrow().unanalyzed_ids()
    }

    /// Unanalyzed games as (id, game_type, pgn) for background analyze-all.
    pub fn unanalyzed_jobs(&self) -> Vec<(String, String, String)> {
        self.library
            .borrow()
            .games
            .iter()
            .filter(|g| !g.is_analyzed())
            .map(|g| (g.id.clone(), g.game_type.clone(), g.pgn.clone()))
            .collect()
    }

    /// Newest game in the library if it still needs analysis (id, game_type, pgn).
    pub fn newest_unanalyzed(&self) -> Option<(String, String, String)> {
        let lib = self.library.borrow();
        let g = lib.games.first()?;
        if g.is_analyzed() {
            return None;
        }
        Some((g.id.clone(), g.game_type.clone(), g.pgn.clone()))
    }

    pub fn replace_library(&self, next: Library) {
        let keep = self.current.borrow().clone();
        // Keep in-memory analysis blobs so a sync reload doesn't force a
        // re-load (and wipe pivotal copy) for the game already on screen.
        let mut next = next;
        {
            let old = self.library.borrow();
            for g in next.games.iter_mut() {
                if g.analysis.is_none() {
                    if let Some(prev) = old.find(&g.id, &g.game_type) {
                        g.analysis = prev.analysis.clone();
                    }
                }
            }
        }
        *self.library.borrow_mut() = next;
        self.refresh_analyze_cluster();
        match keep {
            Some((id, gt)) if self.library.borrow().find(&id, &gt).is_some() => {
                // Same game still present — keep pivotal copy / board as-is.
            }
            Some(_) => {
                *self.current.borrow_mut() = None;
                self.refresh_board();
            }
            None => self.refresh_board(),
        }
    }

    pub fn update_library_only(&self, next: Library) {
        *self.library.borrow_mut() = next;
        self.refresh_analyze_cluster();
    }

    /// Flip `has_analysis` in memory after a background analyze finishes.
    pub fn mark_analyzed(&self, ids: &[(String, String)]) {
        let mut lib = self.library.borrow_mut();
        for (id, gt) in ids {
            if let Some(g) = lib.games.iter_mut().find(|g| g.id == *id && g.game_type == *gt) {
                g.has_analysis = true;
                // Drop any stale in-memory blob so refresh_board reloads from disk.
                g.analysis = None;
            }
        }
    }

    /// Open a specific game on the playbench (e.g. from the library list).
    pub fn open_game(&self, id: &str, game_type: &str) {
        *self.current.borrow_mut() = Some((id.to_string(), game_type.to_string()));
        self.refresh_board();
    }

    /// Persist that the current game was opened for review (analyzed + on playbench).
    pub fn mark_current_reviewed(&self) {
        let t0 = Instant::now();
        let Some((id, gt)) = self.current.borrow().clone() else {
            return;
        };
        let analyzed = self
            .library
            .borrow()
            .find(&id, &gt)
            .map(|g| g.is_analyzed())
            .unwrap_or(false);
        if !analyzed {
            return;
        }
        let path = self.library.borrow().path.clone();
        let changed = self.library.borrow_mut().mark_reviewed(&id, &gt);
        if changed {
            persist_reviewed_flag_async(path, id, gt);
        }
        eprintln!("reprise: mark_current_reviewed main {:.2?}", t0.elapsed());
    }

    pub fn refresh_board(&self) {
        let t0 = Instant::now();
        if let Some(exit) = self.exit_variation.borrow().clone() {
            exit();
        }
        // Invalidate any in-flight analysis load for a previous game.
        let load_gen = self.analysis_load_gen.get().wrapping_add(1);
        self.analysis_load_gen.set(load_gen);

        let game = {
            let lib = self.library.borrow();
            let from_current = self
                .current
                .borrow()
                .as_ref()
                .and_then(|(id, gt)| lib.find(id, gt).cloned());
            from_current.or_else(|| lib.games.first().cloned())
        };
        match game {
            Some(game) => {
                *self.current.borrow_mut() = Some((game.id.clone(), game.game_type.clone()));

                let needs_async_load = game.analysis.is_none() && game.has_analysis;
                if needs_async_load {
                    // Keep pivotal area empty (never "Loading…"); status is bottom-left.
                    let white_at_bottom = game.user_color.as_deref() != Some("black");
                    self.board.set_orientation_white_bottom(white_at_bottom);
                    self.game_meta.set_text(&game_meta_line(&game));
                    if let Some(fen) = game.end_fen.as_deref() {
                        self.board.set_fen(fen);
                        if let Some(uci) = game.end_uci.as_deref() {
                            if let Some((from, to)) = squares_from_uci(uci) {
                                self.board.set_last_move(Some(from), Some(to));
                            } else {
                                self.board.set_last_move(None, None);
                            }
                        } else {
                            self.board.set_last_move(None, None);
                        }
                        if let Some(ply) = game.end_ply {
                            self.ply_label.set_text(&format!("{ply}/{ply}"));
                        }
                    } else {
                        self.board.set_fen(&start_fen());
                        self.board.set_last_move(None, None);
                        self.ply_label.set_text("…");
                    }
                    self.board.set_arrow(None, None);
                    *self.state.borrow_mut() = None;
                    self.moment_head.set_text("");
                    self.moment_head.set_visible(false);
                    self.moment_body.set_text("");
                    self.moment_body.set_visible(false);
                    self.set_status("Loading analysis…");
                    let status_token = self.status_gen.get();

                    let path = self.library.borrow().path.clone();
                    let id = game.id.clone();
                    let game_type = game.game_type.clone();
                    let tx = self.analysis_tx.clone();
                    std::thread::spawn(move || {
                        let bg = Instant::now();
                        let result = load_game_analysis(&path, &id, &game_type);
                        eprintln!(
                            "reprise: analysis load bg thread {:.2?}",
                            bg.elapsed()
                        );
                        let _ = tx.send_blocking(AnalysisLoadMsg {
                            load_gen,
                            status_token,
                            id,
                            game_type,
                            result,
                        });
                    });
                    eprintln!(
                        "reprise: refresh_board main (scheduled async load) {:.2?}",
                        t0.elapsed()
                    );
                } else {
                    present_game(
                        &game,
                        &self.board,
                        &self.state,
                        &self.moment_head,
                        &self.moment_body,
                        &self.game_meta,
                        &self.ply_label,
                        true,
                    );
                    eprintln!("reprise: refresh_board main {:.2?}", t0.elapsed());
                }
            }
            None => {
                *self.current.borrow_mut() = None;
                self.board.set_fen(&start_fen());
                self.board.set_last_move(None, None);
                self.board.set_arrow(None, None);
                *self.state.borrow_mut() = None;
                self.moment_head.set_text("");
                self.moment_head.set_visible(false);
                self.moment_body.set_text("");
                self.moment_body.set_visible(false);
                self.game_meta.set_text("");
                self.ply_label.set_text("0/0");
                eprintln!("reprise: refresh_board main (empty) {:.2?}", t0.elapsed());
            }
        }
        self.refresh_analyzing_badge();
    }

    pub fn on_analyze<F: Fn() + 'static>(&self, f: F) {
        *self.analyze_action.borrow_mut() = Some(Rc::new(f));
    }

    /// Same path as the `a` key (confirms before reanalyze).
    pub fn request_analyze(&self) {
        (self.request_analyze)();
    }

    /// Run analysis on the current game with no reanalyze confirm (list `a`).
    pub fn request_analyze_now(&self) {
        if *self.busy.borrow() {
            return;
        }
        if let Some(f) = self.analyze_action.borrow().clone() {
            f();
        }
    }

    pub fn on_analyze_all<F: Fn() + 'static>(&self, f: F) {
        *self.analyze_all_action.borrow_mut() = Some(Rc::new(f));
    }

    pub fn on_sync<F: Fn() + 'static>(&self, f: F) {
        *self.sync_action.borrow_mut() = Some(Rc::new(f));
    }

    /// Same path as the `s` key.
    pub fn request_sync(&self) {
        if let Some(f) = self.sync_action.borrow().clone() {
            f();
        }
    }

    pub fn on_open_library<F: Fn() + 'static>(&self, f: F) {
        *self.open_library_action.borrow_mut() = Some(Rc::new(f));
    }

    pub fn on_open_progress<F: Fn() + 'static>(&self, f: F) {
        *self.open_progress_action.borrow_mut() = Some(Rc::new(f));
    }

    pub fn board_repaint_callback(&self) -> Rc<dyn Fn()> {
        let board = self.board.clone();
        let variation_board = self.variation_board.clone();
        Rc::new(move || {
            board.queue_draw();
            variation_board.queue_draw();
        })
    }

    pub fn grab_focus(&self) {
        self.widget.grab_focus();
    }

    pub fn prompt_username(&self, on_ok: impl Fn(String) + 'static) {
        // Close help if it was open so only one overlay is active.
        self.keys_help.set_visible(false);
        *self.modal_keys.borrow_mut() = None;
        modal::show_username(&self.modal_root, &self.modal_card, &self.content, on_ok);
    }

    pub fn notify_analysis_finished(&self, detail: &str, on_ok: impl FnOnce() + 'static) {
        self.keys_help.set_visible(false);
        let handlers = modal::show_notice(
            &self.modal_root,
            &self.modal_card,
            &self.content,
            "Analysis finished",
            detail,
            on_ok,
        );
        *self.modal_keys.borrow_mut() = Some(handlers);
    }

    pub fn notify_analysis_failed(&self, detail: &str) {
        self.keys_help.set_visible(false);
        let handlers = modal::show_notice(
            &self.modal_root,
            &self.modal_card,
            &self.content,
            "Analysis failed",
            detail,
            || {},
        );
        *self.modal_keys.borrow_mut() = Some(handlers);
    }
}

pub(crate) fn game_meta_line(game: &Game) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(at) = game.played_at {
        parts.push(at.format("%d %b").to_string());
    }

    let (my_elo, opp_elo) = match game.user_color.as_deref() {
        Some("black") => (game.black_elo, game.white_elo),
        _ => (game.white_elo, game.black_elo),
    };
    let opp_name = game
        .opponent
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("Opp");

    match (my_elo, opp_elo) {
        (Some(me), Some(opp)) => parts.push(format!("You {me} · {opp_name} {opp}")),
        (Some(me), None) => parts.push(format!("You {me}")),
        (None, Some(opp)) => parts.push(format!("{opp_name} {opp}")),
        (None, None) => {
            if game.opponent.as_deref().is_some_and(|s| !s.is_empty()) {
                parts.push(format!("vs {opp_name}"));
            }
        }
    }

    if let Some(result) = game.result.as_deref().filter(|r| !r.is_empty()) {
        parts.push(result.to_string());
    }

    parts.join(" · ")
}

#[derive(Default)]
struct GotoPlyInput {
    /// Buffered count for `<digits>G`. `None` = idle.
    digits: Option<String>,
    timer: Option<glib::SourceId>,
}

#[derive(Default)]
struct AnalyzeChord {
    armed: bool,
    timer: Option<glib::SourceId>,
}

const GOTO_PLY_CANCEL_MS: u64 = 1500;
const ANALYZE_CHORD_MS: u64 = 400;

fn clear_analyze_chord(chord: &Rc<RefCell<AnalyzeChord>>) {
    let mut c = chord.borrow_mut();
    if let Some(id) = c.timer.take() {
        id.remove();
    }
    c.armed = false;
}

fn arm_analyze_chord(chord: Rc<RefCell<AnalyzeChord>>, request_analyze: Rc<dyn Fn()>) {
    clear_analyze_chord(&chord);
    {
        let mut c = chord.borrow_mut();
        c.armed = true;
    }
    let id = glib::timeout_add_local_once(Duration::from_millis(ANALYZE_CHORD_MS), {
        let chord = chord.clone();
        move || {
            {
                let mut c = chord.borrow_mut();
                c.timer = None;
                if !c.armed {
                    return;
                }
                c.armed = false;
            }
            request_analyze();
        }
    });
    chord.borrow_mut().timer = Some(id);
}

fn key_to_digit(key: gtk4::gdk::Key) -> Option<char> {
    use gtk4::gdk::Key;
    match key {
        Key::_0 | Key::KP_0 => Some('0'),
        Key::_1 | Key::KP_1 => Some('1'),
        Key::_2 | Key::KP_2 => Some('2'),
        Key::_3 | Key::KP_3 => Some('3'),
        Key::_4 | Key::KP_4 => Some('4'),
        Key::_5 | Key::KP_5 => Some('5'),
        Key::_6 | Key::KP_6 => Some('6'),
        Key::_7 | Key::KP_7 => Some('7'),
        Key::_8 | Key::KP_8 => Some('8'),
        Key::_9 | Key::KP_9 => Some('9'),
        _ => None,
    }
}

fn is_modifier_key(key: gtk4::gdk::Key) -> bool {
    use gtk4::gdk::Key;
    matches!(
        key,
        Key::Shift_L
            | Key::Shift_R
            | Key::Control_L
            | Key::Control_R
            | Key::Alt_L
            | Key::Alt_R
            | Key::Meta_L
            | Key::Meta_R
            | Key::Super_L
            | Key::Super_R
            | Key::Hyper_L
            | Key::Hyper_R
            | Key::Caps_Lock
            | Key::Num_Lock
            | Key::Scroll_Lock
            | Key::ISO_Level3_Shift
            | Key::ISO_Level5_Shift
    )
}


fn insight_at(pb: &Playback, ply: usize) -> Option<&PlyInsight> {
    pb.insights.iter().find(|i| i.ply == ply)
}

fn variation_line_from_insight(
    ins: &PlyInsight,
    kind: VariationKind,
) -> Option<VariationPlayback> {
    let (fen, pv) = match kind {
        VariationKind::Best => (ins.fen_before.as_str(), ins.best_pv_sans.as_ref()?),
        VariationKind::Punished => (ins.fen_after.as_str(), ins.punished_pv_sans.as_ref()?),
    };
    if pv.is_empty() {
        return None;
    }
    let (fens, ucis) = apply_san_line_with_ucis(fen, pv)?;
    if fens.len() < 2 {
        return None;
    }
    Some(VariationPlayback {
        kind,
        fens,
        ucis,
        ply: 0,
    })
}

fn moment_for_ply(pb: &Playback) -> (String, String) {
    if pb.ply == 0 {
        if pb.insights.is_empty() {
            return (String::new(), String::new());
        }
        let body = if pb.pivotal_moments.is_empty() {
            "Step through with ← →".to_string()
        } else {
            format!(
                "{} pivotal moments · n / p jumps · step through with ← →",
                pb.pivotal_moments.len()
            )
        };
        return ("Start".into(), body);
    }

    let Some(ins) = insight_at(pb, pb.ply) else {
        return (String::new(), String::new());
    };

    let who = if ins.mine { "You" } else { "Opp" };
    let class = title_case(&ins.classification);
    let loss = ins
        .loss_cp
        .map(|cp| format!("−{:.1}", cp as f64 / 100.0))
        .unwrap_or_else(|| "—".into());
    let eval = ins
        .eval_played
        .as_deref()
        .or(ins.eval_best_before.as_deref())
        .unwrap_or("—");

    let mut head = String::new();
    if let Some(i) = pb.pivotal_idx {
        head.push_str(&format!("Pivotal Moment {}/{} · ", i + 1, pb.pivotal_moments.len()));
    }
    head.push_str(&format!("{who} · {class} · {} · {loss} · {eval}", ins.san));

    let mut parts: Vec<String> = Vec::new();
    if let Some(s) = ins.summary.as_deref().filter(|s| !s.is_empty()) {
        parts.push(s.to_string());
    }
    if let Some(s) = ins.why_played.as_deref().filter(|s| !s.is_empty()) {
        parts.push(s.to_string());
    }
    if let Some(s) = ins.why_best.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("Better: {s}"));
    }

    if let Some(pv) = ins.best_pv.as_deref().filter(|s| !s.is_empty()) {
        if !parts.iter().any(|p| p.contains(pv)) {
            parts.push(format!("Best line: {pv}"));
        }
    }
    if let Some(cont) = ins
        .played_continuation
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        if !parts.iter().any(|p| p.contains(cont)) {
            parts.push(format!("After the played move: {cont}"));
        }
    }

    if parts.is_empty() {
        let mut facts = Vec::new();
        if let (Some(best), Some(before)) = (&ins.best_san, &ins.eval_best_before) {
            let be = ins.best_eval.as_deref().unwrap_or("—");
            if !matches!(ins.classification.as_str(), "best" | "excellent") {
                facts.push(format!("Best was {best} ({be}); before-move best eval {before}"));
            }
        }
        if facts.is_empty() {
            facts.push(format!(
                "{who} played {}. Classification: {} ({loss} pawns).",
                ins.san, ins.classification
            ));
        }
        parts = facts;
    }

    (head, parts.join("\n\n"))
}

fn pivotal_target(pb: &Playback, dir: i32) -> Option<usize> {
    if pb.pivotal_moments.is_empty() {
        return None;
    }
    let n = pb.pivotal_moments.len();
    if let Some(i) = pb.pivotal_idx {
        let next = ((i as i32 + dir).rem_euclid(n as i32)) as usize;
        return Some(pb.pivotal_moments[next]);
    }
    if dir > 0 {
        pb.pivotal_moments.iter().copied().find(|&p| p > pb.ply)
    } else {
        pb.pivotal_moments.iter().rev().copied().find(|&p| p < pb.ply)
    }
}

fn present_game(
    game: &Game,
    board: &BoardView,
    state: &Rc<RefCell<Option<Playback>>>,
    moment_head: &Label,
    moment_body: &Label,
    game_meta: &Label,
    ply_label: &Label,
    prefer_first_pivotal: bool,
) {
    let white_at_bottom = game.user_color.as_deref() != Some("black");
    board.set_orientation_white_bottom(white_at_bottom);
    game_meta.set_text(&game_meta_line(game));

    let playback = build_playback(game);
    let has_pivotals = !playback.pivotal_moments.is_empty();

    if let Some(fen) = playback.fens.first() {
        board.set_fen(fen);
    }
    board.set_last_move(None, None);

    if prefer_first_pivotal && has_pivotals {
        let first = playback.pivotal_moments[0];
        *state.borrow_mut() = Some(Playback {
            pivotal_idx: Some(0),
            ..playback
        });
        goto_ply_widgets(
            board,
            state,
            moment_head,
            moment_body,
            ply_label,
            first,
        );
    } else {
        *state.borrow_mut() = Some(playback);
        goto_ply_widgets(
            board,
            state,
            moment_head,
            moment_body,
            ply_label,
            0,
        );
        if prefer_first_pivotal {
            moment_head.set_text("");
            moment_head.set_visible(false);
            moment_body.set_text("");
            moment_body.set_visible(false);
        }
    }
}

fn goto_ply_widgets(
    board: &BoardView,
    state: &Rc<RefCell<Option<Playback>>>,
    moment_head: &Label,
    moment_body: &Label,
    ply_label: &Label,
    ply: usize,
) {
    let mut borrow = state.borrow_mut();
    let Some(pb) = borrow.as_mut() else {
        return;
    };
    let max = pb.fens.len().saturating_sub(1);
    let ply = ply.min(max);
    pb.ply = ply;
    pb.pivotal_idx = pb.pivotal_moments.iter().position(|&p| p == ply);
    let fen = pb.fens[ply].clone();
    let uci = if ply == 0 {
        None
    } else {
        pb.ucis.get(ply - 1).and_then(|u| u.clone())
    };
    let best_uci = insight_at(pb, ply).and_then(|i| i.best_uci.clone());
    let (head, body) = moment_for_ply(pb);
    let counter = format!("{ply}/{max}");
    drop(borrow);

    board.set_fen(&fen);
    if let Some(uci) = uci.as_deref() {
        if let Some((from, to)) = squares_from_uci(uci) {
            board.set_last_move(Some(from), Some(to));
        } else {
            board.set_last_move(None, None);
        }
    } else {
        board.set_last_move(None, None);
    }
    if let Some(best) = best_uci.as_deref().filter(|b| Some(*b) != uci.as_deref()) {
        if let Some((from, to)) = squares_from_uci(best) {
            board.set_arrow(Some(from), Some(to));
        } else {
            board.set_arrow(None, None);
        }
    } else {
        board.set_arrow(None, None);
    }
    let show_head = !head.is_empty();
    let show_body = !body.is_empty();
    moment_head.set_text(&head);
    moment_head.set_visible(show_head);
    moment_body.set_text(&body);
    moment_body.set_visible(show_body);
    ply_label.set_text(&counter);
}


fn build_playback(game: &Game) -> Playback {
    let t0 = Instant::now();
    let white_at_bottom = game.user_color.as_deref() != Some("black");
    let insights = ply_insights(game);
    let pivotal_moments: Vec<usize> = game.pivotal_moments().into_iter().map(|m| m.ply as usize).collect();

    if let Some(analysis) = &game.analysis {
        if !analysis.moves.is_empty() {
            let mut fens = vec![analysis.moves[0].fen_before.clone()];
            let mut ucis = Vec::new();
            for m in &analysis.moves {
                fens.push(m.fen_after.clone());
                ucis.push(m.uci.clone());
            }
            eprintln!(
                "reprise: build playback from analysis {} plies · {} pivotal moments · {} insights {:.2?}",
                ucis.len(),
                pivotal_moments.len(),
                insights.len(),
                t0.elapsed()
            );
            return Playback {
                fens,
                ucis,
                ply: 0,
                insights,
                pivotal_moments,
                pivotal_idx: None,
                white_at_bottom,
                variation: None,
            };
        }
    }

    let mut board = rschess::Board::default();
    let mut fens = vec![board.to_fen().to_string()];
    let mut ucis = Vec::new();
    for san in pgn_sans(&game.pgn) {
        let Ok(mv) = board.san_to_move(&san) else {
            break;
        };
        let uci = mv.to_uci();
        if board.make_move(mv).is_err() {
            break;
        }
        ucis.push(Some(uci));
        fens.push(board.to_fen().to_string());
    }
    eprintln!(
        "reprise: build playback from pgn {} plies · {} pivotal moments · {} insights {:.2?}",
        ucis.len(),
        pivotal_moments.len(),
        insights.len(),
        t0.elapsed()
    );

    Playback {
        fens,
        ucis,
        ply: 0,
        insights,
        pivotal_moments,
        pivotal_idx: None,
        white_at_bottom,
        variation: None,
    }
}

fn ply_insights(game: &Game) -> Vec<PlyInsight> {
    let Some(analysis) = &game.analysis else {
        return Vec::new();
    };
    analysis
        .moves
        .iter()
        .map(|m| {
            let mine = color_is_mine_move(&m.color, game.user_color.as_deref());
            let best = m.best_moves.first();
            let best_pv_sans = best
                .and_then(|b| b.pv_san.clone())
                .filter(|pv| !pv.is_empty());
            let punished_pv_sans = m
                .ideal_after_played
                .as_ref()
                .and_then(|p| p.pv_san.clone())
                .filter(|pv| !pv.is_empty());
            let best_pv = best_pv_sans
                .as_ref()
                .map(|pv| pv.iter().take(8).cloned().collect::<Vec<_>>().join(" "));
            let played_continuation = punished_pv_sans
                .as_ref()
                .map(|pv| pv.iter().take(8).cloned().collect::<Vec<_>>().join(" "));
            let teaching = m.teaching.as_ref();
            PlyInsight {
                ply: m.ply as usize,
                san: m.san.clone(),
                mine,
                classification: m.classification.clone(),
                loss_cp: m.loss_cp,
                eval_played: m.eval_played_text.clone(),
                eval_best_before: m.eval_before_best_text.clone(),
                best_san: best.and_then(|b| b.san.clone()),
                best_eval: best.and_then(|b| b.eval_text.clone()),
                best_uci: best.and_then(|b| b.uci.clone()),
                best_pv,
                played_continuation,
                fen_before: m.fen_before.clone(),
                fen_after: m.fen_after.clone(),
                best_pv_sans,
                punished_pv_sans,
                summary: teaching.and_then(|t| t.summary.clone()),
                why_played: teaching.and_then(|t| t.why_played.clone()),
                why_best: teaching.and_then(|t| t.why_best.clone()),
            }
        })
        .collect()
}

fn color_is_mine_move(move_color: &str, user_color: Option<&str>) -> bool {
    let user_is_white = matches!(user_color.unwrap_or("white"), "white" | "w");
    let move_is_white = matches!(move_color, "white" | "w");
    user_is_white == move_is_white
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

/// Build a prompt for Omarchy's default agent (same style as `omarchy agent crash`).
fn agent_prompt(game: &Game, pb: &Playback) -> String {
    let meta = game_meta_line(game);
    let max = pb.fens.len().saturating_sub(1);
    let fen = pb.fens.get(pb.ply).cloned().unwrap_or_default();
    let (head, body) = moment_for_ply(pb);
    let opp = game.opponent.as_deref().unwrap_or("unknown");
    let result = game.result.as_deref().unwrap_or("—");
    let color = game.user_color.as_deref().unwrap_or("white");

    let mut prompt = String::new();
    prompt.push_str(
        "Analyze this move and describe why it is good or bad and how it can be improved.\n\n",
    );
    prompt.push_str("What the board is showing:\n");
    if !meta.is_empty() {
        prompt.push_str(&format!("  game:     {meta}\n"));
    }
    prompt.push_str(&format!("  opponent: {opp}\n"));
    prompt.push_str(&format!("  my color: {color}\n"));
    prompt.push_str(&format!("  result:   {result}\n"));
    prompt.push_str(&format!("  ply:      {}/{max}\n", pb.ply));
    prompt.push_str(&format!("  FEN:      {fen}\n"));
    if !head.is_empty() {
        prompt.push_str("\nMoment text above the board:\n");
        prompt.push_str(&format!("  {head}\n"));
        if !body.is_empty() {
            for line in body.lines() {
                prompt.push_str(&format!("  {line}\n"));
            }
        }
    }
    prompt
}
