use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use serde::de::{DeserializeOwned, Deserializer, IgnoredAny};
use serde::{Deserialize, Serialize};

use super::models::{Game, LibraryFile, Settings};

#[derive(Debug, Clone)]
pub struct Library {
    pub path: PathBuf,
    pub games: Vec<Game>,
    pub settings: Settings,
}

pub fn default_db_path() -> PathBuf {
    if let Ok(p) = std::env::var("REPRISE_DB") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/db.json")
}

pub fn index_path_for(db_path: &Path) -> PathBuf {
    let parent = db_path.parent().unwrap_or_else(|| Path::new("."));
    parent.join("db-index.json")
}

pub fn empty_library(path: PathBuf) -> Library {
    Library {
        path,
        games: Vec::new(),
        settings: Settings::default(),
    }
}

/// Prefer slim `db-index.json` (~1–3ms). Fall back to skipping analysis in full db.
pub fn load_library(path: impl AsRef<Path>) -> Result<Library> {
    let path = path.as_ref().to_path_buf();
    let index = index_path_for(&path);

    if index.is_file() {
        let t0 = Instant::now();
        match load_from_path::<LibraryFile>(&index) {
            Ok(mut file) => {
                for g in &mut file.games {
                    if g.analysis.as_ref().is_some_and(|a| !a.moves.is_empty()) {
                        g.has_analysis = true;
                    }
                    g.analysis = None;
                }
                eprintln!(
                    "reprise: index load {} games in {:.2?} ({})",
                    file.games.len(),
                    t0.elapsed(),
                    index.display()
                );
                return Ok(library_from_file(path, file));
            }
            Err(err) => {
                eprintln!(
                    "reprise: bad index {} ({err:#}) — falling back to db.json lite",
                    index.display()
                );
            }
        }
    }

    load_library_lite(&path)
}

/// Parse full db.json while discarding analysis object bodies (no move-tree heap).
pub fn load_library_lite(path: &Path) -> Result<Library> {
    let t0 = Instant::now();
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading library at {}", path.display()))?;
    let t_read = t0.elapsed();

    let t1 = Instant::now();
    let file: LibraryFileLite =
        serde_json::from_str(&raw).with_context(|| format!("parsing lite {}", path.display()))?;
    eprintln!(
        "reprise: lite load read {:.2?} parse {:.2?} ({} bytes, {} games)",
        t_read,
        t1.elapsed(),
        raw.len(),
        file.games.len()
    );

    let games = file
        .games
        .into_iter()
        .map(|g| Game {
            id: g.id,
            game_type: g.game_type,
            url: g.url,
            white: g.white,
            black: g.black,
            white_elo: g.white_elo,
            black_elo: g.black_elo,
            result: g.result,
            time_control: g.time_control,
            eco: g.eco,
            termination: g.termination,
            played_at: g.played_at,
            user_color: g.user_color,
            opponent: g.opponent,
            my_username: g.my_username,
            pgn: g.pgn,
            end_fen: g.end_fen,
            end_ply: g.end_ply,
            end_uci: g.end_uci,
            has_analysis: g.has_analysis || g.analysis_present,
            reviewed: g.reviewed,
            analysis: None,
        })
        .collect();

    Ok(Library {
        path: path.to_path_buf(),
        games: sorted_games(games),
        settings: file.settings,
    })
}

fn load_from_path<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

fn library_from_file(path: PathBuf, file: LibraryFile) -> Library {
    Library {
        path,
        games: sorted_games(file.games),
        settings: file.settings,
    }
}

fn sorted_games(mut games: Vec<Game>) -> Vec<Game> {
    games.sort_by(|a, b| b.played_at.cmp(&a.played_at));
    games
}

