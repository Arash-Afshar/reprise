//! End-of-week pack for the default Omarchy agent (Ctrl+E).

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, Datelike, Duration, Local, TimeZone, Utc};

use crate::data::{load_analyses_for, Game, GameAnalysis, Library};

/// Result of packing this week's games for the agent.
pub struct WeeklyReviewPack {
    pub game_count: usize,
    pub analyzed_count: usize,
    pub week_label: String,
    /// Relative path for the agent cwd, e.g. `chess/2026-09-14.md`.
    pub review_relpath: String,
    pub prompt: String,
}

/// Monday 00:00 – Sunday 23:59:59 in the local timezone, as UTC bounds + label.
pub fn current_week_bounds() -> (DateTime<Utc>, DateTime<Utc>, String) {
    let now = Local::now();
    let today = now.date_naive();
    let monday = today - Duration::days(today.weekday().num_days_from_monday() as i64);
    let sunday = monday + Duration::days(6);
    let start_local = monday
        .and_hms_opt(0, 0, 0)
        .expect("midnight")
        .and_local_timezone(Local)
        .single()
        .unwrap_or_else(|| Local.from_utc_datetime(&monday.and_hms_opt(0, 0, 0).unwrap()));
    let end_local = sunday
        .and_hms_opt(23, 59, 59)
        .expect("end of day")
        .and_local_timezone(Local)
        .single()
        .unwrap_or_else(|| Local.from_utc_datetime(&sunday.and_hms_opt(23, 59, 59).unwrap()));
    let label = format!(
        "{} – {}",
        monday.format("%Y-%m-%d"),
        sunday.format("%Y-%m-%d")
    );
    (
        start_local.with_timezone(&Utc),
        end_local.with_timezone(&Utc),
        label,
    )
}

/// Relative markdown path under the agent's `chess/` directory (Monday of the week).
pub fn review_relpath_for_week(week_label: &str) -> String {
    // week_label is "YYYY-MM-DD – YYYY-MM-DD"; file named by Monday start.
    let monday = week_label
        .split_once(' ')
        .map(|(d, _)| d)
        .unwrap_or(week_label);
    format!("chess/{monday}.md")
}

/// Games in `library` whose `played_at` falls in the current local week.
pub fn games_this_week(library: &Library) -> Vec<&Game> {
    let (start, end, _) = current_week_bounds();
    library
        .games
        .iter()
        .filter(|g| {
            g.played_at
                .map(|at| at >= start && at <= end)
                .unwrap_or(false)
        })
        .collect()
}

/// Build a token-efficient weekly review prompt (loads analyses from disk once).
pub fn build_pack(library: &Library) -> Result<WeeklyReviewPack> {
    let (start, end, week_label) = current_week_bounds();
    let week_games: Vec<&Game> = library
        .games
        .iter()
        .filter(|g| {
            g.played_at
                .map(|at| at >= start && at <= end)
                .unwrap_or(false)
        })
        .collect();

    let me = library
        .settings
        .my_username
        .as_deref()
        .or_else(|| week_games.iter().find_map(|g| g.my_username.as_deref()))
        .unwrap_or("me");

    let refs = game_refs(&week_games);

    let keys: Vec<(String, String)> = week_games
        .iter()
        .map(|g| (g.id.clone(), g.game_type.clone()))
        .collect();
    let analyses = load_analyses_for(Path::new(&library.path), &keys)?;

    let mut analyzed_count = 0usize;
    let mut body = String::new();
    for (game, game_ref) in week_games.iter().zip(refs.iter()) {
        let key = (game.id.clone(), game.game_type.clone());
        let analysis = analyses.get(&key);
        if analysis.is_some() {
            analyzed_count += 1;
        }
        body.push_str(&format_game(game_ref, game, analysis, me));
        body.push('\n');
    }

    let review_relpath = review_relpath_for_week(&week_label);
    let prompt = format!(
        "Weekly chess review for {me}. Week {week_label}. {} games ({} analyzed).\n\
         \n\
         Task: Review these games. Describe 4 mistakes to improve next week:\n\
         - 2 patterns common to both colors (mistakes I make as White and as Black)\n\
         - 1 mistake specific to when I play White\n\
         - 1 mistake specific to when I play Black\n\
         Link examples across games where you can. For each, give concise practical steps I can apply next week. No fluff — I will re-run this next week to check progress.\n\
         \n\
         When citing a game, use its title exactly (opponent name + date; with 1st/2nd/… when shown). \
         When citing a mistake, always include the ply number (e.g. ply 17) so I can jump to it in Reprise.\n\
         \n\
         Write the full review as markdown to {review_relpath} (create the chess/ directory if needed; path is relative to your working directory).\n\
         \n\
         Notation: me=my color; pivots=my top 5 worst moves by centipawn loss (ply N, SAN, class, loss, eval after play, best alternative).\n\
         \n\
         {body}",
        week_games.len(),
        analyzed_count,
    );

    Ok(WeeklyReviewPack {
        game_count: week_games.len(),
        analyzed_count,
        week_label,
        review_relpath,
        prompt,
    })
}

