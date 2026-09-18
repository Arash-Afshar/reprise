//! Chess.com published-data sync (import only — never analyzes).

use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::chess_util::pgn_tail;
use crate::data::{append_games_to_db, ensure_library_files, save_index, Game, Library, Settings};

/// How many newest games to surface before the rest finish importing.
pub const EARLY_SYNC_BATCH: usize = 5;

#[derive(Debug, Clone)]
pub struct SyncReport {
    pub imported: usize,
    pub skipped: usize,
    pub fetched: usize,
    pub username: String,
    pub elapsed: Duration,
    pub offline: bool,
    pub message: String,
}

#[derive(Debug, Deserialize)]
struct ArchivesList {
    #[serde(default)]
    archives: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct MonthGames {
    #[serde(default)]
    games: Vec<Value>,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into()
}

/// Cheap connectivity probe against chess.com pub API.
pub fn network_available() -> bool {
    let agent = agent();
    agent
        .get("https://api.chess.com/pub/player/hikaru")
        .call()
        .map(|r| r.status().is_success() || r.status().as_u16() == 404)
        .unwrap_or(false)
}

/// Import new games newest-first. Calls `on_early` once after the first
/// [`EARLY_SYNC_BATCH`] new games are persisted so the UI can open quickly;
/// remaining months continue in the same call until finished.
///
/// Walks chess.com monthly archives from newest → oldest, importing gaps.
/// Stops once a month is fully already present locally (caught up), or when
/// the archive list is exhausted (beginning of history).
pub fn sync_new_games<F>(library: &mut Library, mut on_early: F) -> Result<SyncReport>
where
    F: FnMut(&Library),
{
    let t0 = Instant::now();
    let username = library
        .settings
        .my_username
        .clone()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow!("No chess.com username in library settings"))?;

    if !network_available() {
        return Ok(SyncReport {
            imported: 0,
            skipped: 0,
            fetched: 0,
            username,
            elapsed: t0.elapsed(),
            offline: true,
            message: "Offline — skipped sync".into(),
        });
    }

    ensure_library_files(library)?;

    let agent = agent();
    let archives_url = format!(
        "https://api.chess.com/pub/player/{}/games/archives",
        username.to_lowercase()
    );
    let list: ArchivesList = agent
        .get(&archives_url)
        .call()
        .with_context(|| format!("listing archives for {username}"))?
        .body_mut()
        .read_json()
        .context("parse archives list")?;

    // Newest months first: tip check → gap fill → stop when caught up.
    let mut month_urls = list.archives;
    month_urls.sort();
    month_urls.reverse();

    let mut existing: HashSet<(String, String)> = library
        .games
        .iter()
        .map(|g| (g.id.clone(), g.game_type.clone()))
        .collect();

    let mut imported = 0;
    let mut skipped = 0;
    let mut fetched = 0;
    let mut months_fetched = 0usize;
    let mut batch: Vec<Game> = Vec::new();
    let mut early_sent = false;

    let flush = |library: &mut Library, batch: &mut Vec<Game>| -> Result<()> {
        if batch.is_empty() {
            return Ok(());
        }
        if library.path.is_file() {
            append_games_to_db(&library.path, batch)?;
        }
        save_index(library)?;
        batch.clear();
        Ok(())
    };

    for url in &month_urls {
        let month: MonthGames = match agent.get(url).call() {
            Ok(mut res) => match res.body_mut().read_json() {
                Ok(m) => m,
                Err(_) => continue,
            },
            Err(_) => continue,
        };
        months_fetched += 1;

        let mut month_new = 0usize;
        let mut month_known = 0usize;

        // Month payloads are usually oldest→newest; reverse for newest-first.
        for api_game in month.games.into_iter().rev() {
            fetched += 1;
            let Some(record) = record_from_pub_api(&api_game, &username) else {
                continue;
            };
            let key = (record.id.clone(), record.game_type.clone());
            if existing.contains(&key) {
                skipped += 1;
                month_known += 1;
                continue;
            }
            existing.insert(key);
            batch.push(record.clone());
            library.games.push(record);
            imported += 1;
            month_new += 1;

            if !early_sent && imported >= EARLY_SYNC_BATCH {
                library
                    .games
                    .sort_by(|a, b| b.played_at.cmp(&a.played_at));
                flush(library, &mut batch)?;
                on_early(library);
                early_sent = true;
            }
        }

        // Fully known month with at least one game ⇒ contiguous with local
        // history from here back; no need to download older archives.
        if month_new == 0 && month_known > 0 {
            eprintln!(
                "reprise: sync caught up after {months_fetched} month(s) · imported {imported} · skipped {skipped}"
            );
            break;
        }
    }

    library
        .games
        .sort_by(|a, b| b.played_at.cmp(&a.played_at));
    flush(library, &mut batch)?;

