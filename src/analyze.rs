//! In-process game analysis (Stockfish pool). No separate analysis-server needed.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;

use anyhow::{anyhow, Context, Result};
use rschess::{Board, Fen, Move};
use serde_json::{json, Value};

use crate::chess_util::pgn_sans;
use crate::data::{load_library, save_index};
use crate::engine::{
    centipawn_loss, classify_loss, default_pool_config, ensure_stockfish, format_score,
    resolve_stockfish_path, score_to_white_pawns, EnginePool, InfoLine, PoolHandle, Score,
    ScoreKind, Stockfish,
};

#[derive(Debug, Clone)]
pub struct AnalyzeReport {
    pub ok: bool,
    pub message: String,
    pub analyzed: usize,
    pub failed: usize,
}

struct PlyJob {
    ply: u32,
    before_fen: String,
    after_fen: String,
    side_to_move: String,
    after_side: String,
    san: String,
    uci: String,
}

static POOL: OnceLock<Mutex<Option<EnginePool>>> = OnceLock::new();

fn pool_handle() -> Result<PoolHandle> {
    let path = ensure_stockfish()?;
    let slot = POOL.get_or_init(|| Mutex::new(None));
    let mut guard = slot
        .lock()
        .map_err(|_| anyhow!("engine pool lock poisoned"))?;
    if guard.is_none() {
        let cfg = default_pool_config();
        eprintln!(
            "reprise: starting Stockfish pool ({} workers × {} threads) from {}",
            cfg.workers,
            cfg.threads_per_worker,
            path.display()
        );
        *guard = Some(EnginePool::start(&path, &cfg)?);
    }
    Ok(guard.as_ref().unwrap().handle())
}

pub fn analyze_and_save(
    db_path: &Path,
    id: &str,
    game_type: &str,
    pgn: &str,
    depth: u32,
    multipv: u32,
    on_progress: impl FnMut(usize, usize, &str),
) -> Result<AnalyzeReport> {
    let analysis = analyze_pgn(pgn, depth, multipv, on_progress)?;
    save_analysis_blob(db_path, id, game_type, analysis)?;
    Ok(AnalyzeReport {
        ok: true,
        message: format!("Analyzed {id}"),
        analyzed: 1,
        failed: 0,
    })
}

pub fn analyze_many_and_save(
    db_path: &Path,
    jobs: &[(String, String, String)],
    depth: u32,
    multipv: u32,
    mut on_progress: impl FnMut(usize, usize, &str),
) -> Result<AnalyzeReport> {
    if jobs.is_empty() {
        return Ok(AnalyzeReport {
            ok: true,
            message: "Nothing to analyze".into(),
            analyzed: 0,
            failed: 0,
        });
    }
    let total = jobs.len();
    // Warm pool once (downloads Stockfish on first need).
    if resolve_stockfish_path().is_none() {
        on_progress(0, total, "Downloading Stockfish…");
    }
    let _ = pool_handle()?;
    let mut analyzed = 0usize;
    let mut failed = 0usize;
    for (i, (id, game_type, pgn)) in jobs.iter().enumerate() {
        on_progress(i + 1, total, &format!("Analyzing {id}"));
        match analyze_and_save(db_path, id, game_type, pgn, depth, multipv, |_, _, _| {}) {
            Ok(_) => analyzed += 1,
            Err(err) => {
                failed += 1;
                on_progress(i + 1, total, &format!("Failed {id}: {err}"));
            }
        }
    }
    Ok(AnalyzeReport {
        ok: failed == 0,
        message: format!("Analyzed {analyzed} · failed {failed} · of {total}"),
        analyzed,
        failed,
    })
}