/// Persist slim index only — never rewrite the analysis db from the UI process.
pub fn save_index(library: &Library) -> Result<()> {
    let t0 = Instant::now();
    let index = index_path_for(&library.path);
    let file = LibraryFile {
        version: 1,
        games: library
            .games
            .iter()
            .map(|g| {
                let mut row = g.clone();
                row.analysis = None;
                row
            })
            .collect(),
        settings: library.settings.clone(),
        updated_at: Some(chrono::Utc::now().to_rfc3339()),
    };
    let raw = serde_json::to_string(&file).context("serialize index")?;
    let tmp = index.with_extension("json.tmp");
    std::fs::write(&tmp, raw).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &index).with_context(|| format!("renaming into {}", index.display()))?;
    eprintln!("reprise: save_index {:.2?}", t0.elapsed());
    Ok(())
}

/// Patch a single `reviewed` flag in `db-index.json` (background-safe).
pub fn persist_reviewed_flag(db_path: &Path, id: &str, game_type: &str) -> Result<()> {
    let t0 = Instant::now();
    let index = index_path_for(db_path);
    if !index.is_file() {
        return Ok(());
    }
    let raw = std::fs::read_to_string(&index)
        .with_context(|| format!("reading {}", index.display()))?;
    let mut root: serde_json::Value =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", index.display()))?;
    let Some(games) = root.get_mut("games").and_then(|g| g.as_array_mut()) else {
        return Ok(());
    };
    let mut found = false;
    for g in games {
        let gid = g.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let gt = g.get("gameType").and_then(|v| v.as_str()).unwrap_or("");
        if gid == id && gt == game_type {
            g["reviewed"] = serde_json::Value::Bool(true);
            found = true;
            break;
        }
    }
    if !found {
        return Ok(());
    }
    root["updatedAt"] = serde_json::Value::String(chrono::Utc::now().to_rfc3339());
    let out = serde_json::to_string(&root).context("serialize index patch")?;
    let tmp = index.with_extension("json.tmp");
    std::fs::write(&tmp, out).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &index).with_context(|| format!("renaming into {}", index.display()))?;
    eprintln!(
        "reprise: persist_reviewed_flag {id} {:.2?}",
        t0.elapsed()
    );
    Ok(())
}

/// Fire-and-forget reviewed flag persistence for the UI thread.
pub fn persist_reviewed_flag_async(db_path: PathBuf, id: String, game_type: String) {
    std::thread::spawn(move || {
        if let Err(err) = persist_reviewed_flag(&db_path, &id, &game_type) {
            eprintln!("reprise: persist reviewed failed: {err:#}");
        }
    });
}

/// Create an empty `db.json` (+ index) when missing, or refresh settings on disk.
pub fn ensure_library_files(library: &Library) -> Result<()> {
    if let Some(parent) = library.path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    if library.path.is_file() {
        // Keep games; just refresh settings + updatedAt via Value merge.
        let raw = std::fs::read_to_string(&library.path)
            .with_context(|| format!("reading {}", library.path.display()))?;
        let mut root: serde_json::Value = serde_json::from_str(&raw)
            .with_context(|| format!("parse {}", library.path.display()))?;
        root["settings"] =
            serde_json::to_value(&library.settings).context("serialize settings")?;
        root["updatedAt"] = serde_json::Value::String(chrono::Utc::now().to_rfc3339());
        let out = serde_json::to_string_pretty(&root).context("serialize db")?;
        let tmp = library.path.with_extension("json.tmp");
        std::fs::write(&tmp, out).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &library.path)?;
    } else {
        let file = LibraryFile {
            version: 1,
            games: Vec::new(),
            settings: library.settings.clone(),
            updated_at: Some(chrono::Utc::now().to_rfc3339()),
        };
        let out = serde_json::to_string_pretty(&file).context("serialize empty db")?;
        let tmp = library.path.with_extension("json.tmp");
        std::fs::write(&tmp, out).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &library.path)?;
    }
    save_index(library)?;
    Ok(())
}

