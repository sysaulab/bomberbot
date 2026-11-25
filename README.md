# 💣🤖 bomberbot

A retro terminal Bomberman-style arcade game written in Rust with [`ratatui`](https://github.com/ratatui/ratatui), procedural chiptune audio, and gamepad support.

## ✨ Features

- **50 Escalating Levels:** Brick density ramps from 10% to 90% as you progress, accompanied by shifting enemy rosters (`👾` Critters, `🧌` Zombies that actively hunt you down, and `👻` Ghosts that phase through bricks).
- **Classic Powerups:** Uncover hidden upgrades beneath destructible bricks:
  - 💣 **Bomb Up:** Increase max simultaneous bombs
  - 🔥 **Fire Up:** Extend blast radius
  - 🧨 **Detonator:** Trigger active bombs on demand
  - ❤️ **Life Up:** Gain an extra life
- **Zero-Dependency Procedural Audio:** Synthesizes square/triangle chiptune melodies, basslines, hi-hats, and sound effects straight to WAV on startup—no external audio libraries required.
- **Keyboard & Gamepad Support:** Play right from the keyboard or map any controller at launch via [`gilrs`](https://gitlab.com/gilrs-project/gilrs).

---

## 🚀 Getting Started

### Prerequisites

- **Rust** (2021 edition / `cargo`)
- **Terminal** with Unicode/emoji support and a monospace font
- **Audio playback** *(optional)*:
  - **macOS:** Uses built-in `afplay`
  - **Linux:** Uses `aplay` (typically provided by `alsa-utils`)

### Build & Run

```bash
git clone https://github.com/<your-username>/bomberbot.git
cd bomberbot
cargo run --release
```

---

## 🎮 Controls

| Action | Keyboard |
| :--- | :--- |
| **Move** | `W` `A` `S` `D` or `↑` `↓` `←` `→` |
| **Place Bomb** | `Space` |
| **Detonate** *(with 🧨)* | `Left Shift` / `Right Shift` |
| **Pause / Resume** | `P` or `Esc` |
| **Mute / Unmute Audio** | `M` |
| **Restart** *(Game Over / Win)* | `R` |
| **Quit** | `Q` |

> **Tip:** Clear all enemies on the stage and step onto the hidden exit door (`🚪`) to advance to the next level.

---

## ⚙️ Command-Line Options

```bash
cargo run --release -- [OPTIONS]
```

| Flag | Description | Default |
| :--- | :--- | :--- |
| `--width <N>` | Arena width (clamped to an odd integer $\ge 9$) | `15` |
| `--height <N>` | Arena height (clamped to an odd integer $\ge 9$) | `15` |
| `--gamepad <N>` | Select gamepad index `N` and run interactive button mapping | `0` |
| `--no-audio` | Disable procedural music and sound effects | *enabled* |

### Examples

Play on a larger arena without audio:
```bash
cargo run --release -- --width 21 --height 17 --no-audio
```

Use a second connected controller:
```bash
cargo run --release -- --gamepad 1
```

---

## 📄 License

This project is open source and available under the [MIT License](LICENSE).