use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Align, Button, Label, Orientation};

use crate::chess_util::{apply_san_line, best_move_squares, squares_from_uci};
use crate::data::{AnalyzedMove, Game};
use crate::theme::ThemeColors;
use crate::ui::board::BoardView;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Walking the short spine of pivotal moments — Reprise's primary loop.
    Walk,
    /// Free step-through of the full game.
    StepThrough,
    /// Temporary lens on an alternative line from the current pivotal moment.
    Fork,
}

struct ReviewState {
    game: Game,
    /// FENs after each ply (index 0 = start).
    fens: Vec<String>,
    sans: Vec<String>,
    ucis: Vec<Option<String>>,
    ply: usize,
    mode: Mode,
    pivotal_idx: usize,
    show_arrow: bool,
    fork_fens: Vec<String>,
    fork_ply: usize,
}

pub struct ReviewPage {
    pub widget: gtk4::Box,
    board: BoardView,
    headline: Label,
    moment: Label,
    teaching: Label,
    mode_label: Label,
    state: Rc<RefCell<Option<ReviewState>>>,
}

impl ReviewPage {
    pub fn new(colors: Rc<RefCell<ThemeColors>>) -> Self {
        let widget = gtk4::Box::new(Orientation::Horizontal, 22);
        widget.add_css_class("page");
        widget.add_css_class("review");
        widget.set_margin_top(20);
        widget.set_margin_bottom(20);
        widget.set_margin_start(24);
        widget.set_margin_end(24);

        let board = BoardView::new(480, colors);

        let side = gtk4::Box::new(Orientation::Vertical, 12);
        side.set_size_request(320, -1);
        side.set_hexpand(true);

        let headline = Label::new(Some("No game open"));
        headline.add_css_class("title");
        headline.set_halign(Align::Start);
        headline.set_wrap(true);

        let mode_label = Label::new(Some("Walk"));
        mode_label.add_css_class("eyebrow");
        mode_label.set_halign(Align::Start);

        let moment = Label::new(Some(
            "Open a game from Still open. Reprise walks pivotal moments — the handful of decisions that turned the story.",
        ));
        moment.add_css_class("moment");
        moment.set_halign(Align::Start);
        moment.set_wrap(true);

        let teaching = Label::new(None);
        teaching.add_css_class("blurb");
        teaching.set_halign(Align::Start);
        teaching.set_wrap(true);

        let controls = gtk4::Box::new(Orientation::Horizontal, 8);
        let btn_prev = Button::with_label("prev");
        let btn_next = Button::with_label("next");
        let btn_fork = Button::with_label("fork best");
        let btn_step = Button::with_label("step");
        let btn_flip = Button::with_label("flip");
        for b in [&btn_prev, &btn_next, &btn_fork, &btn_step, &btn_flip] {
            b.add_css_class("ghost");
            controls.append(b);
        }

        let hint = Label::new(Some(
            "n / p  pivotal moments · ← → step · e fork · a arrow · f flip · Esc back",
        ));
        hint.add_css_class("hint");
        hint.set_halign(Align::Start);
        hint.set_wrap(true);

        side.append(&mode_label);
        side.append(&headline);
        side.append(&moment);
        side.append(&teaching);
        side.append(&controls);
        side.append(&hint);

        widget.append(&board.widget);
        widget.append(&side);

        let state: Rc<RefCell<Option<ReviewState>>> = Rc::new(RefCell::new(None));

        let page = Self {
            widget,
            board,
            headline,
            moment,
            teaching,
            mode_label,
            state,
        };

        page.bind_controls(btn_prev, btn_next, btn_fork, btn_step, btn_flip);
        page
    }