/// Upsert new games into full db.json via Value merge (avoids typed analysis parse).
pub fn append_games_to_db(db_path: &Path, new_games: &[Game]) -> Result<()> {
    if new_games.is_empty() {
        return Ok(());
    }
    let raw = std::fs::read_to_string(db_path)
        .with_context(|| format!("reading {}", db_path.display()))?;
    let mut root: serde_json::Value =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", db_path.display()))?;
    let games = root
        .get_mut("games")
        .and_then(|g| g.as_array_mut())
        .context("db.json missing games array")?;

    let existing: std::collections::HashSet<(String, String)> = games
        .iter()
        .filter_map(|g| {
            Some((
                g.get("id")?.as_str()?.to_string(),
                g.get("gameType")?.as_str()?.to_string(),
            ))
        })
        .collect();

    for g in new_games {
        let key = (g.id.clone(), g.game_type.clone());
        if existing.contains(&key) {
            continue;
        }
        let mut v = serde_json::to_value(g).context("serialize game")?;
        if let Some(obj) = v.as_object_mut() {
            obj.remove("hasAnalysis");
            obj.remove("analysis");
        }
        games.push(v);
    }

    root["updatedAt"] = serde_json::Value::String(chrono::Utc::now().to_rfc3339());
    let out = serde_json::to_string_pretty(&root).context("serialize db")?;
    let tmp = db_path.with_extension("json.tmp");
    std::fs::write(&tmp, out).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, db_path)?;
    Ok(())
}

/// Load a single game's analysis blob without attaching every game's trees.
pub fn load_game_analysis(
    db_path: &Path,
    id: &str,
    game_type: &str,
) -> Result<Option<super::models::GameAnalysis>> {
    let key = (id.to_string(), game_type.to_string());
    let mut map = load_analyses_for(db_path, &[key])?;
    Ok(map.remove(&(id.to_string(), game_type.to_string())))
}

/// One disk pass: deserialize analysis only for the requested `(id, gameType)` keys.
pub fn load_analyses_for(
    db_path: &Path,
    keys: &[(String, String)],
) -> Result<std::collections::HashMap<(String, String), super::models::GameAnalysis>> {
    use std::collections::{HashMap, HashSet};

    let mut out = HashMap::new();
    if keys.is_empty() {
        return Ok(out);
    }
    let want: HashSet<(&str, &str)> = keys
        .iter()
        .map(|(id, gt)| (id.as_str(), gt.as_str()))
        .collect();

    let t0 = Instant::now();
    let raw = std::fs::read_to_string(db_path)
        .with_context(|| format!("reading {}", db_path.display()))?;
    let t_read = t0.elapsed();
    let root: serde_json::Value =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", db_path.display()))?;
    let Some(games) = root.get("games").and_then(|g| g.as_array()) else {
        return Ok(out);
    };

    for g in games {
        if out.len() == want.len() {
            break;
        }
        let gid = g.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let gt = g.get("gameType").and_then(|v| v.as_str()).unwrap_or("");
        if !want.contains(&(gid, gt)) {
            continue;
        }
        let Some(analysis_val) = g.get("analysis") else {
            continue;
        };
        match serde_json::from_value::<super::models::GameAnalysis>(analysis_val.clone()) {
            Ok(analysis) if !analysis.moves.is_empty() => {
                out.insert((gid.to_string(), gt.to_string()), analysis);
            }
            Ok(_) => {}
            Err(err) => {
                eprintln!("reprise: skip analysis {gid}: {err:#}");
            }
        }
    }
    eprintln!(
        "reprise: load analyses {}/{} (read {:.2?} total {:.2?})",
        out.len(),
        keys.len(),
        t_read,
        t0.elapsed()
    );
    Ok(out)
}

impl Library {
    pub fn still_open(&self, limit: usize) -> Vec<&Game> {
        let mut open: Vec<&Game> = self
            .games
            .iter()
            .filter(|g| g.is_analyzed())
            .collect();
        if open.len() < limit {
            for g in &self.games {
                if open.len() >= limit {
                    break;
                }
                if !g.is_analyzed() {
                    open.push(g);
                }
            }
        }
        open.into_iter().take(limit).collect()
    }

    pub fn recent(&self, limit: usize) -> Vec<&Game> {
        self.games.iter().take(limit).collect()
    }

    pub fn find(&self, id: &str, game_type: &str) -> Option<&Game> {
        self.games
            .iter()
            .find(|g| g.id == id && g.game_type == game_type)
    }

