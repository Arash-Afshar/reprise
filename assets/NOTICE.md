# Third-party notices

## Chess piece artwork (`assets/pieces/`)

**Chessnut** SVG pieces by Alexis Luengas.

- Copyright 2015 Alexis Luengas
- License: Apache License 2.0 (see `LICENSE.Apache-2.0.txt`)
- Upstream: https://github.com/LexLuengas/chessnut-pieces

Apache-2.0 is compatible with this project's MIT license. The Apache notice
and copyright must be preserved when redistributing these files.

## Stockfish (not vendored)

Official Stockfish releases are **GPL-3.0**. On first analysis (or via
`scripts/fetch-stockfish.sh`), Reprise may download an official Linux binary into
`bin/`. That binary is **not** part of the MIT-licensed source tree and must not
be redistributed as if it were MIT.

Upstream: https://github.com/official-stockfish/Stockfish

## Intentionally not used

- **Cburnett / Lichess `public/piece/cburnett`**: attributed as GPLv2+ in Lichess
  COPYING.md — incompatible with shipping an MIT application that embeds the art.
- **`shakmaty` crate**: GPL-3.0-or-later — would require the whole app to be GPL.
