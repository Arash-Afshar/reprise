//! Native Stockfish via UCI (in-process pool — no analysis-server).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};

#[derive(Debug, Clone)]
pub struct Score {
    pub kind: ScoreKind,
    pub value: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScoreKind {
    Cp,
    Mate,
}

#[derive(Debug, Clone)]
pub struct InfoLine {
    pub depth: Option<u32>,
    pub multipv: u32,
    pub score: Score,
    pub pv: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AnalyzeLines {
    pub lines: Vec<InfoLine>,
}

type JobFn = Box<dyn FnOnce(&mut Stockfish) + Send>;

pub struct PoolConfig {
    pub workers: usize,
    pub threads_per_worker: usize,
    pub hash_mb: u32,
}

pub fn default_pool_config() -> PoolConfig {
    let cpus = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let workers = (cpus / 3).clamp(2, 8);
    let threads_per_worker = (cpus / workers).max(2);
    PoolConfig {
        workers,
        threads_per_worker,
        hash_mb: 256,
    }
}

/// Official Linux AVX2 build used when nothing is installed locally.
const DEFAULT_STOCKFISH_URL: &str = "https://github.com/official-stockfish/Stockfish/releases/download/sf_17/stockfish-ubuntu-x86-64-avx2.tar";

pub fn default_bin_stockfish() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bin/stockfish")
}

pub fn resolve_stockfish_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("STOCKFISH_PATH") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let bundled = default_bin_stockfish();
    if bundled.is_file() {
        return Some(bundled);
    }
    stockfish_on_path()
}

fn stockfish_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("stockfish");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Resolve Stockfish, downloading an official release into `bin/` on first need.
pub fn ensure_stockfish() -> Result<PathBuf> {
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK
        .lock()
        .map_err(|_| anyhow!("stockfish ensure lock poisoned"))?;

    if let Some(p) = resolve_stockfish_path() {
        return Ok(p);
    }

    let dest = default_bin_stockfish();
    eprintln!(
        "reprise: Stockfish not found — downloading official release to {} …",
        dest.display()
    );
    download_official_stockfish(&dest)?;
    eprintln!("reprise: Stockfish ready at {}", dest.display());
    Ok(dest)
}

fn download_official_stockfish(dest: &Path) -> Result<()> {
    use std::fs::{self, File};
    use std::io::copy;

    let url = std::env::var("STOCKFISH_URL").unwrap_or_else(|_| DEFAULT_STOCKFISH_URL.to_string());
    let bin_dir = dest
        .parent()
        .ok_or_else(|| anyhow!("invalid stockfish destination"))?;
    fs::create_dir_all(bin_dir).context("create bin/")?;

    let tmp_dir = bin_dir.join(".stockfish-download");
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(&tmp_dir).context("create download temp dir")?;
    let tmp_tar = tmp_dir.join("stockfish.tar");

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(180)))
        .build()
        .into();
    let mut res = agent
        .get(&url)
        .call()
        .with_context(|| format!("download Stockfish from {url}"))?;
    if !res.status().is_success() {
        bail!("Stockfish download failed: HTTP {}", res.status());
    }
    {
        let mut file = File::create(&tmp_tar).context("create stockfish.tar")?;
        copy(&mut res.body_mut().as_reader(), &mut file).context("write stockfish.tar")?;
    }

    let status = Command::new("tar")
        .args(["-xf"])
        .arg(&tmp_tar)
        .arg("-C")
        .arg(&tmp_dir)
        .status()
        .context("run tar to extract Stockfish")?;
    if !status.success() {
        bail!("tar extract failed with {status}");
    }

    let extracted = find_extracted_stockfish(&tmp_dir)
        .ok_or_else(|| anyhow!("could not find stockfish binary in downloaded archive"))?;
    let staging = dest.with_extension("new");
    fs::copy(&extracted, &staging).context("copy stockfish into place")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o755))
            .context("chmod stockfish")?;
    }
    fs::rename(&staging, dest).context("install stockfish binary")?;
    let _ = fs::remove_dir_all(&tmp_dir);
    Ok(())
}

fn find_extracted_stockfish(root: &Path) -> Option<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("stockfish") && !name.ends_with(".tar") {
                    out.push(path);
                }
            }
        }
    }
    let mut found = Vec::new();
    walk(root, &mut found);
    found.into_iter().next()
}

pub struct EnginePool {
    job_tx: Sender<JobFn>,
    _workers: Vec<thread::JoinHandle<()>>,
}

/// Cloneable handle for submitting work (safe to share across threads).
#[derive(Clone)]
pub struct PoolHandle {
    job_tx: Sender<JobFn>,
}

impl EnginePool {
    pub fn handle(&self) -> PoolHandle {
        PoolHandle {
            job_tx: self.job_tx.clone(),
        }
    }