    pub fn unanalyzed_ids(&self) -> Vec<(String, String)> {
        self.games
            .iter()
            .filter(|g| !g.is_analyzed())
            .map(|g| (g.id.clone(), g.game_type.clone()))
            .collect()
    }

    /// Mark a game as reviewed in memory. Returns true if the flag changed
    /// (caller should schedule `persist_reviewed_flag_async`).
    pub fn mark_reviewed(&mut self, id: &str, game_type: &str) -> bool {
        let Some(g) = self
            .games
            .iter_mut()
            .find(|g| g.id == id && g.game_type == game_type)
        else {
            return false;
        };
        if g.reviewed {
            return false;
        }
        g.reviewed = true;
        true
    }

    /// Fill missing end-position fields from PGN (call off the UI thread).
    pub fn backfill_end_positions(&mut self) -> usize {
        use crate::chess_util::pgn_tail;
        let mut n = 0usize;
        for g in &mut self.games {
            if g.end_fen.is_some() {
                continue;
            }
            let Some(tail) = pgn_tail(&g.pgn) else {
                continue;
            };
            g.end_fen = Some(tail.fen);
            g.end_ply = Some(tail.ply as u32);
            g.end_uci = tail.last_uci;
            n += 1;
        }
        n
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LibraryFileLite {
    #[serde(default)]
    games: Vec<GameLite>,
    #[serde(default)]
    settings: Settings,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GameLite {
    id: String,
    game_type: String,
    #[serde(default)]
    url: Option<String>,
    white: String,
    black: String,
    #[serde(default)]
    white_elo: Option<i32>,
    #[serde(default)]
    black_elo: Option<i32>,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    time_control: Option<String>,
    #[serde(default)]
    eco: Option<String>,
    #[serde(default)]
    termination: Option<String>,
    #[serde(default)]
    played_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    user_color: Option<String>,
    #[serde(default)]
    opponent: Option<String>,
    #[serde(default)]
    my_username: Option<String>,
    pgn: String,
    #[serde(default)]
    end_fen: Option<String>,
    #[serde(default)]
    end_ply: Option<u32>,
    #[serde(default)]
    end_uci: Option<String>,
    #[serde(default)]
    has_analysis: bool,
    #[serde(default)]
    reviewed: bool,
    /// If the JSON has an `analysis` object, mark present and discard the body.
    #[serde(default, rename = "analysis", deserialize_with = "deserialize_analysis_present")]
    analysis_present: bool,
}

fn deserialize_analysis_present<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<IgnoredAny>::deserialize(deserializer)? {
        Some(_) => Ok(true),
        None => Ok(false),
    }
}

// Keep Serialize import used via Game in append path.
#[allow(dead_code)]
fn _assert_serialize() {
    fn assert<T: Serialize>() {}
    assert::<Settings>();
}

#[cfg(test)]
mod perf_tests {
    use super::*;
    use std::time::Duration;

    /// Frame budget at 60Hz.
    const FRAME: Duration = Duration::from_micros(16_667);

    #[test]
    fn async_open_moves_analysis_io_off_ui_shell() {
        let path = default_db_path();
        if !path.is_file() {
            eprintln!("skip: no db at {}", path.display());
            return;
        }
        let lib = load_library(&path).expect("load library");
        let g = lib
            .games
            .iter()
            .find(|g| g.has_analysis)
            .expect("analyzed game")
            .clone();
        assert!(g.analysis.is_none());
        assert!(g.has_analysis);

        // Warm caches once (matches steady-state app use after first open).
        let _ = load_game_analysis(&path, &g.id, &g.game_type).expect("warm analysis");

        let mut shell_samples = Vec::new();
        for _ in 0..50 {
            let t0 = Instant::now();
            let shell = g.clone();
            let _meta = format!(
                "{} vs {} · {}",
                shell.white,
                shell.black,
                shell.result.as_deref().unwrap_or("—")
            );
            shell_samples.push(t0.elapsed());
        }
        shell_samples.sort();
        let shell = shell_samples[shell_samples.len() / 2];

        let mut load_samples = Vec::new();
        for _ in 0..5 {
            let t0 = Instant::now();
            let loaded = load_game_analysis(&path, &g.id, &g.game_type).expect("load analysis");
            load_samples.push(t0.elapsed());
            assert!(loaded.is_some(), "expected analysis blob");
        }
        load_samples.sort();
        let load = load_samples[load_samples.len() / 2];

        eprintln!(
            "open shell median {:?} · load_game_analysis median {:?} · frame {:?}",
            shell, load, FRAME
        );

        assert!(
            shell < FRAME,
            "UI shell for async open must fit a frame (got {shell:?})"
        );
        // Analysis I/O must dominate the shell — otherwise keeping it sync would be fine.
        assert!(
            load > shell * 50,
            "analysis load should dwarf the UI shell (shell {shell:?}, load {load:?})"
        );
    }

    #[test]
    fn mark_reviewed_memory_fits_frame_vs_save_index() {
        let path = default_db_path();
        if !path.is_file() {
            eprintln!("skip: no db at {}", path.display());
            return;
        }
        let mut lib = load_library(&path).expect("load library");
        let Some(g) = lib.games.first().map(|g| (g.id.clone(), g.game_type.clone())) else {
            eprintln!("skip: empty library");
            return;
        };
        // Ensure we can flip reviewed for timing (then restore).
        let was = lib
            .games
            .iter()
            .find(|x| x.id == g.0 && x.game_type == g.1)
            .map(|x| x.reviewed)
            .unwrap_or(false);
        if let Some(row) = lib
            .games
            .iter_mut()
            .find(|x| x.id == g.0 && x.game_type == g.1)
        {
            row.reviewed = false;
        }

        let mut mem_samples = Vec::new();
        for _ in 0..100 {
            if let Some(row) = lib
                .games
                .iter_mut()
                .find(|x| x.id == g.0 && x.game_type == g.1)
            {
                row.reviewed = false;
            }
            let t0 = Instant::now();
            let _ = lib.mark_reviewed(&g.0, &g.1);
            mem_samples.push(t0.elapsed());
        }
        mem_samples.sort();
        let mem = mem_samples[mem_samples.len() / 2];

        let mut save_samples = Vec::new();
        for _ in 0..3 {
            let t0 = Instant::now();
            save_index(&lib).expect("save index");
            save_samples.push(t0.elapsed());
        }
        save_samples.sort();
        let save = save_samples[save_samples.len() / 2];

        // Restore prior reviewed flag in memory + disk patch.
        if let Some(row) = lib
            .games
            .iter_mut()
            .find(|x| x.id == g.0 && x.game_type == g.1)
        {
            row.reviewed = was;
        }
        let _ = save_index(&lib);

        eprintln!(
            "mark_reviewed memory {:?} · save_index {:?} · frame {:?}",
            mem, save, FRAME
        );
        assert!(mem < FRAME, "in-memory mark must fit a frame");
        assert!(
            save > mem * 20,
            "save_index should dominate in-memory mark (mem {mem:?}, save {save:?})"
        );
    }

    #[test]
    fn analyze_finished_patch_fits_frame_vs_reload() {
        let path = default_db_path();
        if !path.is_file() {
            eprintln!("skip: no db at {}", path.display());
            return;
        }
        let mut lib = load_library(&path).expect("load library");
        let ids: Vec<(String, String)> = lib
            .games
            .iter()
            .take(8)
            .map(|g| (g.id.clone(), g.game_type.clone()))
            .collect();
        if ids.is_empty() {
            eprintln!("skip: empty library");
            return;
        }

        let mut patch_samples = Vec::new();
        for _ in 0..100 {
            let t0 = Instant::now();
            for (id, gt) in &ids {
                if let Some(g) = lib.games.iter_mut().find(|g| g.id == *id && g.game_type == *gt)
                {
                    g.has_analysis = true;
                    g.analysis = None;
                }
            }
            patch_samples.push(t0.elapsed());
        }
        patch_samples.sort();
        let patch = patch_samples[patch_samples.len() / 2];

        let _ = load_library(&path); // warm
        let mut reload_samples = Vec::new();
        for _ in 0..5 {
            let t0 = Instant::now();
            let _ = load_library(&path).expect("reload");
            reload_samples.push(t0.elapsed());
        }
        reload_samples.sort();
        let reload = reload_samples[reload_samples.len() / 2];

        eprintln!(
            "AnalyzeFinished patch {:?} · load_library {:?} · frame {:?}",
            patch, reload, FRAME
        );
        assert!(patch < FRAME, "in-memory analyze patch must fit a frame");
        assert!(
            reload > patch * 20,
            "full reload should dominate patch (patch {patch:?}, reload {reload:?})"
        );
    }

    #[test]
    fn end_fen_preview_beats_pgn_replay() {
        let path = default_db_path();
        if !path.is_file() {
            eprintln!("skip: no db at {}", path.display());
            return;
        }
        let mut lib = load_library(&path).expect("load library");
        let Some(g) = lib.games.first_mut() else {
            eprintln!("skip: empty library");
            return;
        };
        let pgn = g.pgn.clone();
        let _ = crate::chess_util::pgn_tail(&pgn); // warm
        if g.end_fen.is_none() {
            if let Some(t) = crate::chess_util::pgn_tail(&pgn) {
                g.end_fen = Some(t.fen);
                g.end_ply = Some(t.ply as u32);
                g.end_uci = t.last_uci;
            }
        }
        let fen = g.end_fen.clone().expect("end fen");
        let ply = g.end_ply.expect("end ply");

        let mut indexed = Vec::new();
        for _ in 0..200 {
            let t0 = Instant::now();
            let _ = (fen.clone(), ply, g.end_uci.clone());
            indexed.push(t0.elapsed());
        }
        indexed.sort();
        let idx = indexed[indexed.len() / 2];

        let mut replay = Vec::new();
        for _ in 0..10 {
            let t0 = Instant::now();
            let _ = crate::chess_util::pgn_tail(&pgn);
            replay.push(t0.elapsed());
        }
        replay.sort();
        let rep = replay[replay.len() / 2];

        eprintln!(
            "list preview indexed {:?} · pgn_tail {:?} · frame {:?}",
            idx, rep, FRAME
        );
        assert!(idx < FRAME, "indexed preview must fit a frame");
        assert!(
            rep > idx * 20,
            "pgn replay should dominate indexed preview (idx {idx:?}, rep {rep:?})"
        );
    }

    #[test]
    fn analyze_all_jobs_cheaper_than_full_library_clone() {
        let path = default_db_path();
        if !path.is_file() {
            eprintln!("skip: no db at {}", path.display());
            return;
        }
        let lib = load_library(&path).expect("load library");

        let mut job_samples = Vec::new();
        for _ in 0..20 {
            let t0 = Instant::now();
            let jobs: Vec<(String, String, String)> = lib
                .games
                .iter()
                .filter(|g| !g.is_analyzed())
                .map(|g| (g.id.clone(), g.game_type.clone(), g.pgn.clone()))
                .collect();
            std::hint::black_box(jobs);
            job_samples.push(t0.elapsed());
        }
        job_samples.sort();
        let jobs = job_samples[job_samples.len() / 2];

        let mut clone_samples = Vec::new();
        for _ in 0..20 {
            let t0 = Instant::now();
            let snap = lib.clone();
            std::hint::black_box(snap);
            clone_samples.push(t0.elapsed());
        }
        clone_samples.sort();
        let full = clone_samples[clone_samples.len() / 2];

        eprintln!(
            "analyze-all jobs {:?} · full Library clone {:?} · frame {:?}",
            jobs, full, FRAME
        );
        assert!(
            jobs < FRAME || jobs < full,
            "job list should fit a frame or beat full clone"
        );
        assert!(
            jobs <= full,
            "unanalyzed job list must not exceed full library clone"
        );
    }
}