    fn bind_controls(
        &self,
        btn_prev: Button,
        btn_next: Button,
        btn_fork: Button,
        btn_step: Button,
        btn_flip: Button,
    ) {
        let s = self.state.clone();
        let board = self.board.clone();
        let headline = self.headline.clone();
        let moment = self.moment.clone();
        let teaching = self.teaching.clone();
        let mode_label = self.mode_label.clone();

        let redraw = {
            let s = s.clone();
            let board = board.clone();
            let headline = headline.clone();
            let moment = moment.clone();
            let teaching = teaching.clone();
            let mode_label = mode_label.clone();
            Rc::new(move || {
                refresh_view(&s, &board, &headline, &moment, &teaching, &mode_label);
            })
        };

        btn_next.connect_clicked({
            let s = s.clone();
            let redraw = redraw.clone();
            move |_| {
                step_pivotal(&s, 1);
                redraw();
            }
        });
        btn_prev.connect_clicked({
            let s = s.clone();
            let redraw = redraw.clone();
            move |_| {
                step_pivotal(&s, -1);
                redraw();
            }
        });
        btn_fork.connect_clicked({
            let s = s.clone();
            let redraw = redraw.clone();
            move |_| {
                enter_fork_best(&s);
                redraw();
            }
        });
        btn_step.connect_clicked({
            let s = s.clone();
            let redraw = redraw.clone();
            move |_| {
                if let Some(st) = s.borrow_mut().as_mut() {
                    st.mode = Mode::StepThrough;
                }
                redraw();
            }
        });
        btn_flip.connect_clicked({
            let board = board.clone();
            move |_| board.flip()
        });

        let key_controller = gtk4::EventControllerKey::new();
        key_controller.connect_key_pressed({
            let s = s.clone();
            let redraw = redraw.clone();
            let board = board.clone();
            move |_, key, _code, _mods| {
                use gtk4::gdk::Key;
                match key {
                    Key::n | Key::N => {
                        step_pivotal(&s, 1);
                        redraw();
                        glib::Propagation::Stop
                    }
                    Key::p | Key::P => {
                        step_pivotal(&s, -1);
                        redraw();
                        glib::Propagation::Stop
                    }
                    Key::Left | Key::h => {
                        step_through(&s, -1);
                        redraw();
                        glib::Propagation::Stop
                    }
                    Key::Right | Key::l => {
                        step_through(&s, 1);
                        redraw();
                        glib::Propagation::Stop
                    }
                    Key::e | Key::E => {
                        enter_fork_best(&s);
                        redraw();
                        glib::Propagation::Stop
                    }
                    Key::a | Key::A => {
                        if let Some(st) = s.borrow_mut().as_mut() {
                            st.show_arrow = !st.show_arrow;
                        }
                        redraw();
                        glib::Propagation::Stop
                    }
                    Key::f | Key::F => {
                        board.flip();
                        glib::Propagation::Stop
                    }
                    Key::Escape => {
                        if let Some(st) = s.borrow_mut().as_mut() {
                            if st.mode == Mode::Fork {
                                st.mode = Mode::Walk;
                            }
                        }
                        redraw();
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                }
            }
        });
        self.widget.add_controller(key_controller);
        self.widget.set_focusable(true);
    }

    pub fn open_game(&self, game: &Game) {
        let (fens, sans, ucis) = build_ply_timeline(game);
        let white_bottom = game.user_color.as_deref() != Some("black");
        self.board.set_orientation_white_bottom(white_bottom);

        let pivotal_moments = game.pivotal_moments();
        let start_ply = pivotal_moments.first().map(|m| m.ply as usize).unwrap_or(0);

        *self.state.borrow_mut() = Some(ReviewState {
            game: game.clone(),
            fens,
            sans,
            ucis,
            ply: start_ply,
            mode: Mode::Walk,
            pivotal_idx: 0,
            show_arrow: true,
            fork_fens: Vec::new(),
            fork_ply: 0,
        });

        self.widget.grab_focus();
        refresh_view(
            &self.state,
            &self.board,
            &self.headline,
            &self.moment,
            &self.teaching,
            &self.mode_label,
        );
    }

    pub fn repaint_board(&self) {
        self.board.queue_draw();
    }

    pub fn board_repaint_callback(&self) -> Rc<dyn Fn()> {
        let board = self.board.clone();
        Rc::new(move || board.queue_draw())
    }
}

fn build_ply_timeline(game: &Game) -> (Vec<String>, Vec<String>, Vec<Option<String>>) {
    if let Some(analysis) = &game.analysis {
        if !analysis.moves.is_empty() {
            let mut fens = vec![analysis.moves[0].fen_before.clone()];
            let mut sans = Vec::new();
            let mut ucis = Vec::new();
            for m in &analysis.moves {
                fens.push(m.fen_after.clone());
                sans.push(m.san.clone());
                ucis.push(m.uci.clone());
            }
            return (fens, sans, ucis);
        }
    }

    // Fallback: parse PGN without analysis (rschess, MIT).
    let mut board = rschess::Board::default();
    let mut fens = vec![board.to_fen().to_string()];
    let mut sans = Vec::new();
    let mut ucis = Vec::new();
    for token in game.pgn.split_whitespace() {
        let t = token.trim_end_matches(|c: char| matches!(c, '!' | '?'));
        if t.is_empty() || t.starts_with('[') || t.chars().all(|c| c.is_ascii_digit() || c == '.') {
            continue;
        }
        if matches!(t, "1-0" | "0-1" | "1/2-1/2" | "*") {
            break;
        }
        let Ok(mv) = board.san_to_move(t) else { continue };
        let uci = mv.to_uci();
        if board.make_move(mv).is_err() {
            break;
        }
        sans.push(t.to_string());
        ucis.push(Some(uci));
        fens.push(board.to_fen().to_string());
    }
    (fens, sans, ucis)
}

fn step_pivotal(state: &Rc<RefCell<Option<ReviewState>>>, dir: i32) {
    let mut borrow = state.borrow_mut();
    let Some(st) = borrow.as_mut() else { return };
    let pivotal_moments = st.game.pivotal_moments();
    if pivotal_moments.is_empty() {
        st.mode = Mode::StepThrough;
        return;
    }
    st.mode = Mode::Walk;
    let len = pivotal_moments.len() as i32;
    let next = (st.pivotal_idx as i32 + dir).rem_euclid(len) as usize;
    st.pivotal_idx = next;
    st.ply = pivotal_moments[next].ply as usize;
}

fn step_through(state: &Rc<RefCell<Option<ReviewState>>>, dir: i32) {
    let mut borrow = state.borrow_mut();
    let Some(st) = borrow.as_mut() else { return };
    if st.mode == Mode::Fork {
        let max = st.fork_fens.len().saturating_sub(1) as i32;
        st.fork_ply = (st.fork_ply as i32 + dir).clamp(0, max) as usize;
        return;
    }
    st.mode = Mode::StepThrough;
    let max = st.fens.len().saturating_sub(1) as i32;
    st.ply = (st.ply as i32 + dir).clamp(0, max) as usize;
}

fn enter_fork_best(state: &Rc<RefCell<Option<ReviewState>>>) {
    let mut borrow = state.borrow_mut();
    let Some(st) = borrow.as_mut() else { return };
    let Some(am) = analyzed_at_ply(&st.game, st.ply) else {
        return;
    };
    let Some(best) = am.best_moves.first() else {
        return;
    };
    let Some(pv) = &best.pv_san else { return };
    if let Some(fens) = apply_san_line(&am.fen_before, pv) {
        st.fork_fens = fens;
        st.fork_ply = 1.min(st.fork_fens.len().saturating_sub(1));
        st.mode = Mode::Fork;
    }
}

fn analyzed_at_ply(game: &Game, ply: usize) -> Option<&AnalyzedMove> {
    game.analysis
        .as_ref()?
        .moves
        .iter()
        .find(|m| m.ply as usize == ply)
}

fn refresh_view(
    state: &Rc<RefCell<Option<ReviewState>>>,
    board: &BoardView,
    headline: &Label,
    moment: &Label,
    teaching: &Label,
    mode_label: &Label,
) {
    let borrow = state.borrow();
    let Some(st) = borrow.as_ref() else { return };

    headline.set_text(&format!(
        "{} · {}",
        st.game.title(),
        st.game.outcome_for_user()
    ));

    match st.mode {
        Mode::Walk => {
            mode_label.set_text("WALK · pivotal moments");
            let pivotal_moments = st.game.pivotal_moments();
            if pivotal_moments.is_empty() {
                moment.set_text("No pivotal moments in this game. Step through the moves quietly.");
                teaching.set_text("");
                if let Some(fen) = st.fens.get(st.ply) {
                    board.set_fen(fen);
                }
                board.set_arrow(None, None);
                return;
            }
            let pivotal = pivotal_moments[st.pivotal_idx.min(pivotal_moments.len() - 1)];
            let fen = st.fens.get(pivotal.ply as usize).cloned().unwrap_or_default();
            board.set_fen(&fen);
            set_last_from_uci(board, pivotal.uci.as_deref());

            moment.set_text(&format!(
                "Pivotal Moment {}/{} · ply {} · {} played {} ({})",
                st.pivotal_idx + 1,
                pivotal_moments.len(),
                pivotal.ply,
                pivotal.color,
                pivotal.san,
                pivotal.classification
            ));

            let summary = pivotal
                .teaching
                .as_ref()
                .and_then(|t| t.summary.clone())
                .unwrap_or_else(|| {
                    format!(
                        "Eval {} → best {}",
                        pivotal.eval_played_text.as_deref().unwrap_or("?"),
                        pivotal.eval_before_best_text.as_deref().unwrap_or("?")
                    )
                });
            teaching.set_text(&summary);

            if st.show_arrow {
                set_best_arrow(board, pivotal);
            } else {
                board.set_arrow(None, None);
            }
        }
        Mode::StepThrough => {
            mode_label.set_text("STEP · full game");
            if let Some(fen) = st.fens.get(st.ply) {
                board.set_fen(fen);
            }
            if st.ply > 0 {
                set_last_from_uci(board, st.ucis.get(st.ply - 1).and_then(|u| u.as_deref()));
            } else {
                board.set_last_move(None, None);
            }
            let san = if st.ply == 0 {
                "start".into()
            } else {
                st.sans.get(st.ply - 1).cloned().unwrap_or_default()
            };
            moment.set_text(&format!("Ply {}/{} · {}", st.ply, st.fens.len().saturating_sub(1), san));
            if let Some(am) = analyzed_at_ply(&st.game, st.ply) {
                teaching.set_text(
                    am.teaching
                        .as_ref()
                        .and_then(|t| t.summary.as_deref())
                        .unwrap_or(""),
                );
                if st.show_arrow {
                    set_best_arrow(board, am);
                } else {
                    board.set_arrow(None, None);
                }
            } else {
                teaching.set_text("");
                board.set_arrow(None, None);
            }
        }
        Mode::Fork => {
            mode_label.set_text("FORK · other line");
            if let Some(fen) = st.fork_fens.get(st.fork_ply) {
                board.set_fen(fen);
            }
            board.set_last_move(None, None);
            board.set_arrow(None, None);
            moment.set_text(&format!(
                "Fork step {}/{} — Esc returns to the pivotal moment",
                st.fork_ply,
                st.fork_fens.len().saturating_sub(1)
            ));
            teaching.set_text("This is the line that kept the story intact.");
        }
    }
}

fn set_last_from_uci(board: &BoardView, uci: Option<&str>) {
    let Some(uci) = uci else {
        board.set_last_move(None, None);
        return;
    };
    if let Some((from, to)) = squares_from_uci(uci) {
        board.set_last_move(Some(from), Some(to));
    } else {
        board.set_last_move(None, None);
    }
}

fn set_best_arrow(board: &BoardView, am: &AnalyzedMove) {
    let Some(best) = am.best_moves.first() else {
        board.set_arrow(None, None);
        return;
    };
    if let Some(san_str) = &best.san {
        if let Some((from, to)) = best_move_squares(&am.fen_before, san_str) {
            board.set_arrow(Some(from), Some(to));
            return;
        }
    }
    board.set_arrow(None, None);
}