fn analyze_pgn(
    pgn: &str,
    depth: u32,
    multipv: u32,
    mut on_progress: impl FnMut(usize, usize, &str),
) -> Result<Value> {
    let jobs = build_ply_jobs(pgn)?;
    let total = jobs.len();
    if total == 0 {
        return Err(anyhow!("PGN has no moves to analyze"));
    }

    if resolve_stockfish_path().is_none() {
        on_progress(0, total, "Downloading Stockfish…");
    }
    let handle = pool_handle()?;
    on_progress(0, total, "analyzing…");
    let (tx, rx) = std::sync::mpsc::channel::<(usize, Result<Value>)>();
    let done = AtomicUsize::new(0);

    for (index, job) in jobs.into_iter().enumerate() {
        let handle = handle.clone();
        let tx = tx.clone();
        thread::spawn(move || {
            let outcome = handle.run(move |eng| analyze_one_ply(eng, &job, depth, multipv));
            let _ = tx.send((index, outcome));
        });
    }
    drop(tx);

    let mut results = vec![None; total];
    for (index, outcome) in rx {
        results[index] = Some(outcome?);
        let n = done.fetch_add(1, Ordering::Relaxed) + 1;
        on_progress(n, total, "ply");
    }

    let moves: Vec<Value> = results
        .into_iter()
        .map(|r| r.ok_or_else(|| anyhow!("missing ply result")))
        .collect::<Result<_>>()?;

    Ok(json!({
        "engine": "Stockfish (reprise native)",
        "depth": depth,
        "multipv": multipv,
        "analyzedAt": chrono::Utc::now().to_rfc3339(),
        "moveCount": moves.len(),
        "moves": moves,
    }))
}

fn analyze_one_ply(
    eng: &mut Stockfish,
    job: &PlyJob,
    depth: u32,
    multipv: u32,
) -> Result<Value> {
    let lines = eng.analyze(&job.before_fen, depth, multipv)?.lines;
    let best = lines.first().cloned();
    let mut played_info = lines
        .iter()
        .find(|l| l.pv.first().map(|u| u.as_str()) == Some(job.uci.as_str()))
        .cloned();
    if played_info.is_none() {
        played_info = eng.score_move(&job.before_fen, &job.uci, depth)?;
    }

    let best_score = best.as_ref().map(|l| l.score.clone());
    let played_score = played_info.as_ref().map(|l| l.score.clone());
    let loss = centipawn_loss(best_score.as_ref(), played_score.as_ref());
    let classification = classify_loss(loss, best_score.as_ref(), played_score.as_ref());

    let played_continuation: Option<InfoLine> =
        if classification != "best" && classification != "excellent" {
            eng.analyze(&job.after_fen, depth, 1)?
                .lines
                .into_iter()
                .next()
        } else if let Some(info) = &played_info {
            if info.pv.len() > 1 {
                Some(InfoLine {
                    depth: info.depth,
                    multipv: 1,
                    score: info.score.clone(),
                    pv: info.pv[1..].to_vec(),
                })
            } else {
                None
            }
        } else {
            None
        };

    let best_moves: Vec<Value> = lines
        .iter()
        .take(multipv as usize)
        .map(|line| {
            let san_line = uci_pv_to_san(&job.before_fen, &line.pv);
            json!({
                "uci": line.pv.first(),
                "san": san_line.first(),
                "eval": score_json(Some(&line.score)),
                "evalText": format_score(Some(&line.score), &job.side_to_move),
                "evalWhite": score_to_white_pawns(&line.score, &job.side_to_move),
                "pvUci": &line.pv,
                "pvSan": san_line,
                "depth": line.depth,
            })
        })
        .collect();

    let played_pv = played_info
        .as_ref()
        .map(|i| i.pv.clone())
        .unwrap_or_else(|| vec![job.uci.clone()]);
    let played_pv_san = uci_pv_to_san(&job.before_fen, &played_pv);
    let after_pv = played_continuation
        .as_ref()
        .map(|i| i.pv.clone())
        .unwrap_or_default();
    let after_pv_san = uci_pv_to_san(&job.after_fen, &after_pv);
    let teaching = build_teaching(
        &job.san,
        classification,
        loss,
        &job.side_to_move,
        &best_moves,
        &after_pv_san,
        best_score.as_ref(),
        played_score.as_ref(),
    );

    Ok(json!({
        "ply": job.ply,
        "san": job.san,
        "uci": job.uci,
        "color": job.side_to_move,
        "fenBefore": job.before_fen,
        "fenAfter": job.after_fen,
        "classification": classification,
        "lossCp": loss,
        "evalBeforeBest": score_json(best_score.as_ref()),
        "evalPlayed": score_json(played_score.as_ref()),
        "evalBeforeBestText": format_score(best_score.as_ref(), &job.side_to_move),
        "evalPlayedText": format_score(played_score.as_ref(), &job.side_to_move),
        "evalWhiteBest": best_score.as_ref().map(|s| score_to_white_pawns(s, &job.side_to_move)),
        "evalWhitePlayed": played_score.as_ref().map(|s| score_to_white_pawns(s, &job.side_to_move)),
        "bestMoves": best_moves,
        "playedLine": {
            "pvSan": played_pv_san,
            "eval": score_json(played_score.as_ref()),
            "evalText": format_score(played_score.as_ref(), &job.side_to_move),
        },
        "idealAfterPlayed": {
            "pvSan": after_pv_san,
            "eval": score_json(played_continuation.as_ref().map(|i| &i.score)),
            "evalText": format_score(
                played_continuation.as_ref().map(|i| &i.score),
                &job.after_side,
            ),
        },
        "teaching": teaching,
    }))
}

