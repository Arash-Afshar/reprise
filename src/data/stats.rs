//! Progress aggregates: Elo timeline, blunder moving average, motif categories.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;

use super::db::Library;
use crate::chess_util::{board_from_fen, pieces_from_fen, Role};

#[derive(Debug, Clone)]
pub struct EloPoint {
    pub at: DateTime<Utc>,
    pub elo: i32,
}

#[derive(Debug, Clone)]
pub struct BlunderMaPoint {
    pub at: DateTime<Utc>,
    /// Rolling mean of blunders per game over the last `window` analyzed games.
    pub avg: f64,
}

#[derive(Debug, Clone)]
pub struct BlunderCategoryCount {
    pub label: String,
    pub count: usize,
}

/// One time-control lane on the Progress dashboard.
#[derive(Debug, Clone)]
pub struct TimeControlProgress {
    /// Raw chess.com / PGN time control (e.g. `900+10`, `600`).
    pub key: String,
    /// Display label (e.g. `15+10`, `10 min`).
    pub label: String,
    pub elo: Vec<EloPoint>,
    pub blunder_ma: Vec<BlunderMaPoint>,
}

#[derive(Debug, Clone)]
pub struct AnalysisProgressStats {
    /// Blunder MA per time control (Elo is filled in by the UI from the index).
    pub by_time_control: Vec<TimeControlProgress>,
    pub categories: Vec<BlunderCategoryCount>,
}

/// Human label for a chess.com-style time control (`900+10` → `15+10`, `600` → `10 min`).
pub fn format_time_control(tc: &str) -> String {
    let tc = tc.trim();
    if tc.is_empty() {
        return "Unknown".to_string();
    }
    let (base, inc) = match tc.split_once('+') {
        Some((b, i)) => (b.trim(), Some(i.trim())),
        None => (tc, None),
    };
    let Ok(secs) = base.parse::<u32>() else {
        return tc.to_string();
    };
    let base_disp = if secs % 60 == 0 {
        (secs / 60).to_string()
    } else {
        format!("{secs}s")
    };
    match inc {
        Some(i) if !i.is_empty() => format!("{base_disp}+{i}"),
        _ if secs % 60 == 0 => format!("{} min", secs / 60),
        _ => format!("{secs}s"),
    }
}

fn time_control_key(raw: Option<&str>) -> String {
    let s = raw.map(str::trim).unwrap_or("");
    if s.is_empty() {
        "unknown".to_string()
    } else {
        s.to_string()
    }
}

/// User Elo over time, one series per time control.
pub fn elo_by_time_control(library: &Library) -> Vec<TimeControlProgress> {
    let mut groups: HashMap<String, Vec<EloPoint>> = HashMap::new();
    for g in &library.games {
        let at = match g.played_at {
            Some(at) => at,
            None => continue,
        };
        let elo = match g.user_color.as_deref() {
            Some("black") => g.black_elo,
            _ => g.white_elo,
        };
        let Some(elo) = elo else { continue };
        let key = time_control_key(g.time_control.as_deref());
        groups.entry(key).or_default().push(EloPoint { at, elo });
    }

    let mut series: Vec<TimeControlProgress> = groups
        .into_iter()
        .map(|(key, mut elo)| {
            elo.sort_by_key(|p| p.at);
            let label = if key == "unknown" {
                "Unknown".to_string()
            } else {
                format_time_control(&key)
            };
            TimeControlProgress {
                key,
                label,
                elo,
                blunder_ma: Vec::new(),
            }
        })
        .collect();
    sort_time_control_series(&mut series);
    series
}

