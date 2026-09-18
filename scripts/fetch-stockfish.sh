#!/usr/bin/env bash
# Optional manual prefetch. Reprise also downloads Stockfish on first Analyze
# when STOCKFISH_PATH / bin/stockfish / PATH are all missing.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/bin/stockfish"
URL="${STOCKFISH_URL:-https://github.com/official-stockfish/Stockfish/releases/download/sf_17/stockfish-ubuntu-x86-64-avx2.tar}"

if [[ -f "$DEST" ]]; then
  echo "Stockfish already present at $DEST"
  exit 0
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

mkdir -p "$ROOT/bin"
echo "Downloading Stockfish…"
curl -fsSL "$URL" -o "$TMP/stockfish.tar"
tar -xf "$TMP/stockfish.tar" -C "$TMP"

BIN="$(find "$TMP" -type f -name 'stockfish*' ! -name '*.tar' | head -n 1 || true)"
if [[ -z "$BIN" ]]; then
  echo "Could not find stockfish binary in archive" >&2
  exit 1
fi

install -m 755 "$BIN" "$DEST"
echo "Installed $DEST"
