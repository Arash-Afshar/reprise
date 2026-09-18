use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk4::cairo::{Format, ImageSurface};
use gtk4::prelude::*;
use gtk4::{DrawingArea, Orientation};

use crate::chess_util::{
    display_to_square, pieces_from_fen, start_fen, Color, Role, Square,
};
use crate::theme::ThemeColors;

const PIECE_SVGS: [(&str, &str); 12] = [
    ("wK", include_str!("../../assets/pieces/wK.svg")),
    ("wQ", include_str!("../../assets/pieces/wQ.svg")),
    ("wR", include_str!("../../assets/pieces/wR.svg")),
    ("wB", include_str!("../../assets/pieces/wB.svg")),
    ("wN", include_str!("../../assets/pieces/wN.svg")),
    ("wP", include_str!("../../assets/pieces/wP.svg")),
    ("bK", include_str!("../../assets/pieces/bK.svg")),
    ("bQ", include_str!("../../assets/pieces/bQ.svg")),
    ("bR", include_str!("../../assets/pieces/bR.svg")),
    ("bB", include_str!("../../assets/pieces/bB.svg")),
    ("bN", include_str!("../../assets/pieces/bN.svg")),
    ("bP", include_str!("../../assets/pieces/bP.svg")),
];

#[derive(Clone, Default)]
struct BoardState {
    fen: String,
    white_at_bottom: bool,
    highlight_from: Option<Square>,
    highlight_to: Option<Square>,
    arrow_from: Option<Square>,
    arrow_to: Option<Square>,
}

type PieceCache = HashMap<(String, i32), ImageSurface>;

#[derive(Clone)]
pub struct BoardView {
    pub widget: gtk4::Box,
    area: DrawingArea,
    state: Rc<RefCell<BoardState>>,
    #[allow(dead_code)]
    colors: Rc<RefCell<ThemeColors>>,
    #[allow(dead_code)]
    cache: Rc<RefCell<PieceCache>>,
}

impl BoardView {
    pub fn new(size: i32, colors: Rc<RefCell<ThemeColors>>) -> Self {
        let state = Rc::new(RefCell::new(BoardState {
            fen: start_fen(),
            white_at_bottom: true,
            ..Default::default()
        }));
        let cache = Rc::new(RefCell::new(PieceCache::new()));

        let area = DrawingArea::new();
        area.set_content_width(size);
        area.set_content_height(size);
        area.set_size_request(size, size);
        area.set_hexpand(false);
        area.set_vexpand(false);
        area.add_css_class("board");

        let state_draw = state.clone();
        let colors_draw = colors.clone();
        let cache_draw = cache.clone();
        area.set_draw_func(move |_area, cr, w, h| {
            draw_board(
                cr,
                w,
                h,
                &state_draw.borrow(),
                &colors_draw.borrow(),
                &mut cache_draw.borrow_mut(),
            );
        });

        let widget = gtk4::Box::new(Orientation::Vertical, 0);
        widget.append(&area);

        let _ = display_to_square;

        Self {
            widget,
            area,
            state,
            colors,
            cache,
        }
    }

    pub fn queue_draw(&self) {
        self.area.queue_draw();
    }

    pub fn set_size(&self, size: i32) {
        self.area.set_content_width(size);
        self.area.set_content_height(size);
        self.area.set_size_request(size, size);
        self.area.queue_draw();
    }

    pub fn set_fen(&self, fen: &str) {
        self.state.borrow_mut().fen = fen.to_string();
        self.area.queue_draw();
    }

    pub fn set_orientation_white_bottom(&self, white_at_bottom: bool) {
        self.state.borrow_mut().white_at_bottom = white_at_bottom;
        self.area.queue_draw();
    }

    pub fn set_last_move(&self, from: Option<Square>, to: Option<Square>) {
        let mut s = self.state.borrow_mut();
        s.highlight_from = from;
        s.highlight_to = to;
        self.area.queue_draw();
    }

    pub fn set_arrow(&self, from: Option<Square>, to: Option<Square>) {
        let mut s = self.state.borrow_mut();
        s.arrow_from = from;
        s.arrow_to = to;
        self.area.queue_draw();
    }

    pub fn flip(&self) {
        let mut s = self.state.borrow_mut();
        s.white_at_bottom = !s.white_at_bottom;
        self.area.queue_draw();
    }
}

fn piece_key(role: Role, color: Color) -> &'static str {
    match (color, role) {
        (Color::White, Role::King) => "wK",
        (Color::White, Role::Queen) => "wQ",
        (Color::White, Role::Rook) => "wR",
        (Color::White, Role::Bishop) => "wB",
        (Color::White, Role::Knight) => "wN",
        (Color::White, Role::Pawn) => "wP",
        (Color::Black, Role::King) => "bK",
        (Color::Black, Role::Queen) => "bQ",
        (Color::Black, Role::Rook) => "bR",
        (Color::Black, Role::Bishop) => "bB",
        (Color::Black, Role::Knight) => "bN",
        (Color::Black, Role::Pawn) => "bP",
    }
}