/// Blunder counts (5-game MA) per time control + top motif categories (all games).
pub fn analysis_progress_stats(
    db_path: &Path,
    ma_window: usize,
    category_limit: usize,
) -> Result<AnalysisProgressStats> {
    let t0 = Instant::now();
    let raw = std::fs::read_to_string(db_path)
        .with_context(|| format!("reading {}", db_path.display()))?;
    let root: Value =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", db_path.display()))?;
    let Some(games) = root.get("games").and_then(|g| g.as_array()) else {
        return Ok(AnalysisProgressStats {
            by_time_control: Vec::new(),
            categories: Vec::new(),
        });
    };

    let mut per_tc: HashMap<String, Vec<(DateTime<Utc>, usize)>> = HashMap::new();
    let mut category_counts: HashMap<&'static str, usize> = HashMap::new();
    let mut scanned = 0usize;

    for g in games {
        let Some(analysis) = g.get("analysis") else {
            continue;
        };
        let Some(moves) = analysis.get("moves").and_then(|m| m.as_array()) else {
            continue;
        };
        let user_color = g
            .get("userColor")
            .and_then(|v| v.as_str())
            .unwrap_or("white");
        let played_at = g
            .get("playedAt")
            .and_then(|v| v.as_str())
            .and_then(parse_played_at);
        let key = time_control_key(g.get("timeControl").and_then(|v| v.as_str()));

        let mut blunders = 0usize;
        for m in moves {
            let move_color = m.get("color").and_then(|v| v.as_str()).unwrap_or("");
            if !color_is_mine(move_color, user_color) {
                continue;
            }
            let class = m
                .get("classification")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if class != "blunder" {
                continue;
            }
            blunders += 1;
            let cat = categorize_blunder(m, user_color);
            *category_counts.entry(cat).or_insert(0) += 1;
        }

        scanned += 1;
        if let Some(at) = played_at {
            per_tc.entry(key).or_default().push((at, blunders));
        }
    }

    let window = ma_window.max(1);
    let mut by_time_control: Vec<TimeControlProgress> = per_tc
        .into_iter()
        .map(|(key, mut per_game)| {
            per_game.sort_by_key(|(at, _)| *at);
            let mut blunder_ma = Vec::with_capacity(per_game.len());
            for i in 0..per_game.len() {
                let start = i.saturating_sub(window - 1);
                let slice = &per_game[start..=i];
                let sum: usize = slice.iter().map(|(_, n)| *n).sum();
                let avg = sum as f64 / slice.len() as f64;
                blunder_ma.push(BlunderMaPoint {
                    at: per_game[i].0,
                    avg,
                });
            }
            let label = if key == "unknown" {
                "Unknown".to_string()
            } else {
                format_time_control(&key)
            };
            TimeControlProgress {
                key,
                label,
                elo: Vec::new(),
                blunder_ma,
            }
        })
        .collect();
    sort_time_control_series(&mut by_time_control);

    let mut categories: Vec<BlunderCategoryCount> = category_counts
        .into_iter()
        .map(|(label, count)| BlunderCategoryCount {
            label: label.to_string(),
            count,
        })
        .collect();
    categories.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.label.cmp(&b.label)));
    categories.truncate(category_limit);

    eprintln!(
        "reprise: progress stats — {scanned} analyzed games, {} time controls, top categories in {:.2?}",
        by_time_control.len(),
        t0.elapsed()
    );

    Ok(AnalysisProgressStats {
        by_time_control,
        categories,
    })
}

/// Merge Elo (index) + blunder MA (db) series by time-control key.
pub fn merge_progress_series(
    elo: Vec<TimeControlProgress>,
    blunders: Vec<TimeControlProgress>,
) -> Vec<TimeControlProgress> {
    let mut map: HashMap<String, TimeControlProgress> = HashMap::new();
    for s in elo {
        map.insert(s.key.clone(), s);
    }
    for s in blunders {
        match map.get_mut(&s.key) {
            Some(existing) => {
                existing.blunder_ma = s.blunder_ma;
            }
            None => {
                map.insert(s.key.clone(), s);
            }
        }
    }
    let mut out: Vec<TimeControlProgress> = map.into_values().collect();
    sort_time_control_series(&mut out);
    out
}