/// Opponent + local date; add 1st/2nd/… when the same pair appears more than once.
fn game_refs(games: &[&Game]) -> Vec<String> {
    let bases: Vec<String> = games.iter().map(|g| game_ref_base(g)).collect();
    let mut totals: HashMap<&str, usize> = HashMap::new();
    for b in &bases {
        *totals.entry(b.as_str()).or_insert(0) += 1;
    }
    let mut seen: HashMap<&str, usize> = HashMap::new();
    bases
        .iter()
        .map(|base| {
            let n = seen.entry(base.as_str()).or_insert(0);
            *n += 1;
            if totals.get(base.as_str()).copied().unwrap_or(1) > 1 {
                format!("{base} ({})", ordinal(*n))
            } else {
                base.clone()
            }
        })
        .collect()
}

fn game_ref_base(game: &Game) -> String {
    let opp = game
        .opponent
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown");
    let date = game
        .played_at
        .map(|at| at.with_timezone(&Local).format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "unknown-date".into());
    format!("{opp} {date}")
}

fn ordinal(n: usize) -> String {
    let mod100 = n % 100;
    let suffix = if (11..=13).contains(&mod100) {
        "th"
    } else {
        match n % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        }
    };
    format!("{n}{suffix}")
}

fn format_game(game_ref: &str, game: &Game, analysis: Option<&GameAnalysis>, me: &str) -> String {
    let color = game.user_color.as_deref().unwrap_or("?");
    let result = game.result.as_deref().unwrap_or("?");
    let outcome = game.outcome_for_user();
    let tc = game.time_control.as_deref().unwrap_or("?");
    let (my_elo, opp_elo) = match color {
        "black" => (game.black_elo, game.white_elo),
        _ => (game.white_elo, game.black_elo),
    };
    let elo = match (my_elo, opp_elo) {
        (Some(a), Some(b)) => format!(" elo {a}/{b}"),
        (Some(a), None) => format!(" elo {a}"),
        _ => String::new(),
    };
    let eco = game
        .eco
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|e| format!(" eco {e}"))
        .unwrap_or_default();

    let moves = compact_movetext(&game.pgn);

    let mut out = format!(
        "{game_ref} | tc={tc} me={color} ({me}){elo}{eco} {result} ({outcome})\n\
         moves: {moves}\n"
    );

    match analysis {
        None => out.push_str("pivots: (unanalyzed)\n"),
        Some(a) => {
            let mut game_with = (*game).clone();
            game_with.analysis = Some(a.clone());
            let pivots = game_with.pivotal_moments();
            if pivots.is_empty() {
                out.push_str("pivots: (none)\n");
            } else {
                out.push_str("pivots:\n");
                for m in pivots {
                    let loss = m
                        .loss_cp
                        .map(|cp| format!("{:.2}", cp as f64 / 100.0))
                        .unwrap_or_else(|| "?".into());
                    let played = m.eval_played_text.as_deref().unwrap_or("?");
                    let best_san = m
                        .best_moves
                        .first()
                        .and_then(|b| b.san.as_deref())
                        .unwrap_or("?");
                    let best_eval = m
                        .eval_before_best_text
                        .as_deref()
                        .or_else(|| {
                            m.best_moves
                                .first()
                                .and_then(|b| b.eval_text.as_deref())
                        })
                        .unwrap_or("?");
                    let pv = m
                        .best_moves
                        .first()
                        .and_then(|b| b.pv_san.as_ref())
                        .map(|v| {
                            let take = v.iter().take(4).cloned().collect::<Vec<_>>().join(" ");
                            if take.is_empty() {
                                String::new()
                            } else {
                                format!(" [{take}]")
                            }
                        })
                        .unwrap_or_default();
                    out.push_str(&format!(
                        "- ply {} {} {} -{loss} →{played} best {best_san}@{best_eval}{pv}\n",
                        m.ply, m.san, m.classification
                    ));
                }
            }
        }
    }
    out
}