fn svg_for(key: &str) -> Option<&'static str> {
    PIECE_SVGS.iter().find(|(k, _)| *k == key).map(|(_, s)| *s)
}

fn rasterize_piece(svg: &str, size: i32) -> Option<ImageSurface> {
    let size = size.max(8) as u32;
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(size, size)?;
    let tw = tree.size().width().max(1.0);
    let th = tree.size().height().max(1.0);
    let scale = (size as f32 / tw).min(size as f32 / th);
    let tx = (size as f32 - tw * scale) * 0.5;
    let ty = (size as f32 - th * scale) * 0.5;
    let transform = tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, tx, ty);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let stride = Format::ARgb32.stride_for_width(size).ok()?;
    let mut data = vec![0u8; (stride * size as i32) as usize];
    let src = pixmap.data();
    for y in 0..size as usize {
        for x in 0..size as usize {
            let i = (y * size as usize + x) * 4;
            let r = src[i];
            let g = src[i + 1];
            let b = src[i + 2];
            let a = src[i + 3];
            let o = y * stride as usize + x * 4;
            data[o] = b;
            data[o + 1] = g;
            data[o + 2] = r;
            data[o + 3] = a;
        }
    }

    ImageSurface::create_for_data(data, Format::ARgb32, size as i32, size as i32, stride).ok()
}

fn piece_surface(cache: &mut PieceCache, key: &str, size: i32) -> Option<ImageSurface> {
    let cache_key = (key.to_string(), size);
    if let Some(surf) = cache.get(&cache_key) {
        return Some(surf.clone());
    }
    let svg = svg_for(key)?;
    let surf = rasterize_piece(svg, size)?;
    cache.insert(cache_key, surf.clone());
    Some(surf)
}

fn draw_board(
    cr: &gtk4::cairo::Context,
    w: i32,
    h: i32,
    state: &BoardState,
    colors: &ThemeColors,
    cache: &mut PieceCache,
) {
    let side = w.min(h) as f64;
    let ox = (w as f64 - side) / 2.0;
    let oy = (h as f64 - side) / 2.0;
    let sq = side / 8.0;

    // Dedicated board palette — independent of Omarchy chrome so the board pops.
    // Warm parchment / forest green: cool Everforest UI + warm playable surface.
    let light = hex_rgb(0xE8, 0xDC, 0xC4);
    let dark = hex_rgb(0x5E, 0x8F, 0x65);
    // Last-move: amber that reads on both light and dark squares.
    let hl_from = (0.96, 0.78, 0.22, 0.42);
    let hl_to = (0.96, 0.72, 0.12, 0.58);
    // Arrow: coral — complementary to green squares, high contrast with pieces.
    let arrow = (0.92, 0.40, 0.32, 0.92);

    // Match the page chrome outside the 8×8 so nothing looks letterboxed.
    let page = ThemeColors::rgb(&colors.background);
    cr.set_source_rgb(page.0, page.1, page.2);
    cr.paint().ok();

    // Checkerboard: a1 is always dark. Display (0,0) is bottom-left — a1 when
    // White is at the bottom, h8 when Black is (both dark under a 180° flip).
    for rank in 0..8 {
        for file in 0..8 {
            let dark_sq = (file + rank) % 2 == 0;
            let (r, g, b) = if dark_sq { dark } else { light };
            cr.set_source_rgb(r, g, b);
            let x = ox + file as f64 * sq;
            let y = oy + (7 - rank) as f64 * sq;
            cr.rectangle(x, y, sq, sq);
            cr.fill().ok();
        }
    }

    let paint_hl = |cr: &gtk4::cairo::Context, square: Square, rgba: (f64, f64, f64, f64)| {
        let (file, rank) = square_to_display(square, state.white_at_bottom);
        cr.set_source_rgba(rgba.0, rgba.1, rgba.2, rgba.3);
        cr.rectangle(ox + file as f64 * sq, oy + (7 - rank) as f64 * sq, sq, sq);
        cr.fill().ok();
    };
    if let Some(s) = state.highlight_from {
        paint_hl(cr, s, hl_from);
    }
    if let Some(s) = state.highlight_to {
        paint_hl(cr, s, hl_to);
    }

    draw_coordinates(cr, ox, oy, sq, state.white_at_bottom, light, dark);

    let piece_px = (sq * 0.92).round().max(8.0) as i32;
    let pad = (sq - piece_px as f64) * 0.5;

    for piece in pieces_from_fen(&state.fen) {
        let (file, rank) = square_to_display(piece.square, state.white_at_bottom);
        let x = ox + file as f64 * sq + pad;
        let y = oy + (7 - rank) as f64 * sq + pad;
        let key = piece_key(piece.role, piece.color);
        if let Some(surf) = piece_surface(cache, key, piece_px) {
            cr.set_source_surface(&surf, x, y).ok();
            cr.paint().ok();
        }
    }

    if let (Some(from), Some(to)) = (state.arrow_from, state.arrow_to) {
        let (ff, fr) = square_to_display(from, state.white_at_bottom);
        let (tf, tr) = square_to_display(to, state.white_at_bottom);
        let x1 = ox + (ff as f64 + 0.5) * sq;
        let y1 = oy + ((7 - fr) as f64 + 0.5) * sq;
        let x2 = ox + (tf as f64 + 0.5) * sq;
        let y2 = oy + ((7 - tr) as f64 + 0.5) * sq;
        draw_arrow(cr, x1, y1, x2, y2, sq, arrow);
    }
}

