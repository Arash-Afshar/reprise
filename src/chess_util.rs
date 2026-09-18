//! MIT-safe chess helpers (no GPL crates).
//! Move legality / SAN via `rschess` (MIT). Board occupancy via FEN parse.

use rschess::{Board, Fen, Move};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    White,
    Black,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    King,
    Queen,
    Rook,
    Bishop,
    Knight,
    Pawn,
}

#[derive(Debug, Clone, Copy)]
pub struct Square {
    pub file: u8, // 0=a .. 7=h
    pub rank: u8, // 0=1 .. 7=8
}

#[derive(Debug, Clone, Copy)]
pub struct PlacedPiece {
    pub square: Square,
    pub role: Role,
    pub color: Color,
}

impl Square {
    pub fn from_coords(file: u8, rank: u8) -> Option<Self> {
        if file < 8 && rank < 8 {
            Some(Self { file, rank })
        } else {
            None
        }
    }

    pub fn from_algebraic(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        if b.len() != 2 {
            return None;
        }
        let file = b[0].to_ascii_lowercase().wrapping_sub(b'a');
        let rank = b[1].wrapping_sub(b'1');
        Self::from_coords(file, rank)
    }
}

fn role_from_fen_char(c: char) -> Option<(Role, Color)> {
    let color = if c.is_ascii_uppercase() {
        Color::White
    } else {
        Color::Black
    };
    let role = match c.to_ascii_lowercase() {
        'k' => Role::King,
        'q' => Role::Queen,
        'r' => Role::Rook,
        'b' => Role::Bishop,
        'n' => Role::Knight,
        'p' => Role::Pawn,
        _ => return None,
    };
    Some((role, color))
}

/// Parse piece placement from a FEN string (first field only is enough).
pub fn pieces_from_fen(fen: &str) -> Vec<PlacedPiece> {
    let placement = fen.split_whitespace().next().unwrap_or("");
    let mut out = Vec::new();
    let mut rank: i32 = 7;
    let mut file: i32 = 0;
    for ch in placement.chars() {
        match ch {
            '/' => {
                rank -= 1;
                file = 0;
            }
            '1'..='8' => {
                file += ch.to_digit(10).unwrap_or(0) as i32;
            }
            c => {
                if let Some((role, color)) = role_from_fen_char(c) {
                    if let Some(square) = Square::from_coords(file as u8, rank as u8) {
                        out.push(PlacedPiece {
                            square,
                            role,
                            color,
                        });
                    }
                }
                file += 1;
            }
        }
    }
    out
}

pub fn start_fen() -> String {
    Board::default().to_fen().to_string()
}

pub fn end_fen_from_pgn(pgn: &str) -> Option<String> {
    Some(pgn_tail(pgn)?.fen)
}

/// Final position after replaying a PGN (ply count + last move UCI for highlights).
#[derive(Debug, Clone)]
pub struct PgnTail {
    pub fen: String,
    pub ply: usize,
    pub last_uci: Option<String>,
}

pub fn pgn_tail(pgn: &str) -> Option<PgnTail> {
    let mut board = Board::default();
    let mut ply = 0usize;
    let mut last_uci = None;
    for san in pgn_sans(pgn) {
        let Ok(mv) = board.san_to_move(&san) else {
            break;
        };
        let uci = mv.to_uci();
        if board.make_move(mv).is_err() {
            break;
        }
        last_uci = Some(uci);
        ply += 1;
    }
    Some(PgnTail {
        fen: board.to_fen().to_string(),
        ply,
        last_uci,
    })
}

pub fn replay_pgn_board(pgn: &str) -> Option<Board> {
    let mut board = Board::default();
    for token in pgn.split_whitespace() {
        let t = token
            .trim_matches(|c: char| c == ';' || c == '{' || c == '}')
            .trim_end_matches(|c: char| matches!(c, '!' | '?' | '#' | '+' | '.'));
        if t.is_empty() || t.starts_with('[') || t.chars().all(|c| c.is_ascii_digit() || c == '.') {
            continue;
        }
        if matches!(t, "1-0" | "0-1" | "1/2-1/2" | "*") {
            break;
        }
        if board.make_move_san(t).is_err() {
            continue;
        }
    }
    Some(board)
}