    let message = if imported > 0 {
        format!("Synced {imported} new · {skipped} known · {fetched} scanned")
    } else {
        format!("Up to date · {skipped} known · {fetched} scanned")
    };

    Ok(SyncReport {
        imported,
        skipped,
        fetched,
        username,
        elapsed: t0.elapsed(),
        offline: false,
        message,
    })
}

fn record_from_pub_api(api_game: &Value, my_username: &str) -> Option<Game> {
    let url = api_game.get("url")?.as_str()?;
    let pgn = api_game.get("pgn")?.as_str()?.to_string();
    if pgn.is_empty() {
        return None;
    }
    let (game_type, id) = parse_game_url(url)?;
    let headers = parse_pgn_headers(&pgn);
    let white = headers
        .get("White")
        .cloned()
        .or_else(|| {
            api_game
                .pointer("/white/username")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "White".into());
    let black = headers
        .get("Black")
        .cloned()
        .or_else(|| {
            api_game
                .pointer("/black/username")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "Black".into());

    let me = my_username.to_lowercase();
    let (user_color, opponent) = if white.to_lowercase() == me {
        (Some("white".into()), Some(black.clone()))
    } else if black.to_lowercase() == me {
        (Some("black".into()), Some(white.clone()))
    } else {
        (None, None)
    };

    let end = api_game
        .get("end_time")
        .and_then(|v| v.as_i64())
        .map(|s| DateTime::from_timestamp(s, 0).unwrap_or_else(Utc::now))
        .unwrap_or_else(Utc::now);

    let (end_fen, end_ply, end_uci) = match pgn_tail(&pgn) {
        Some(tail) => (Some(tail.fen), Some(tail.ply as u32), tail.last_uci),
        None => (None, None, None),
    };

    Some(Game {
        id,
        game_type,
        url: Some(url.to_string()),
        white,
        black,
        white_elo: headers
            .get("WhiteElo")
            .and_then(|s| s.parse().ok())
            .or_else(|| {
                api_game
                    .pointer("/white/rating")
                    .and_then(|v| v.as_i64())
                    .map(|n| n as i32)
            }),
        black_elo: headers
            .get("BlackElo")
            .and_then(|s| s.parse().ok())
            .or_else(|| {
                api_game
                    .pointer("/black/rating")
                    .and_then(|v| v.as_i64())
                    .map(|n| n as i32)
            }),
        result: headers.get("Result").cloned(),
        time_control: headers.get("TimeControl").cloned().or_else(|| {
            api_game
                .get("time_control")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        }),
        eco: headers.get("ECO").cloned(),
        termination: headers.get("Termination").cloned(),
        played_at: Some(end),
        user_color,
        opponent,
        my_username: Some(my_username.to_string()),
        pgn,
        end_fen,
        end_ply,
        end_uci,
        has_analysis: false,
        reviewed: false,
        analysis: None, // sync never analyzes
    })
}

fn parse_game_url(url: &str) -> Option<(String, String)> {
    regex_lite_game_url(url)
}

fn regex_lite_game_url(url: &str) -> Option<(String, String)> {
    // /game/live/123 or /game/daily/123 or /game/123
    if let Some(caps) = find_two(url, "/game/") {
        return Some(caps);
    }
    None
}

fn find_two(url: &str, marker: &str) -> Option<(String, String)> {
    let idx = url.find(marker)?;
    let rest = &url[idx + marker.len()..];
    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        [ty, id, ..] if *ty == "live" || *ty == "daily" => {
            let id = id
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>();
            if id.is_empty() {
                None
            } else {
                Some(((*ty).to_string(), id))
            }
        }
        [id, ..] => {
            let id = id
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>();
            if id.is_empty() {
                None
            } else {
                Some(("live".into(), id))
            }
        }
        _ => None,
    }
}

fn parse_pgn_headers(pgn: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for line in pgn.lines() {
        let line = line.trim();
        if !line.starts_with('[') {
            if line.is_empty() {
                continue;
            }
            // stop after header block
            if !line.starts_with('[') && !map.is_empty() && !line.contains('"') {
                break;
            }
        }
        if let Some((k, v)) = parse_header_line(line) {
            map.insert(k, v);
        }
    }
    map
}

fn parse_header_line(line: &str) -> Option<(String, String)> {
    let line = line.strip_prefix('[')?.strip_suffix(']')?;
    let mut parts = line.splitn(2, ' ');
    let key = parts.next()?.trim().to_string();
    let val = parts.next()?.trim();
    let val = val.strip_prefix('"')?.strip_suffix('"')?.to_string();
    Some((key, val))
}

pub fn settings_username(settings: &Settings) -> Option<&str> {
    settings.my_username.as_deref().filter(|s| !s.is_empty())
}
