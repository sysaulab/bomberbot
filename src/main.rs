use std::{
    io,
    time::{Duration, Instant},
};

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use gilrs::{Button, EventType, Gilrs};
use rand::Rng;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Text,
    widgets::Paragraph,
    Frame, Terminal,
};
use std::collections::HashMap;
use std::io::Write as IoWrite;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Tunables
// ---------------------------------------------------------------------------

const BOMB_FUSE_MS: u64 = 2500;
const EXPLOSION_LIFETIME_MS: u64 = 750;
const ENEMY_MOVE_MS: u64 = 400;

const BRICK_SCORE: u32 = 10;
const ENEMY_SCORE: u32 = 100;
const LEVEL_SCORE: u32 = 500;
const POWERUP_SCORE: u32 = 50;

const STARTING_LIVES: u32 = 3;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Sound {
    Bomb,
    Explosion,
    Powerup,
    LevelUp,
    Death,
    Kill,
}

// ---------------------------------------------------------------------------
// Terrain
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum PowerupKind {
    BombUp,
    FireUp,
    LifeUp,
    Detonator,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cell {
    Empty,
    Solid,
    Brick,
    Door,
    Powerup(PowerupKind),
}

// ---------------------------------------------------------------------------
// Entities
// ---------------------------------------------------------------------------

struct Bomb {
    x: usize,
    y: usize,
    placed_at: Instant,
    range: usize,
}

struct Explosion {
    x: usize,
    y: usize,
    expires_at: Instant,
}

struct Enemy {
    x: usize,
    y: usize,
    dx: isize,
    dy: isize,
    last_move: Instant,
    alive: bool,
}

impl Enemy {
    fn new(x: usize, y: usize, now: Instant) -> Self {
        let dirs = [(1isize, 0isize), (-1, 0), (0, 1), (0, -1)];
        let (dx, dy) = dirs[rand::thread_rng().gen_range(0..4)];
        Enemy {
            x,
            y,
            dx,
            dy,
            last_move: now,
            alive: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Game
// ---------------------------------------------------------------------------

struct Game {
    width: usize,
    height: usize,
    grid: Vec<Vec<Cell>>,
    player: (usize, usize),
    bombs: Vec<Bomb>,
    explosions: Vec<Explosion>,
    enemies: Vec<Enemy>,
    door_pos: (usize, usize),
    powerup_brick: Option<(usize, usize)>,
    powerup_kind: PowerupKind,
    score: u32,
    level: u32,
    lives: u32,
    max_bombs: usize,
    bomb_range: usize,
    bomb_fuse_ms: u64,
    has_detonator: bool,
    game_over: bool,
    invincible_until: Option<Instant>,
    frame: u64,
    sounds: Vec<Sound>,
}

impl Game {
    fn new(width: usize, height: usize) -> Self {
        let mut game = Game {
            width,
            height,
            grid: vec![vec![Cell::Empty; width]; height],
            player: (1, 1),
            bombs: Vec::new(),
            explosions: Vec::new(),
            enemies: Vec::new(),
            door_pos: (1, 1),
            powerup_brick: None,
            powerup_kind: PowerupKind::BombUp,
            score: 0,
            level: 1,
            lives: STARTING_LIVES,
            max_bombs: 1,
            bomb_range: 1,
            bomb_fuse_ms: BOMB_FUSE_MS,
            has_detonator: false,
            game_over: false,
            invincible_until: None,
            frame: 0,
            sounds: Vec::new(),
        };
        game.generate_level();
        game
    }

    // -----------------------------------------------------------------------
    // Level generation
    // -----------------------------------------------------------------------

    fn generate_level(&mut self) {
        let w = self.width;
        let h = self.height;
        let mut grid = vec![vec![Cell::Empty; w]; h];

        // solid border
        for x in 0..w {
            grid[0][x] = Cell::Solid;
            grid[h - 1][x] = Cell::Solid;
        }
        for y in 0..h {
            grid[y][0] = Cell::Solid;
            grid[y][w - 1] = Cell::Solid;
        }

        // pillars + random bricks
        let mut brick_cells = Vec::new();
        for y in 2..h - 1 {
            for x in 2..w - 1 {
                if x % 2 == 0 && y % 2 == 0 {
                    grid[y][x] = Cell::Solid;
                } else if rand::thread_rng().gen_bool(0.25) {
                    grid[y][x] = Cell::Brick;
                    brick_cells.push((x, y));
                }
            }
        }

        // clear a starting pocket at (1,1)
        grid[1][1] = Cell::Empty;
        if w > 2 {
            grid[1][2] = Cell::Empty;
        }
        if h > 2 {
            grid[2][1] = Cell::Empty;
        }

        // hide the door under a brick, far from start if possible
        let far_candidates: Vec<(usize, usize)> = brick_cells
            .iter()
            .copied()
            .filter(|&(x, y)| (x + y) > 6)
            .collect();
        let door_pos = if !far_candidates.is_empty() {
            far_candidates[rand::thread_rng().gen_range(0..far_candidates.len())]
        } else if !brick_cells.is_empty() {
            brick_cells[rand::thread_rng().gen_range(0..brick_cells.len())]
        } else {
            (w - 3, h - 3)
        };

        // if no brick ended up at door_pos, reveal the door immediately
        if grid[door_pos.1][door_pos.0] != Cell::Brick {
            grid[door_pos.1][door_pos.0] = Cell::Door;
        }

        // pick one brick (distinct from the door) to hide a powerup under
        use rand::Rng;
        let powerup_candidates: Vec<(usize, usize)> = brick_cells
            .iter()
            .copied()
            .filter(|&p| p != door_pos)
            .collect();
        let powerup_brick = if !powerup_candidates.is_empty() {
            Some(powerup_candidates[rand::thread_rng().gen_range(0..powerup_candidates.len())])
        } else {
            None
        };
        // randomize which powerup this level offers
        let kinds = [
            PowerupKind::BombUp,
            PowerupKind::FireUp,
            PowerupKind::BombUp,
            PowerupKind::FireUp,
            PowerupKind::BombUp,
            PowerupKind::FireUp,
            PowerupKind::Detonator,
            PowerupKind::LifeUp,
        ];
        let powerup_kind = kinds[rand::thread_rng().gen_range(0..kinds.len())];

        // spawn enemies on empty tiles far from start
        let now = Instant::now();
        let mut enemies = Vec::new();
        let target = 2 + self.level as usize; // 3, 4, 5, ...
        let mut attempts = 0;
        while enemies.len() < target && attempts < 500 {
            attempts += 1;
            let x = rand::thread_rng().gen_range(1..w - 1);
            let y = rand::thread_rng().gen_range(1..h - 1);
            if grid[y][x] != Cell::Empty {
                continue;
            }
            if x == 1 && y == 1 {
                continue;
            }
            let manhattan = (x as isize - 1).abs() + (y as isize - 1).abs();
            if manhattan < 5 {
                continue;
            }
            if enemies.iter().any(|e: &Enemy| e.x == x && e.y == y) {
                continue;
            }
            enemies.push(Enemy::new(x, y, now));
        }

        self.grid = grid;
        self.player = (1, 1);
        self.bombs.clear();
        self.explosions.clear();
        self.enemies = enemies;
        self.door_pos = door_pos;
        self.powerup_brick = powerup_brick;
        self.powerup_kind = powerup_kind;
    }

    fn next_level(&mut self) {
        self.score += LEVEL_SCORE;
        self.level += 1;
        self.lives += 1;
        self.sounds.push(Sound::LevelUp);
        self.generate_level();
    }

    // -----------------------------------------------------------------------
    // Player actions
    // -----------------------------------------------------------------------

    fn move_player(&mut self, dx: isize, dy: isize) {
        if self.game_over {
            return;
        }

        let nx = self.player.0 as isize + dx;
        let ny = self.player.1 as isize + dy;
        if nx < 0 || ny < 0 || nx >= self.width as isize || ny >= self.height as isize {
            return;
        }
        let nx = nx as usize;
        let ny = ny as usize;

        match self.grid[ny][nx] {
            Cell::Solid | Cell::Brick => return,
            _ => {}
        }
        if self.bombs.iter().any(|b| b.x == nx && b.y == ny) {
            return;
        }
        // fire is blocked (player dies when fire spawns *under* them instead)
        if self.explosions.iter().any(|e| e.x == nx && e.y == ny) {
            return;
        }

        self.player = (nx, ny);

        // collect a revealed powerup
        if let Cell::Powerup(kind) = self.grid[ny][nx] {
            match kind {
                PowerupKind::BombUp => self.max_bombs += 1,
                PowerupKind::FireUp => self.bomb_range += 1,
                PowerupKind::LifeUp => self.lives += 1,
                PowerupKind::Detonator => self.has_detonator = true,
            }
            self.score += POWERUP_SCORE;
            self.grid[ny][nx] = Cell::Empty;
            self.sounds.push(Sound::Powerup);
        }

        // instant death from touching an enemy (skip while invincible after reincarnation)
        if !self.is_invincible()
            && self
                .enemies
                .iter()
                .any(|e| e.alive && e.x == nx && e.y == ny)
        {
            if self.lives > 1 {
                self.lives -= 1;
                self.respawn();
            } else {
                self.game_over = true;
            }
            self.sounds.push(Sound::Death);
            return;
        }

        // reached the door
        if self.grid[ny][nx] == Cell::Door {
            self.next_level();
        }
    }

    fn place_bomb(&mut self) {
        if self.game_over {
            return;
        }
        if self.bombs.len() >= self.max_bombs {
            return;
        }
        if self
            .bombs
            .iter()
            .any(|b| b.x == self.player.0 && b.y == self.player.1)
        {
            return;
        }
        self.bombs.push(Bomb {
            x: self.player.0,
            y: self.player.1,
            placed_at: Instant::now(),
            range: self.bomb_range,
        });
        self.sounds.push(Sound::Bomb);
    }

    fn respawn(&mut self) {
        self.player = (1, 1);
        self.bombs.clear();
        self.explosions.clear();
        self.invincible_until = Some(Instant::now() + Duration::from_secs(1));
    }

    fn detonate(&mut self) -> bool {
        if !self.has_detonator || self.bombs.is_empty() {
            return false;
        }
        let now = Instant::now();
        for b in &mut self.bombs {
            b.placed_at = now - Duration::from_millis(self.bomb_fuse_ms + 1);
        }
        true
    }

    fn is_invincible(&self) -> bool {
        self.invincible_until.map_or(false, |t| Instant::now() < t)
    }

    pub fn drain_sounds(&mut self) -> Vec<Sound> {
        std::mem::take(&mut self.sounds)
    }

    // -----------------------------------------------------------------------
    // Tick
    // -----------------------------------------------------------------------

    fn update(&mut self) {
        if self.game_over {
            return;
        }
        let now = Instant::now();

        // 1) explode any bombs whose fuse has run out
        let mut bombs_to_explode = Vec::new();
        for (i, bomb) in self.bombs.iter().enumerate() {
            if now.duration_since(bomb.placed_at) >= Duration::from_millis(self.bomb_fuse_ms) {
                bombs_to_explode.push(i);
            }
        }

        for i in bombs_to_explode.clone().into_iter().rev() {
            let bomb = self.bombs.remove(i);
            let mut new_explosions = Vec::new();
            self.create_explosion(bomb.x, bomb.y, bomb.range, &mut new_explosions);

            // chain reaction: any bomb caught in the blast blows on the next tick
            for exp in &new_explosions {
                if let Some(pos) = self.bombs.iter().position(|b| b.x == exp.x && b.y == exp.y) {
                    self.bombs[pos].placed_at = now - Duration::from_millis(self.bomb_fuse_ms + 1);
                }
            }

            self.explosions.extend(new_explosions);
        }
        if !bombs_to_explode.is_empty() {
            self.sounds.push(Sound::Explosion);
        }

        // 2) expire explosions — the previous predicate was always false
        //    and killed every explosion the tick it was created.
        self.explosions.retain(|e| e.expires_at > now);

        // 3) enemies caught in fire die and award points
        let mut killed = 0u32;
        for enemy in &mut self.enemies {
            if !enemy.alive {
                continue;
            }
            if self
                .explosions
                .iter()
                .any(|e| e.x == enemy.x && e.y == enemy.y)
            {
                enemy.alive = false;
                killed += 1;
            }
        }
        self.score += killed * ENEMY_SCORE;
        if killed > 0 {
            self.sounds.push(Sound::Kill);
        }

        // 4) move the survivors
        for i in 0..self.enemies.len() {
            if !self.enemies[i].alive {
                continue;
            }
            if now.duration_since(self.enemies[i].last_move) >= Duration::from_millis(ENEMY_MOVE_MS)
            {
                self.move_enemy(i);
                self.enemies[i].last_move = now;
            }
        }

        // 5) player death checks (skip while invincible after reincarnation)
        let player_died = self
            .explosions
            .iter()
            .any(|e| e.x == self.player.0 && e.y == self.player.1)
            || self
                .enemies
                .iter()
                .any(|e| e.alive && e.x == self.player.0 && e.y == self.player.1);
        if player_died && !self.is_invincible() {
            if self.lives > 1 {
                self.lives -= 1;
                self.respawn();
            } else {
                self.game_over = true;
            }
            self.sounds.push(Sound::Death);
        }
    }

    fn try_move_to(&self, x: usize, y: usize, dx: isize, dy: isize) -> Option<(usize, usize)> {
        let nx = x as isize + dx;
        let ny = y as isize + dy;
        if nx < 0 || ny < 0 || nx >= self.width as isize || ny >= self.height as isize {
            return None;
        }
        let nx = nx as usize;
        let ny = ny as usize;
        if matches!(
            self.grid[ny][nx],
            Cell::Solid | Cell::Brick | Cell::Powerup(_)
        ) {
            return None;
        }
        if self.bombs.iter().any(|b| b.x == nx && b.y == ny) {
            return None;
        }
        Some((nx, ny))
    }

    fn move_enemy(&mut self, idx: usize) {
        let (ex, ey, edx, edy) = {
            let e = &self.enemies[idx];
            (e.x, e.y, e.dx, e.dy)
        };

        if let Some((nx, ny)) = self.try_move_to(ex, ey, edx, edy) {
            self.enemies[idx].x = nx;
            self.enemies[idx].y = ny;
            return;
        }

        // try the other three directions, in a random order
        let dirs = [(1isize, 0isize), (-1, 0), (0, 1), (0, -1)];
        let start = rand::thread_rng().gen_range(0..4);
        for i in 0..4 {
            let (dx, dy) = dirs[(start + i) % 4];
            if let Some((nx, ny)) = self.try_move_to(ex, ey, dx, dy) {
                self.enemies[idx].x = nx;
                self.enemies[idx].y = ny;
                self.enemies[idx].dx = dx;
                self.enemies[idx].dy = dy;
                return;
            }
        }
        // completely boxed in: stay put
    }

    // -----------------------------------------------------------------------
    // Explosion propagation
    // -----------------------------------------------------------------------

    fn create_explosion(&mut self, x: usize, y: usize, range: usize, out: &mut Vec<Explosion>) {
        let expires = Instant::now() + Duration::from_millis(EXPLOSION_LIFETIME_MS);
        out.push(Explosion {
            x,
            y,
            expires_at: expires,
        });

        for (dx, dy) in [(1isize, 0isize), (-1, 0), (0, 1), (0, -1)] {
            for dist in 1..=range {
                let nx = x as isize + dx * dist as isize;
                let ny = y as isize + dy * dist as isize;
                if nx < 0 || ny < 0 || nx >= self.width as isize || ny >= self.height as isize {
                    break;
                }
                let nx = nx as usize;
                let ny = ny as usize;

                match self.grid[ny][nx] {
                    Cell::Solid => break,
                    Cell::Brick => {
                        if (nx, ny) == self.door_pos {
                            // reveal the door instead of clearing the cell
                            self.grid[ny][nx] = Cell::Door;
                        } else if Some((nx, ny)) == self.powerup_brick {
                            // reveal the hidden powerup
                            self.grid[ny][nx] = Cell::Powerup(self.powerup_kind);
                            self.powerup_brick = None;
                        } else {
                            self.grid[ny][nx] = Cell::Empty;
                            self.score += BRICK_SCORE;
                        }
                        out.push(Explosion {
                            x: nx,
                            y: ny,
                            expires_at: expires,
                        });
                        break;
                    }
                    Cell::Door => {
                        // fire passes through, door survives
                        out.push(Explosion {
                            x: nx,
                            y: ny,
                            expires_at: expires,
                        });
                    }
                    Cell::Powerup(_) => {
                        // fire passes through, powerup survives
                        out.push(Explosion {
                            x: nx,
                            y: ny,
                            expires_at: expires,
                        });
                    }
                    Cell::Empty => {
                        out.push(Explosion {
                            x: nx,
                            y: ny,
                            expires_at: expires,
                        });
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Rendering
    // -----------------------------------------------------------------------

    fn render(&mut self, frame: &mut Frame) {
        self.frame = self.frame.wrapping_add(1);
        let area = frame.area();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),                  // status
                Constraint::Length(self.height as u16), // field
                Constraint::Min(0),                     // breathing room
            ])
            .split(area);

        // --- status line ---------------------------------------------------
        let alive_enemies = self.enemies.iter().filter(|e| e.alive).count();
        let bombs_ready = self.max_bombs.saturating_sub(self.bombs.len());
        let detonator = if self.has_detonator { "  |  🧨" } else { "" };
        let status = format!(
            " Level {}  |  Score {}  |  💖 {}  |  💣 {}/{} (r{})  |  👾 {}{}",
            self.level,
            self.score,
            self.lives,
            bombs_ready,
            self.max_bombs,
            self.bomb_range,
            alive_enemies,
            detonator,
        );
        let status_par = Paragraph::new(status).style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
        frame.render_widget(status_par, chunks[0]);

        // --- field ---------------------------------------------------------
        let mut lines = Vec::with_capacity(self.height);
        for y in 0..self.height {
            let mut line = String::with_capacity(self.width * 2);
            for x in 0..self.width {
                // Overlay priority: fire > bomb > player > enemy > terrain.
                // Every glyph is exactly two terminal columns, so the grid
                // stays perfectly aligned.
                if self.explosions.iter().any(|e| e.x == x && e.y == y) {
                    line.push_str("🔥");
                } else if self.bombs.iter().any(|b| b.x == x && b.y == y) {
                    line.push_str("💣");
                } else if self.player.0 == x
                    && self.player.1 == y
                    && !(self.is_invincible() && (self.frame % 2 == 1))
                {
                    line.push_str("🤖");
                } else if self.enemies.iter().any(|e| e.alive && e.x == x && e.y == y) {
                    line.push_str("👾");
                } else {
                    line.push_str(match self.grid[y][x] {
                        Cell::Empty => "  ",
                        Cell::Solid => "⬜",
                        Cell::Brick => "🧱",
                        Cell::Door => "🚪",
                        Cell::Powerup(PowerupKind::BombUp) => {
                            if self.frame % 2 == 1 {
                                "  "
                            } else {
                                "💣"
                            }
                        }
                        Cell::Powerup(PowerupKind::FireUp) => {
                            if self.frame % 2 == 1 {
                                "  "
                            } else {
                                "🔥"
                            }
                        }
                        Cell::Powerup(PowerupKind::LifeUp) => {
                            if self.frame % 2 == 1 {
                                "  "
                            } else {
                                "❤️"
                            }
                        }
                        Cell::Powerup(PowerupKind::Detonator) => {
                            if self.frame % 2 == 1 {
                                "  "
                            } else {
                                "🧨"
                            }
                        }
                    });
                }
            }
            lines.push(line);
        }

        let field =
            Paragraph::new(Text::from(lines.join("\n"))).style(Style::default().fg(Color::White));
        frame.render_widget(field, chunks[1]);

        // --- game over banner ---------------------------------------------
        if self.game_over {
            let banner = Paragraph::new("💀 GAME OVER — R to restart, Q to quit")
                .style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
                .alignment(Alignment::Center);
            frame.render_widget(banner, centered_rect(area, 60, 1));
        }
    }
}

fn centered_rect(area: Rect, percent_x: u16, height: u16) -> Rect {
    let w = area.width * percent_x / 100;
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height - height) / 2;
    Rect {
        x,
        y,
        width: w,
        height,
    }
}

// ---------------------------------------------------------------------------
// Audio
// ---------------------------------------------------------------------------

const SR: u32 = 44100;

fn env_decay(i: usize, n: usize) -> f32 {
    if n == 0 {
        return 0.0;
    }
    let t = i as f32 / n as f32;
    (1.0 - t).powf(2.0)
}

fn square_wave(freq: f32, dur: f32, amp: f32) -> Vec<f32> {
    let n = (dur * SR as f32) as usize;
    let mut out = vec![0.0; n];
    let mut phase = 0.0f32;
    let inc = freq / SR as f32;
    for i in 0..n {
        let e = env_decay(i, n);
        let s = if phase < 0.5 { 1.0 } else { -1.0 };
        out[i] = s * amp * e;
        phase = (phase + inc).fract();
    }
    out
}

fn triangle_wave(freq: f32, dur: f32, amp: f32) -> Vec<f32> {
    let n = (dur * SR as f32) as usize;
    let mut out = vec![0.0; n];
    let mut phase = 0.0f32;
    let inc = freq / SR as f32;
    for i in 0..n {
        let e = env_decay(i, n);
        let s = 4.0 * (phase - 0.5).abs() - 1.0; // triangle -1..1
        out[i] = s * amp * e;
        phase = (phase + inc).fract();
    }
    out
}

fn noise_burst(dur: f32, amp: f32) -> Vec<f32> {
    let n = (dur * SR as f32) as usize;
    let mut out = vec![0.0; n];
    let mut seed: u32 = 0x12345678;
    for i in 0..n {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let r = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
        let e = env_decay(i, n);
        out[i] = r * amp * e;
    }
    out
}

fn mix(a: &[f32], b: &[f32]) -> Vec<f32> {
    let n = a.len().max(b.len());
    let mut out = vec![0.0; n];
    for (i, v) in a.iter().enumerate() {
        out[i] += v;
    }
    for (i, v) in b.iter().enumerate() {
        out[i] += v;
    }
    // clamp
    for v in out.iter_mut() {
        *v = v.clamp(-0.95, 0.95);
    }
    out
}

fn midi_freq(n: u8) -> f32 {
    if n == 0 {
        return 0.0;
    }
    440.0 * 2.0f32.powf((n as f32 - 69.0) / 12.0)
}

fn render_melody(notes: &[(u8, f32)], bpm: f32, wave_amp: f32, triangle: bool) -> Vec<f32> {
    let spb = SR as f32 * 60.0 / bpm;
    // total beats
    let total_beats: f32 = notes.iter().map(|n| n.1).sum();
    let n = (total_beats * spb) as usize;
    let mut out = vec![0.0f32; n];
    let mut pos = 0.0f32; // in samples (float)
    for (note, beats) in notes {
        let freq = midi_freq(*note);
        let note_len = (beats * spb) as usize;
        let start = pos as usize;
        if freq > 0.0 && note_len > 0 {
            let mut phase = 0.0f32;
            let inc = freq / SR as f32;
            for j in 0..note_len {
                let i = start + j;
                if i >= out.len() {
                    break;
                }
                let e = env_decay(j, note_len);
                let s = if triangle {
                    4.0 * ((j as f32 * inc).fract() - 0.5).abs() - 1.0
                } else {
                    if (j as f32 * inc).fract() < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                };
                out[i] = s * wave_amp * e;
                let _ = phase;
            }
        }
        pos += note_len as f32;
    }
    out
}

fn generate_music() -> Vec<f32> {
    // Lead (square): C major-ish bounce
    let lead_notes: &[(u8, f32)] = &[
        (60, 1.0),
        (64, 1.0),
        (67, 1.0),
        (67, 1.0),
        (64, 1.0),
        (60, 1.0),
        (67, 2.0),
        (69, 1.0),
        (67, 1.0),
        (64, 1.0),
        (60, 1.0),
        (64, 2.0),
        (60, 2.0),
    ];
    // Bass (triangle): simple roots
    let bass_notes: &[(u8, f32)] = &[
        (48, 2.0),
        (55, 2.0),
        (48, 2.0),
        (55, 2.0),
        (48, 2.0),
        (55, 2.0),
        (48, 2.0),
        (55, 2.0),
        (48, 2.0),
        (55, 2.0),
        (48, 2.0),
        (55, 2.0),
    ];
    let lead = render_melody(lead_notes, 140.0, 0.18, false);
    let bass = render_melody(bass_notes, 140.0, 0.12, true);
    mix(&lead, &bass)
}

fn explosion_sfx() -> Vec<f32> {
    // noise burst + low square sweep
    let n1 = noise_burst(0.45, 0.5);
    let mut sweep = square_wave(110.0, 0.45, 0.25);
    // lower pitch tail by re-render? approximate via decaying freq not needed
    // add downward sweep: scale amplitude with decreasing freq
    for i in 0..sweep.len() {
        let t = i as f32 / sweep.len() as f32;
        sweep[i] *= 1.0 - t * 0.4;
    }
    mix(&n1, &sweep)
}

fn sfx_buffer(sound: Sound) -> Vec<f32> {
    match sound {
        Sound::Bomb => square_wave(330.0, 0.08, 0.25),
        Sound::Explosion => explosion_sfx(),
        Sound::Powerup => {
            let a = square_wave(523.25, 0.08, 0.25);
            let b = square_wave(659.25, 0.08, 0.25);
            let c = square_wave(783.99, 0.08, 0.25);
            let d = square_wave(1046.5, 0.12, 0.25);
            [a, b, c, d].concat()
        }
        Sound::LevelUp => {
            let notes = [523.25f32, 659.25, 783.99, 1046.5];
            let mut out = Vec::new();
            for f in notes {
                out.extend(square_wave(f, 0.12, 0.25));
            }
            out
        }
        Sound::Death => {
            let a = square_wave(392.0, 0.15, 0.25);
            let b = square_wave(330.0, 0.15, 0.25);
            let c = square_wave(261.63, 0.3, 0.25);
            [a, b, c].concat()
        }
        Sound::Kill => square_wave(880.0, 0.06, 0.2),
    }
}

fn write_wav(path: &str, samples: &[f32]) {
    let spec = hound_spec();
    let mut file = std::fs::File::create(path).unwrap();
    // RIFF header
    let data_len = (samples.len() * 2) as u32;
    let file_len = 36 + data_len;
    file.write_all(b"RIFF").unwrap();
    file.write_all(&file_len.to_le_bytes()).unwrap();
    file.write_all(b"WAVE").unwrap();
    file.write_all(b"fmt ").unwrap();
    file.write_all(&16u32.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap(); // PCM
    file.write_all(&1u16.to_le_bytes()).unwrap(); // mono
    file.write_all(&SR.to_le_bytes()).unwrap();
    let byte_rate = SR * 2;
    file.write_all(&byte_rate.to_le_bytes()).unwrap();
    file.write_all(&2u16.to_le_bytes()).unwrap(); // block align
    file.write_all(&16u16.to_le_bytes()).unwrap(); // bits per sample
    file.write_all(b"data").unwrap();
    file.write_all(&data_len.to_le_bytes()).unwrap();
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        file.write_all(&v.to_le_bytes()).unwrap();
    }
    let _ = spec;
}

fn hound_spec() {} // placeholder

fn player_cmd() -> &'static str {
    // macOS
    if std::path::Path::new("/usr/bin/afplay").exists() {
        "afplay"
    } else if std::path::Path::new("/usr/bin/aplay").exists() {
        "aplay"
    } else {
        ""
    }
}

struct Audio {
    dir: std::path::PathBuf,
    sfx: HashMap<Sound, String>,
    stop: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
    music_thread: Option<std::thread::JoinHandle<()>>,
}

impl Audio {
    fn new() -> Option<Self> {
        let cmd = player_cmd();
        if cmd.is_empty() {
            return None;
        }

        let dir = std::env::temp_dir().join("bomberhuman_audio");
        let _ = std::fs::create_dir_all(&dir);

        let mut sfx = HashMap::new();
        for s in [
            Sound::Bomb,
            Sound::Explosion,
            Sound::Powerup,
            Sound::LevelUp,
            Sound::Death,
            Sound::Kill,
        ] {
            let path = dir
                .join(format!("{:?}.wav", s))
                .to_string_lossy()
                .to_string();
            write_wav(&path, &sfx_buffer(s));
            sfx.insert(s, path);
        }

        let music_path = dir.join("music.wav").to_string_lossy().to_string();
        write_wav(&music_path, &generate_music());

        let stop = Arc::new(AtomicBool::new(false));
        let muted = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();
        let muted_clone = muted.clone();
        let music_path_clone = music_path.clone();
        let cmd_str = cmd.to_string();
        let music_thread = std::thread::spawn(move || {
            while !stop_clone.load(Ordering::Relaxed) {
                if muted_clone.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                // spawn (not .output()) so muting can interrupt playback
                let mut child = match Command::new(&cmd_str).arg(&music_path_clone).spawn() {
                    Ok(c) => c,
                    Err(_) => {
                        std::thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                };
                while !stop_clone.load(Ordering::Relaxed) {
                    if muted_clone.load(Ordering::Relaxed) {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                    match child.try_wait() {
                        Ok(Some(_)) => break,
                        Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                        Err(_) => break,
                    }
                }
                if stop_clone.load(Ordering::Relaxed) {
                    let _ = child.kill();
                }
                let _ = child.wait();
            }
        });

        Some(Audio {
            dir,
            sfx,
            stop,
            muted,
            music_thread: Some(music_thread),
        })
    }

    fn play(&self, sound: Sound) {
        if self.muted.load(Ordering::Relaxed) {
            return;
        }
        let cmd = player_cmd();
        if cmd.is_empty() {
            return;
        }
        if let Some(path) = self.sfx.get(&sound) {
            let path = path.clone();
            let cmd = cmd.to_string();
            std::thread::spawn(move || {
                let _ = Command::new(&cmd).arg(&path).output();
            });
        }
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.music_thread.take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ---------------------------------------------------------------------------
// Main loop
// ---------------------------------------------------------------------------

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // --- CLI args ---------------------------------------------------------
    let mut gamepad_index: usize = 0;
    let mut no_audio = false;
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--gamepad" {
            if i + 1 < args.len() {
                gamepad_index = args[i + 1].parse().unwrap_or(0);
                i += 2;
                continue;
            }
        } else if let Some(val) = args[i].strip_prefix("--gamepad=") {
            gamepad_index = val.parse().unwrap_or(0);
        } else if args[i] == "--no-audio" {
            no_audio = true;
        }
        i += 1;
    }

    // --- gamepad init -----------------------------------------------------
    let mut gilrs = Gilrs::new().ok();
    let gamepad_id = if let Some(g) = gilrs.as_mut() {
        // wait briefly for a gamepad to show up
        let mut connected: Vec<gilrs::GamepadId> = Vec::new();
        for _ in 0..50 {
            while let Some(ev) = g.next_event() {
                let _ = ev;
            }
            connected = g.gamepads().map(|(id, _)| id).collect();
            if !connected.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        connected.get(gamepad_index).copied()
    } else {
        None
    };

    // user-driven button mapping (only when a gamepad is available)
    #[derive(Clone, Copy)]
    struct Mapping {
        up: Button,
        down: Button,
        left: Button,
        right: Button,
        bomb: Button,
        detonate: Button,
    }

    let mapping: Option<Mapping> = if let (Some(g), Some(id)) = (gilrs.as_mut(), gamepad_id) {
        let mut map_one = |name: &str| -> Button {
            println!("Please press {} on your gamepad...", name);
            let _ = io::stdout().flush();
            loop {
                while let Some(ev) = g.next_event() {
                    if ev.id == id {
                        if let EventType::ButtonPressed(btn, _) = ev.event {
                            println!("  mapped to {:?}", btn);
                            let _ = io::stdout().flush();
                            return btn;
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        Some(Mapping {
            up: map_one("UP"),
            down: map_one("DOWN"),
            left: map_one("LEFT"),
            right: map_one("RIGHT"),
            bomb: map_one("BOMB"),
            detonate: map_one("DETONATE"),
        })
    } else {
        if gamepad_index > 0 || gilrs.is_some() {
            println!(
                "No gamepad found at index {}; using keyboard only.",
                gamepad_index
            );
        }
        None
    };

    // --- audio setup ------------------------------------------------------
    let (audio, audio_muted): (Option<Audio>, Option<Arc<AtomicBool>>) = if no_audio {
        (None, None)
    } else {
        match Audio::new() {
            Some(a) => {
                let m = a.muted.clone();
                (Some(a), Some(m))
            }
            None => (None, None),
        }
    };

    // --- terminal setup ---------------------------------------------------
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut game = Game::new(15, 15);

    let tick_rate = Duration::from_millis(50); // game logic step
    let render_rate = Duration::from_millis(33); // ~30 FPS draw
    let mut last_tick = Instant::now();
    let move_cooldown = Duration::from_millis(150);
    let mut last_move = Instant::now();
    let mut prev_bomb = false;
    let mut prev_detonate = false;

    loop {
        terminal.draw(|f| game.render(f))?;

        // keyboard + frame pacing
        if event::poll(render_rate)? {
            while event::poll(Duration::from_millis(0)).unwrap_or(false) {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        match key.code {
                            KeyCode::Char('q') => {
                                drop(audio);
                                disable_raw_mode()?;
                                execute!(
                                    terminal.backend_mut(),
                                    LeaveAlternateScreen,
                                    DisableMouseCapture
                                )?;
                                terminal.show_cursor()?;
                                return Ok(());
                            }
                            KeyCode::Char('m') => {
                                if let Some(muted) = &audio_muted {
                                    let cur = muted.load(Ordering::Relaxed);
                                    muted.store(!cur, Ordering::Relaxed);
                                }
                            }
                            KeyCode::Char('r') if game.game_over => {
                                game = Game::new(15, 15);
                            }
                            KeyCode::Up | KeyCode::Char('w') => game.move_player(0, -1),
                            KeyCode::Down | KeyCode::Char('s') => game.move_player(0, 1),
                            KeyCode::Left | KeyCode::Char('a') => game.move_player(-1, 0),
                            KeyCode::Right | KeyCode::Char('d') => game.move_player(1, 0),
                            KeyCode::Char(' ') => game.place_bomb(),
                            KeyCode::Modifier(event::ModifierKeyCode::LeftShift)
                            | KeyCode::Modifier(event::ModifierKeyCode::RightShift) => {
                                if game.detonate() {
                                    game.update();
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        // gamepad
        if let (Some(g), Some(id), Some(m)) = (gilrs.as_mut(), gamepad_id, mapping.as_ref()) {
            while let Some(_) = g.next_event() {}
            let gp = g.gamepad(id);
            {
                let held_up = gp.is_pressed(m.up);
                let held_down = gp.is_pressed(m.down);
                let held_left = gp.is_pressed(m.left);
                let held_right = gp.is_pressed(m.right);
                let held_bomb = gp.is_pressed(m.bomb);
                let held_detonate = gp.is_pressed(m.detonate);

                if !game.game_over && last_move.elapsed() >= move_cooldown {
                    let mut moved = false;
                    if held_up {
                        game.move_player(0, -1);
                        moved = true;
                    } else if held_down {
                        game.move_player(0, 1);
                        moved = true;
                    } else if held_left {
                        game.move_player(-1, 0);
                        moved = true;
                    } else if held_right {
                        game.move_player(1, 0);
                        moved = true;
                    }
                    if moved {
                        last_move = Instant::now();
                    }
                }

                if held_bomb && !prev_bomb && !game.game_over {
                    game.place_bomb();
                }
                prev_bomb = held_bomb;

                if held_detonate && !prev_detonate && !game.game_over {
                    if game.detonate() {
                        game.update();
                    }
                }
                prev_detonate = held_detonate;

                // restart on gameover via bomb+up? keep keyboard r
            }
        }

        if last_tick.elapsed() >= tick_rate {
            game.update();
            last_tick = Instant::now();
        }

        if let Some(a) = &audio {
            for s in game.drain_sounds() {
                a.play(s);
            }
        }
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}