pub fn board_from_fen(fen: &str) -> Option<Board> {
    let fen: Fen = fen.try_into().ok()?;
    Some(Board::from_fen(fen))
}

pub fn apply_san_line(fen_before: &str, sans: &[String]) -> Option<Vec<String>> {
    apply_san_line_with_ucis(fen_before, sans).map(|(fens, _)| fens)
}

/// Replay a SAN PV from `fen_before`. Returns FENs (starting with `fen_before`)
/// and the UCI of each applied move.
pub fn apply_san_line_with_ucis(
    fen_before: &str,
    sans: &[String],
) -> Option<(Vec<String>, Vec<Option<String>>)> {
    let mut board = board_from_fen(fen_before)?;
    let mut fens = vec![board.to_fen().to_string()];
    let mut ucis = Vec::new();
    for san in sans {
        let mv = board.san_to_move(san).ok()?;
        let uci = mv.to_uci();
        board.make_move(mv).ok()?;
        ucis.push(Some(uci));
        fens.push(board.to_fen().to_string());
    }
    Some((fens, ucis))
}

pub fn best_move_squares(fen_before: &str, san: &str) -> Option<(Square, Square)> {
    let board = board_from_fen(fen_before)?;
    let mv = board.san_to_move(san).ok()?;
    squares_from_move(&mv)
}

pub fn squares_from_uci(uci: &str) -> Option<(Square, Square)> {
    let mv = Move::from_uci(uci).ok()?;
    squares_from_move(&mv)
}

fn squares_from_move(mv: &Move) -> Option<(Square, Square)> {
    let (ff, fr) = mv.from_square();
    let (tf, tr) = mv.to_square();
    let from = Square::from_algebraic(&format!("{ff}{fr}"))?;
    let to = Square::from_algebraic(&format!("{tf}{tr}"))?;
    Some((from, to))
}

/// Extract SAN tokens from a PGN, skipping headers, clocks, move numbers, and results.
pub fn pgn_sans(pgn: &str) -> Vec<String> {
    let movetext = match pgn.find("\n\n") {
        Some(i) => &pgn[i + 2..],
        None => {
            let mut start = 0;
            for line in pgn.lines() {
                let t = line.trim();
                if t.starts_with('[') {
                    start += line.len() + 1;
                    continue;
                }
                break;
            }
            &pgn[start.min(pgn.len())..]
        }
    };

    let mut cleaned = String::with_capacity(movetext.len());
    let mut depth = 0i32;
    for ch in movetext.chars() {
        match ch {
            '{' => depth += 1,
            '}' => depth = (depth - 1).max(0),
            _ if depth == 0 => cleaned.push(ch),
            _ => {}
        }
    }

    let mut sans = Vec::new();
    for token in cleaned.split_whitespace() {
        let t = token.trim_end_matches(|c: char| matches!(c, '!' | '?' | '#' | '+'));
        if t.is_empty() {
            continue;
        }
        if matches!(t, "1-0" | "0-1" | "1/2-1/2" | "*") {
            break;
        }
        if t.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
            continue;
        }
        if t.starts_with('(') || t.starts_with('$') {
            continue;
        }
        sans.push(t.to_string());
    }
    sans
}

/// Map board coords (0..8 from bottom-left of *display*).
pub fn display_to_square(file: i32, rank_from_bottom: i32, white_at_bottom: bool) -> Option<Square> {
    if !(0..8).contains(&file) || !(0..8).contains(&rank_from_bottom) {
        return None;
    }
    let (f, r) = if white_at_bottom {
        (file as u8, rank_from_bottom as u8)
    } else {
        ((7 - file) as u8, (7 - rank_from_bottom) as u8)
    };
    Square::from_coords(f, r)
}