fn sort_time_control_series(series: &mut [TimeControlProgress]) {
    series.sort_by(|a, b| {
        let ca = a.elo.len().max(a.blunder_ma.len());
        let cb = b.elo.len().max(b.blunder_ma.len());
        cb.cmp(&ca)
            .then_with(|| a.label.cmp(&b.label))
            .then_with(|| a.key.cmp(&b.key))
    });
}

fn parse_played_at(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|| {
            // chess.com sometimes omits offset; treat as UTC
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
                .or_else(|_| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S"))
                .ok()
                .map(|n| n.and_utc())
        })
}

fn color_is_mine(move_color: &str, user_color: &str) -> bool {
    let user_is_white = matches!(user_color, "white" | "w");
    let move_is_white = matches!(move_color, "white" | "w");
    user_is_white == move_is_white
}

/// Motif label for a single blunder (static str for cheap counting).
fn categorize_blunder(m: &Value, user_color: &str) -> &'static str {
    let user_is_white = matches!(user_color, "white" | "w");

    if score_is_mate_for(m.get("evalBeforeBest"), user_is_white) {
        return "Missed mate";
    }
    if score_is_mate_against(m.get("evalPlayed"), user_is_white) {
        return "Allowed mate";
    }

    let fen_after = m.get("fenAfter").and_then(|v| v.as_str()).unwrap_or("");
    if !fen_after.is_empty() && leaves_hanging_piece(fen_after) {
        return "Hanging piece";
    }

    let best_san = m
        .get("bestMoves")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|b| b.get("san"))
        .and_then(|s| s.as_str())
        .unwrap_or("");
    if best_san.contains('x') {
        return "Missed capture";
    }

    let fen_before = m.get("fenBefore").and_then(|v| v.as_str()).unwrap_or("");
    let ply = m.get("ply").and_then(|v| v.as_u64()).unwrap_or(0);
    if is_endgame(fen_before) {
        return "Endgame";
    }
    if ply <= 20 {
        return "Opening";
    }
    "Middlegame"
}

fn score_is_mate_for(score: Option<&Value>, _user_is_white: bool) -> bool {
    let Some(score) = score else {
        return false;
    };
    if score.get("type").and_then(|t| t.as_str()) != Some("mate") {
        return false;
    }
    // UCI score from the root (our move): positive mate = we deliver mate.
    score.get("value").and_then(|v| v.as_i64()).is_some_and(|v| v > 0)
}

fn score_is_mate_against(score: Option<&Value>, _user_is_white: bool) -> bool {
    let Some(score) = score else {
        return false;
    };
    if score.get("type").and_then(|t| t.as_str()) != Some("mate") {
        return false;
    }
    // Negative mate from our root = opponent mates us after this move.
    score.get("value").and_then(|v| v.as_i64()).is_some_and(|v| v < 0)
}

fn leaves_hanging_piece(fen_after: &str) -> bool {
    let Some(board) = board_from_fen(fen_after) else {
        return false;
    };
    for mv in board.gen_legal_moves() {
        if !board.is_capture(mv).unwrap_or(false) {
            continue;
        }
        let (file, rank) = mv.to_square();
        let Ok(Some(piece)) = board.occupant_of_square(file, rank) else {
            // En passant — treat as hanging pawn.
            return true;
        };
        if piece_type_cp(piece.piece_type()) >= 300 {
            return true;
        }
    }
    false
}

fn piece_type_cp(pt: rschess::PieceType) -> i32 {
    use rschess::PieceType::*;
    match pt {
        P => 100,
        N | B => 300,
        R => 500,
        Q => 900,
        K => 0,
    }
}

fn is_endgame(fen: &str) -> bool {
    let pieces = pieces_from_fen(fen);
    let non_pawn = pieces
        .iter()
        .filter(|p| !matches!(p.role, Role::King | Role::Pawn))
        .count();
    non_pawn <= 6
}
