# Reprise

**Chess Reprise** — *Reprise* for short.

A *reprise* is a return: music comes back to a theme; theater revisits a moment. This app’s loop is exactly that — reopen a game and walk the pivotal moments again, not live play or a scorecard dump.

Local, keyboard-first chess postmortem **for Omarchy** (Rust + GTK4).
Built for Omarchy and agent integration; other environments are not a goal for now.

**One binary:** sync, library, board, and Stockfish analysis all live in this app.

## Requirements

- Omarchy (theme palette + `Ctrl+A` agent hook)
- Rust (edition 2024 toolchain)
- GTK 4 development libraries (for building)
- Network access for chess.com sync and (on first Analyze) Stockfish download
- `curl` / `tar` only if you prefetch Stockfish yourself via the optional script

## Run

```bash
cargo run --release
```

Stockfish (GPLv3) is **not** vendored. On first Analyze, if none is found, Reprise downloads an official Linux AVX2 build into `bin/stockfish`. Override with:

1. `STOCKFISH_PATH` — explicit binary
2. `bin/stockfish` — local install (auto-downloaded or prefetched)
3. `stockfish` on your `PATH` (e.g. distro package)

Optional prefetch (no Node required):

```bash
./scripts/fetch-stockfish.sh
# or: STOCKFISH_URL=… ./scripts/fetch-stockfish.sh
```

Your game library is stored under `data/db.json` (slim index: `data/db-index.json`).
Those files are gitignored. Override the path with `REPRISE_DB=/path/to/db.json`.

On launch the UI appears immediately; chess.com sync runs in the background when online.
On first run Reprise asks for your chess.com username, then imports the newest 5 games
right away and continues the rest of history in the background.
**Analyze** / **Analyze all** run Stockfish **in-process** (on demand only; first run may download the engine).

### Playbench keys

| Key | Action |
|-----|--------|
| `←` `→` / `h` `l` | Step through plies |
| `Home` / `End` | First / last ply |
| `a` | Analyze current game (confirms before reanalyze) |
| `aa` | Analyze all unanalyzed games |
| `o` | Games list |
| `s` | Sync — fetch latest games missing from the library |
| `Ctrl+A` | Send position + moment text to Omarchy default agent |
| `b` | Toggle chrome buttons |
| `n` / `p` | Next / previous **pivotal moment** |
| Prev pivotal / Next pivotal | Same as `n` / `p` |
| Analyze | Engine-analyze current game |
| Analyze all | Engine-analyze every unanalyzed game |

After analysis loads, Reprise jumps to the first pivotal moment so you can walk the spine of the game.

### Games list (`o`)

Three sections — jump with `1` / `2` / `3`:
1. Recently reviewed (3) — analyzed and opened on the playbench (`p`, auto-analyze after sync, or Enter on an analyzed game). List `a` / Analyze all alone do not count.
2. Recent — recent games not yet reviewed (includes analyzed-but-not-opened)
3. All games (newest→oldest, loads in pages)

Selecting a row previews the **final position** on a full-size board.
`?` shows list-only keybindings (playbench `?` shows playbench keys).

| Key | Action |
|-----|--------|
| `1` `2` `3` | Jump to section |
| `↑` `↓` / `j` `k` | Move within list |
| `Enter` | Open on playbench |
| `p` | Open on playbench + analyze if needed |
| `a` | Analyze here (stay on list) |
| `s` | Sync |
| `/` | Filter |
| `?` | Keybindings |
| `Esc` | Back to playbench |

Press `?` for keybindings (Esc to close).

## Theming

Uses the active Omarchy palette from:

`~/.local/state/omarchy/current/theme/colors.toml`

## License

Reprise source code is **MIT** — see [`LICENSE`](LICENSE).

Third-party notices (piece art, etc.) are in [`assets/NOTICE.md`](assets/NOTICE.md).

**Stockfish** is separate software under the **GNU GPL v3**. This repository does not vendor the Stockfish binary; on first Analyze (or via `scripts/fetch-stockfish.sh`) an official release may be downloaded into `bin/` for local use. Redistributing a Stockfish binary with your build is subject to GPL terms, independent of Reprise’s MIT license.