    pub fn start(binary: &Path, cfg: &PoolConfig) -> Result<Self> {
        let (job_tx, job_rx) = mpsc::channel::<JobFn>();
        let job_rx = Arc::new(Mutex::new(job_rx));
        let mut workers = Vec::with_capacity(cfg.workers);

        for i in 0..cfg.workers {
            let rx = Arc::clone(&job_rx);
            let binary = binary.to_path_buf();
            let threads = cfg.threads_per_worker;
            let hash_mb = cfg.hash_mb;
            let handle = thread::Builder::new()
                .name(format!("stockfish-{i}"))
                .spawn(move || {
                    let mut eng = match Stockfish::spawn(&binary, threads, hash_mb) {
                        Ok(e) => e,
                        Err(err) => {
                            eprintln!("reprise: stockfish worker {i} failed to start: {err:#}");
                            return;
                        }
                    };
                    if let Err(err) = eng.init() {
                        eprintln!("reprise: stockfish worker {i} init failed: {err:#}");
                        return;
                    }
                    loop {
                        let job = {
                            let guard = match rx.lock() {
                                Ok(g) => g,
                                Err(_) => break,
                            };
                            guard.recv()
                        };
                        match job {
                            Ok(job) => job(&mut eng),
                            Err(_) => break,
                        }
                    }
                    eng.quit();
                })
                .context("spawn stockfish worker")?;
            workers.push(handle);
        }

        Ok(Self {
            job_tx,
            _workers: workers,
        })
    }

    pub fn run<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Stockfish) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        self.handle().run(f)
    }
}

impl PoolHandle {
    pub fn run<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Stockfish) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let (tx, rx) = mpsc::sync_channel(1);
        self.job_tx
            .send(Box::new(move |eng| {
                let _ = tx.send(f(eng));
            }))
            .map_err(|_| anyhow!("engine pool shut down"))?;
        rx.recv()
            .map_err(|_| anyhow!("engine worker dropped job"))?
    }
}

pub struct Stockfish {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    threads: usize,
    hash_mb: u32,
    _reader: thread::JoinHandle<()>,
}

impl Stockfish {
    pub fn spawn(binary: &Path, threads: usize, hash_mb: u32) -> Result<Self> {
        let mut child = Command::new(binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("spawn {}", binary.display()))?;
        let stdin = child.stdin.take().context("stockfish stdin")?;
        let stdout = child.stdout.take().context("stockfish stdout")?;
        let (tx, lines) = mpsc::channel();
        let reader = thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().flatten() {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            lines,
            threads,
            hash_mb,
            _reader: reader,
        })
    }

    fn post(&mut self, cmd: &str) -> Result<()> {
        writeln!(self.stdin, "{cmd}").context("write stockfish")?;
        self.stdin.flush().context("flush stockfish")?;
        Ok(())
    }

    fn wait_for(&self, prefix: &str, timeout: Duration) -> Result<String> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!("timeout waiting for {prefix}");
            }
            match self.lines.recv_timeout(remaining) {
                Ok(line) => {
                    if line == prefix || line.starts_with(prefix) {
                        return Ok(line);
                    }
                    // Non-matching lines during wait are ignored here; analyze()
                    // uses a dedicated collector that reads until bestmove.
                }
                Err(mpsc::RecvTimeoutError::Timeout) => bail!("timeout waiting for {prefix}"),
                Err(mpsc::RecvTimeoutError::Disconnected) => bail!("stockfish stdout closed"),
            }
        }
    }

    /// Drain lines until `bestmove`, invoking `on_line` for each.
    fn collect_until_bestmove(
        &self,
        on_line: &mut dyn FnMut(&str),
        timeout: Duration,
    ) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!("timeout waiting for bestmove");
            }
            match self.lines.recv_timeout(remaining) {
                Ok(line) => {
                    if line.starts_with("bestmove") {
                        return Ok(());
                    }
                    on_line(&line);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => bail!("timeout waiting for bestmove"),
                Err(mpsc::RecvTimeoutError::Disconnected) => bail!("stockfish stdout closed"),
            }
        }
    }

    pub fn init(&mut self) -> Result<()> {
        self.post("uci")?;
        self.wait_for("uciok", Duration::from_secs(30))?;
        self.post(&format!("setoption name Threads value {}", self.threads))?;
        self.post(&format!("setoption name Hash value {}", self.hash_mb))?;
        self.post("setoption name UCI_AnalyseMode value true")?;
        self.post("isready")?;
        self.wait_for("readyok", Duration::from_secs(30))?;
        Ok(())
    }

    pub fn new_game(&mut self) -> Result<()> {
        self.post("ucinewgame")?;
        self.post("isready")?;
        self.wait_for("readyok", Duration::from_secs(30))?;
        Ok(())
    }

    pub fn analyze(&mut self, fen: &str, depth: u32, multipv: u32) -> Result<AnalyzeLines> {
        let mut pv_map: HashMap<u32, InfoLine> = HashMap::new();
        self.post(&format!("setoption name MultiPV value {multipv}"))?;
        self.post(&format!("position fen {fen}"))?;
        self.post(&format!("go depth {depth}"))?;
        self.collect_until_bestmove(
            &mut |line| {
                if line.starts_with("info ") && line.contains(" pv ") {
                    if let Some(parsed) = parse_info_line(line) {
                        let prev = pv_map.get(&parsed.multipv);
                        if prev.is_none_or(|p| parsed.depth.unwrap_or(0) >= p.depth.unwrap_or(0)) {
                            pv_map.insert(parsed.multipv, parsed);
                        }
                    }
                }
            },
            Duration::from_secs(180),
        )?;
        let mut lines: Vec<_> = pv_map.into_values().collect();
        lines.sort_by_key(|l| l.multipv);
        Ok(AnalyzeLines { lines })
    }

    pub fn score_move(
        &mut self,
        fen: &str,
        uci_move: &str,
        depth: u32,
    ) -> Result<Option<InfoLine>> {
        let mut last: Option<InfoLine> = None;
        self.post("setoption name MultiPV value 1")?;
        self.post(&format!("position fen {fen}"))?;
        self.post(&format!("go depth {depth} searchmoves {uci_move}"))?;
        self.collect_until_bestmove(
            &mut |line| {
                if line.starts_with("info ") && line.contains(" pv ") {
                    if let Some(parsed) = parse_info_line(line) {
                        if last
                            .as_ref()
                            .is_none_or(|p| parsed.depth.unwrap_or(0) >= p.depth.unwrap_or(0))
                        {
                            last = Some(parsed);
                        }
                    }
                }
            },
            Duration::from_secs(180),
        )?;
        Ok(last)
    }

    pub fn quit(&mut self) {
        let _ = self.post("quit");
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Stockfish {
    fn drop(&mut self) {
        self.quit();
    }
}

pub fn parse_info_line(line: &str) -> Option<InfoLine> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    let mut depth = None;
    let mut multipv = 1u32;
    let mut score = None;
    let mut pv = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        match parts[i] {
            "depth" => {
                i += 1;
                depth = parts.get(i).and_then(|s| s.parse().ok());
            }
            "multipv" => {
                i += 1;
                multipv = parts.get(i).and_then(|s| s.parse().ok()).unwrap_or(1);
            }
            "score" => {
                i += 1;
                let kind = *parts.get(i)?;
                i += 1;
                let value: i32 = parts.get(i)?.parse().ok()?;
                score = Some(Score {
                    kind: match kind {
                        "mate" => ScoreKind::Mate,
                        _ => ScoreKind::Cp,
                    },
                    value,
                });
            }
            "pv" => {
                pv = parts[i + 1..].iter().map(|s| (*s).to_string()).collect();
                break;
            }
            _ => {}
        }
        i += 1;
    }
    Some(InfoLine {
        depth,
        multipv,
        score: score?,
        pv,
    })
}