fn hex_rgb(r: u8, g: u8, b: u8) -> (f64, f64, f64) {
    (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0)
}

/// File letters on the bottom row and rank numbers on the left file, inside
/// each square at its bottom-left (follows board orientation).
fn draw_coordinates(
    cr: &gtk4::cairo::Context,
    ox: f64,
    oy: f64,
    sq: f64,
    white_at_bottom: bool,
    light: (f64, f64, f64),
    dark: (f64, f64, f64),
) {
    use gtk4::cairo::{FontSlant, FontWeight};

    let font_size = (sq * 0.18).max(8.0);
    let inset = sq * 0.07;
    cr.select_font_face("Sans", FontSlant::Normal, FontWeight::Bold);
    cr.set_font_size(font_size);

    for disp_file in 0..8 {
        for disp_rank in 0..8 {
            let on_bottom = disp_rank == 0;
            let on_left = disp_file == 0;
            if !on_bottom && !on_left {
                continue;
            }

            let (chess_file, chess_rank) = if white_at_bottom {
                (disp_file, disp_rank)
            } else {
                (7 - disp_file, 7 - disp_rank)
            };

            let x = ox + disp_file as f64 * sq;
            let y = oy + (7 - disp_rank) as f64 * sq;
            let dark_sq = (disp_file + disp_rank) % 2 == 0;
            let (r, g, b) = if dark_sq { light } else { dark };
            cr.set_source_rgb(r, g, b);

            let baseline = y + sq - inset;
            if on_left && on_bottom {
                // Corner: rank above file, both bottom-left.
                let rank_lbl = format!("{}", chess_rank + 1);
                let file_lbl = format!("{}", (b'a' + chess_file as u8) as char);
                cr.move_to(x + inset, baseline - font_size * 0.95);
                cr.show_text(&rank_lbl).ok();
                cr.move_to(x + inset, baseline);
                cr.show_text(&file_lbl).ok();
            } else if on_left {
                let rank_lbl = format!("{}", chess_rank + 1);
                cr.move_to(x + inset, baseline);
                cr.show_text(&rank_lbl).ok();
            } else if on_bottom {
                let file_lbl = format!("{}", (b'a' + chess_file as u8) as char);
                cr.move_to(x + inset, baseline);
                cr.show_text(&file_lbl).ok();
            }
        }
    }
}

fn draw_arrow(
    cr: &gtk4::cairo::Context,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    sq: f64,
    rgba: (f64, f64, f64, f64),
) {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let len = (dx * dx + dy * dy).sqrt().max(1.0);
    let ux = dx / len;
    let uy = dy / len;
    let px = -uy;
    let py = ux;

    // Single filled polygon — avoids Round line caps that read as a blob on the tip.
    let tip_x = x2 - ux * (sq * 0.08);
    let tip_y = y2 - uy * (sq * 0.08);
    let head_len = sq * 0.34;
    let head_half = sq * 0.20;
    let shaft_half = sq * 0.07;
    let base_x = tip_x - ux * head_len;
    let base_y = tip_y - uy * head_len;

    cr.set_source_rgba(rgba.0, rgba.1, rgba.2, rgba.3);
    cr.move_to(tip_x, tip_y);
    cr.line_to(base_x + px * head_half, base_y + py * head_half);
    cr.line_to(base_x + px * shaft_half, base_y + py * shaft_half);
    cr.line_to(x1 + px * shaft_half, y1 + py * shaft_half);
    cr.line_to(x1 - px * shaft_half, y1 - py * shaft_half);
    cr.line_to(base_x - px * shaft_half, base_y - py * shaft_half);
    cr.line_to(base_x - px * head_half, base_y - py * head_half);
    cr.close_path();
    cr.fill().ok();
}

fn square_to_display(square: Square, white_at_bottom: bool) -> (i32, i32) {
    let file = square.file as i32;
    let rank = square.rank as i32;
    if white_at_bottom {
        (file, rank)
    } else {
        (7 - file, 7 - rank)
    }
}