fn score_json(score: Option<&Score>) -> Value {
    match score {
        Some(s) => json!({
            "type": match s.kind {
                ScoreKind::Cp => "cp",
                ScoreKind::Mate => "mate",
            },
            "value": s.value,
        }),
        None => Value::Null,
    }
}

fn build_teaching(
    played_san: &str,
    classification: &str,
    loss: i32,
    side_to_move: &str,
    best_moves: &[Value],
    after_pv_san: &[String],
    best_score: Option<&Score>,
    played_score: Option<&Score>,
) -> Value {
    let side = if side_to_move == "w" { "White" } else { "Black" };
    let is_best = classification == "best" || classification == "excellent";
    let loss_pawns = format!("{:.1}", loss as f64 / 100.0);
    let best_san = best_moves
        .first()
        .and_then(|m| m.get("san"))
        .and_then(|s| s.as_str())
        .unwrap_or("?");
    let best_eval = best_moves
        .first()
        .and_then(|m| m.get("evalText"))
        .and_then(|s| s.as_str())
        .unwrap_or("—");

    if is_best {
        let summary = if classification == "best" {
            format!("{played_san} is the best move.")
        } else {
            format!("{played_san} is an excellent move.")
        };
        return json!({
            "summary": summary,
            "whyPlayed": format!(
                "{side} played {played_san}. Engine evaluation: {}.",
                format_score(played_score, side_to_move)
            ),
            "whyBest": "No meaningfully better alternative.",
            "idealFuture": "",
            "classification": classification,
        });
    }

    let label = match classification {
        "blunder" => "a blunder",
        "mistake" => "a mistake",
        "inaccuracy" => "an inaccuracy",
        _ => "a weaker move",
    };
    let mut why_played = format!(
        "{side} played {played_san}. Evaluation dropped from {} (best) to {} after this choice.",
        format_score(best_score, side_to_move),
        format_score(played_score, side_to_move)
    );
    if !after_pv_san.is_empty() {
        let line: Vec<_> = after_pv_san.iter().take(10).cloned().collect();
        why_played.push_str(&format!(" Against accurate replies: {}.", line.join(" ")));
    }

    json!({
        "summary": format!("{played_san} is {label}, costing about {loss_pawns} pawns of evaluation."),
        "whyPlayed": why_played,
        "whyBest": format!("{best_san} was better, evaluating to {best_eval}."),
        "idealFuture": if after_pv_san.is_empty() {
            String::new()
        } else {
            let line: Vec<_> = after_pv_san.iter().take(10).cloned().collect();
            format!(
                "If {} had played {best_san}, ideal play could continue: {}.",
                side.to_lowercase(),
                line.join(" ")
            )
        },
        "classification": classification,
    })
}