pub fn score_to_cp(score: &Score) -> i32 {
    match score.kind {
        ScoreKind::Mate => {
            if score.value > 0 {
                100_000 - score.value
            } else {
                -100_000 - score.value
            }
        }
        ScoreKind::Cp => score.value,
    }
}

pub fn score_to_white_pawns(score: &Score, side_to_move: &str) -> f64 {
    let v = match score.kind {
        ScoreKind::Mate => score_to_cp(score) as f64 / 100.0,
        ScoreKind::Cp => score.value as f64 / 100.0,
    };
    if side_to_move == "b" {
        -v
    } else {
        v
    }
}

pub fn format_score(score: Option<&Score>, side_to_move: &str) -> String {
    let Some(score) = score else {
        return "—".into();
    };
    if score.kind == ScoreKind::Mate {
        let mate = if side_to_move == "b" {
            -score.value
        } else {
            score.value
        };
        return format!("M{mate}");
    }
    let pawns = score_to_white_pawns(score, side_to_move);
    if pawns > 0.0 {
        format!("+{pawns:.2}")
    } else {
        format!("{pawns:.2}")
    }
}

pub fn centipawn_loss(best: Option<&Score>, played: Option<&Score>) -> i32 {
    match (best, played) {
        (Some(b), Some(p)) => (score_to_cp(b) - score_to_cp(p)).max(0),
        _ => 0,
    }
}

pub fn classify_loss(loss_cp: i32, best: Option<&Score>, played: Option<&Score>) -> &'static str {
    if let (Some(best), Some(played)) = (best, played) {
        if best.kind == ScoreKind::Mate
            && best.value > 0
            && played.kind != ScoreKind::Mate
            && loss_cp >= 300
        {
            return "blunder";
        }
        if played.kind == ScoreKind::Mate && played.value < 0 {
            return if loss_cp >= 200 { "blunder" } else { "mistake" };
        }
    }
    if loss_cp <= 10 {
        "best"
    } else if loss_cp <= 25 {
        "excellent"
    } else if loss_cp <= 50 {
        "good"
    } else if loss_cp <= 100 {
        "inaccuracy"
    } else if loss_cp <= 300 {
        "mistake"
    } else {
        "blunder"
    }
}