/// Movetext only, clocks/comments stripped; black `N...` markers removed.
fn compact_movetext(pgn: &str) -> String {
    let movetext = match pgn.split_once("\n\n") {
        Some((_, rest)) => rest.trim(),
        None => pgn.trim(),
    };
    let no_comments = strip_brace_comments(movetext);
    strip_black_move_markers(&no_comments)
}

/// Drop `12...` markers (chess.com style); keep `12.` for white.
fn strip_black_move_markers(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i + 2 < bytes.len() && &bytes[i..i + 3] == b"..." {
                i += 3;
                if i < bytes.len() && bytes[i] == b' ' {
                    i += 1;
                }
                continue;
            }
            out.push_str(&s[start..i]);
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn strip_brace_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0i32;
    for ch in s.chars() {
        match ch {
            '{' => depth += 1,
            '}' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    let mut compact = String::with_capacity(out.len());
    let mut prev_space = false;
    for ch in out.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                compact.push(' ');
                prev_space = true;
            }
        } else {
            compact.push(ch);
            prev_space = false;
        }
    }
    compact.trim().to_string()
}

/// Hand a prompt to Omarchy's default agent (`omarchy-agent --prompt`).
pub fn launch_default_agent(prompt: &str) {
    match std::process::Command::new("omarchy-agent")
        .arg("--prompt")
        .arg(prompt)
        .spawn()
    {
        Ok(_) => eprintln!("reprise: handed prompt to omarchy-agent"),
        Err(err) => eprintln!(
            "reprise: failed to launch omarchy-agent ({err}). \
             Set a default with: omarchy default agent <name>"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_clock_comments_from_movetext() {
        let pgn = "[Event \"x\"]\n\n1. e4 {[%clk 0:10:00]} 1... e5 {[%clk 0:09:59]} 2. Nf3 *";
        let mt = compact_movetext(pgn);
        assert!(!mt.contains("clk"));
        assert!(!mt.contains("..."));
        assert!(mt.contains("1. e4"));
        assert!(mt.contains("e5"));
        assert!(mt.contains("2. Nf3"));
    }

    #[test]
    fn week_bounds_are_monday_through_sunday() {
        let (start, end, label) = current_week_bounds();
        assert!(end > start);
        assert!(label.contains('–') || label.contains('-'));
        let start_local = start.with_timezone(&Local);
        assert_eq!(start_local.weekday().num_days_from_monday(), 0);
    }

    #[test]
    fn ordinals_and_duplicate_refs() {
        assert_eq!(ordinal(1), "1st");
        assert_eq!(ordinal(2), "2nd");
        assert_eq!(ordinal(3), "3rd");
        assert_eq!(ordinal(11), "11th");
        assert_eq!(ordinal(21), "21st");
    }

    #[test]
    fn review_relpath_uses_monday() {
        assert_eq!(
            review_relpath_for_week("2026-09-14 – 2026-09-20"),
            "chess/2026-09-14.md"
        );
    }

    #[test]
    fn pack_against_local_db_if_present() {
        use crate::data::load_library;
        let path = crate::data::default_db_path();
        if !path.is_file() {
            return;
        }
        let lib = load_library(&path).expect("load library");
        let pack = build_pack(&lib).expect("build pack");
        eprintln!(
            "week {} · {} games · {} analyzed · prompt {} chars · {}",
            pack.week_label,
            pack.game_count,
            pack.analyzed_count,
            pack.prompt.len(),
            pack.review_relpath
        );
        assert!(pack.prompt.contains("2 patterns common to both colors"));
        assert!(pack.prompt.contains("specific to when I play White"));
        assert!(pack.prompt.contains("specific to when I play Black"));
        assert!(pack.prompt.contains("When citing a game"));
        assert!(pack.prompt.contains("always include the ply number"));
        assert!(pack.prompt.contains(&pack.review_relpath));
        assert!(pack.review_relpath.starts_with("chess/"));
        assert!(!pack.prompt.contains("/home/"));
        if pack.game_count > 0 {
            assert!(!pack.prompt.contains("G1 "));
            assert!(pack.prompt.contains("moves:"));
        }
        for line in pack.prompt.lines().take(14) {
            eprintln!("{line}");
        }
    }
}
