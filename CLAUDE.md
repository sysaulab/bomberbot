# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

A Rust terminal (TUI) Bomberman game using `ratatui` + `crossterm`, single binary crate `bomberman-tui` (edition 2021). One source file: `src/main.rs`.

## Commands

- `cargo run` — build and launch the game in the terminal.
- `cargo run -- --gamepad 0` — enable gamepad input; default id `0`; on startup the user is prompted to press buttons to map UP/DOWN/LEFT/RIGHT/BOMB.
- `cargo build` / `cargo build --release` — compile.
- `cargo test` — run tests (none currently exist in the repo).
- `cargo test <name>` — run a single test by name/substring.
- `cargo clippy` — lints; `cargo fmt` — formatting (rustfmt default).
- `cargo clean` — remove `target/`.

## Architecture

`src/main.rs` is the entire program. Key pieces:

- **Constants block** (top of file): tunables — `BOMB_FUSE_MS`, `EXPLOSION_LIFETIME_MS`, `ENEMY_MOVE_MS`, scores.
- **Terrain/Entities**: `Cell` enum (`Empty/Solid/Brick/Door`); `Bomb`, `Explosion`, `Enemy` structs. `Enemy::new` picks a random initial direction.
- **`Game` struct** centralizes all state: grid, player coords, bombs, explosions, enemies, door, score/level, `max_bombs`, game_over.
  - `Game::new` → `generate_level()` seeds grid (solid border, odd/even pillars, random bricks), hides the exit door under a brick far from start, spawns enemies on open tiles ≥5 Manhattan from player. Player starts at `(1,1)` with a cleared pocket.
  - `move_player`: wall/bomb/fire/enemy-collision handling; stepping onto `Door` calls `next_level` (adds `LEVEL_SCORE`, increments level, regenerates).
  - `place_bomb`: respects `max_bombs` cap and no-stacking.
  - `update` (called every 50ms tick): fuse expiry → `create_explosion` + chain reactions; expire explosions; kill enemies in fire; move survivors on their timer; player death checks.
  - `create_explosion`: central cell + 4-spoke raycasts; bricks shatter (and may reveal the door if `door_pos`); doors survive, fire passes through; enemies scored via `ENEMY_SCORE`.
  - `render`: ratatui `Frame`; status line, then field grid with overlay priority fire > bomb > player > enemy > terrain (all glyphs are two columns for alignment), then game-over banner.
- **Main loop** (`main`): enables raw mode + alternate screen; draws each frame; polls input with `render_rate` (33ms) timeout; dispatches keys (q quit, r restart on game over, WASD/arrows move, space bomb); calls `game.update()` on `tick_rate`. On exit restores terminal mode.

## Notes

- Powerups: each level hides one powerup under a brick distinct from the door. Kinds: `BombUp` (+max bombs), `FireUp` (+blast range), `SpeedUp` (shorter fuse). Collected on walk-over.
- Lives: start `STARTING_LIVES` (3); level completion grants +1 life; death decrements and respawns at (1,1) until 0 → game over.
- Audio: WAV files generated at runtime into a temp dir and played via `afplay` (macOS) / `aplay` (Linux); `Audio::new()` returns `None` if no player exists and the game runs silently. SFX and a looping generated chiptune melody.
- `max_bombs` starts at 1 and grows via BombUp; blast range starts 2 and grows via FireUp; fuse starts `BOMB_FUSE_MS` and shrinks via SpeedUp (min 800ms).
- The door is represented as a `Brick` until blasted; `door_pos` is tracked separately.
- Explosion lifetime predicate in `update` is `expires_at > now` (retain active).
- There is no persistence, no tests, and no config files.