fn build_ply_jobs(pgn: &str) -> Result<Vec<PlyJob>> {
    let mut board = Board::default();
    if let Some(fen) = pgn_fen_header(pgn) {
        let fen: Fen = fen
            .as_str()
            .try_into()
            .map_err(|_| anyhow!("invalid FEN header"))?;
        board = Board::from_fen(fen);
    }

    let mut jobs = Vec::new();
    for (i, san) in pgn_sans(pgn).into_iter().enumerate() {
        let before_fen = board.to_fen().to_string();
        let side_to_move = if board.side_to_move().is_white() {
            "w"
        } else {
            "b"
        }
        .to_string();
        let mv = board
            .san_to_move(&san)
            .map_err(|_| anyhow!("illegal SAN at ply {}: {san}", i + 1))?;
        let uci = mv.to_uci();
        board
            .make_move(mv)
            .map_err(|_| anyhow!("cannot play {san}"))?;
        let after_fen = board.to_fen().to_string();
        let after_side = if board.side_to_move().is_white() {
            "w"
        } else {
            "b"
        }
        .to_string();
        jobs.push(PlyJob {
            ply: (i + 1) as u32,
            before_fen,
            after_fen,
            side_to_move,
            after_side,
            san,
            uci,
        });
    }
    Ok(jobs)
}

fn pgn_fen_header(pgn: &str) -> Option<String> {
    let mut setup = false;
    let mut fen = None;
    for line in pgn.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("[SetUp \"") {
            setup = rest.starts_with('1');
        } else if let Some(rest) = line.strip_prefix("[FEN \"") {
            fen = rest.strip_suffix("\"]").map(str::to_string);
        } else if line.is_empty() {
            break;
        }
    }
    if setup {
        fen
    } else {
        None
    }
}

fn uci_pv_to_san(fen: &str, pv: &[String]) -> Vec<String> {
    let Ok(fen) = Fen::try_from(fen) else {
        return Vec::new();
    };
    let mut board = Board::from_fen(fen);
    let mut sans = Vec::new();
    for uci in pv {
        let Ok(mv) = Move::from_uci(uci) else {
            break;
        };
        let Ok(san) = board.move_to_san(mv) else {
            break;
        };
        if board.make_move(mv).is_err() {
            break;
        }
        sans.push(san);
    }
    sans
}

pub fn save_analysis_blob(
    db_path: &Path,
    id: &str,
    game_type: &str,
    analysis: Value,
) -> Result<()> {
    let raw = std::fs::read_to_string(db_path)
        .with_context(|| format!("reading {}", db_path.display()))?;
    let mut root: Value = serde_json::from_str(&raw).context("parse db.json")?;
    let games = root
        .get_mut("games")
        .and_then(|g| g.as_array_mut())
        .context("db.json missing games")?;
    let mut found = false;
    for g in games.iter_mut() {
        let gid = g.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let gt = g.get("gameType").and_then(|v| v.as_str()).unwrap_or("");
        if gid == id && gt == game_type {
            let obj = g.as_object_mut().context("game not object")?;
            obj.insert("analysis".into(), analysis);
            obj.insert(
                "updatedAt".into(),
                json!(chrono::Utc::now().to_rfc3339()),
            );
            found = true;
            break;
        }
    }
    if !found {
        return Err(anyhow!("game {id}/{game_type} not found in db"));
    }
    root["updatedAt"] = json!(chrono::Utc::now().to_rfc3339());
    let out = serde_json::to_string_pretty(&root).context("serialize db")?;
    let tmp = db_path.with_extension("json.tmp");
    std::fs::write(&tmp, out)?;
    std::fs::rename(&tmp, db_path)?;

    if let Ok(mut lib) = load_library(db_path) {
        if let Some(g) = lib
            .games
            .iter_mut()
            .find(|g| g.id == id && g.game_type == game_type)
        {
            g.has_analysis = true;
            g.analysis = None;
        }
        save_index(&lib)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn smoke_analyze_mini_pgn() {
        let pgn = r#"[Event "Test"]
[White "A"]
[Black "B"]
[Result "*"]

1. e4 e5 2. Nf3
"#;
        let t0 = Instant::now();
        let v = analyze_pgn(pgn, 6, 1, |c, t, _| eprintln!("ply {c}/{t}")).expect("analyze");
        eprintln!("done in {:.2?} moves={}", t0.elapsed(), v["moveCount"]);
        assert_eq!(v["moveCount"].as_u64().unwrap(), 3);
    }
}
