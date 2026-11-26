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
const CRITTER_MOVE_MS: u64 = 400;
const ZOMBIE_MOVE_MS: u64 = 600;
const GHOST_MOVE_MS: u64 = 300;
const WIN_LEVEL: u32 = 50;

const BRICK_SCORE: u32 = 10;
const ENEMY_SCORE: u32 = 100;
const LEVEL_SCORE: u32 = 500;
const POWERUP_SCORE: u32 = 50;
const TIME_BONUS_START: u32 = 300;
const TIME_BONUS_MULTIPLIER: u32 = 5;

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum EnemyKind {
    Critter,
    Zombie,
    Ghost,
}

impl EnemyKind {
    fn move_interval_ms(self) -> u64 {
        match self {
            EnemyKind::Critter => CRITTER_MOVE_MS,
            EnemyKind::Zombie => ZOMBIE_MOVE_MS,
            EnemyKind::Ghost => GHOST_MOVE_MS,
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            EnemyKind::Critter => "👾",
            EnemyKind::Zombie => "🧌",
            EnemyKind::Ghost => "👻",
        }
    }
}

struct Bomb {
    x: usize,
    y: usize,
    placed_at: Instant,
    range: usize,
    detonated: bool,
}

struct Explosion {
    x: usize,
    y: usize,
    expires_at: Instant,
}

struct Enemy {
    kind: EnemyKind,
    x: usize,
    y: usize,
    dx: isize,
    dy: isize,
    last_move: Instant,
    alive: bool,
}

impl Enemy {
    fn new(kind: EnemyKind, x: usize, y: usize, now: Instant) -> Self {
        let dirs = [(1isize, 0isize), (-1, 0), (0, 1), (0, -1)];
        let (dx, dy) = dirs[rand::thread_rng().gen_range(0..4)];
        Enemy {
            kind,
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
    time_bonus: u32,
    last_timer_tick: Instant,
    level: u32,
    lives: u32,
    max_bombs: usize,
    bomb_range: usize,
    bomb_fuse_ms: u64,
    has_detonator: bool,
    game_over: bool,
    won: bool,
    paused: bool,
    paused_at: Option<Instant>,
    pending_respawn_at: Option<Instant>,
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
            time_bonus: TIME_BONUS_START,
            last_timer_tick: Instant::now(),
            level: 1,
            lives: STARTING_LIVES,
            max_bombs: 1,
            bomb_range: 1,
            bomb_fuse_ms: BOMB_FUSE_MS,
            has_detonator: false,
            game_over: false,
            won: false,
            paused: false,
            paused_at: None,
            pending_respawn_at: None,
            invincible_until: None,
            frame: 0,
            sounds: Vec::new(),
        };
        game.generate_level();
        game
    }

    // -----------------------------------------------------------------------
    // Level generation & monster ratios
    // -----------------------------------------------------------------------

    fn brick_fill_ratio(&self) -> f64 {
        // Ramps from 10% (0.10) at level 1 to 90% (0.90) by level 50.
        let t = (self.level.saturating_sub(1).min(WIN_LEVEL - 1) as f64)
            / ((WIN_LEVEL - 1).max(1) as f64);
        0.10 + 0.80 * t
    }



    // -----------------------------------------------------------------------
    // Level generation & monster ratios
    // -----------------------------------------------------------------------

    fn level_progress(&self) -> f64 {
        (self.level.saturating_sub(1).min(WIN_LEVEL - 1) as f64)
            / ((WIN_LEVEL - 1).max(1) as f64)
    }

    fn filler_envelope_ratio(&self) -> f64 {
        // Total filler (bricks + enemies) ramps from 5% (0.05) to 95% (0.95).
        0.05 + 0.90 * self.level_progress()
    }

    fn enemy_filler_share(&self) -> f64 {
        // Enemies' share of the filler budget ramps from 20% (0.20) down to 5% (0.05).
        0.20 - 0.15 * self.level_progress()
    }

    fn brick_filler_share(&self) -> f64 {
        // Bricks' share of the filler budget ramps from 80% (0.80) up to 95% (0.95).
        1.0 - self.enemy_filler_share()
    }

    fn monster_roster(&self, total: usize) -> Vec<EnemyKind> {
        // Level 1..=25:  Critter 100% -> 50%, Zombie 0% -> 50%, Ghost 0%
        // Level 26..=50: Critter 50%  -> 10%, Zombie 50% -> 30%, Ghost 0% -> 60%
        let (critter_ratio, zombie_ratio, ghost_ratio) = if self.level <= 25 {
            let p = (self.level.saturating_sub(1) as f64) / 24.0;
            (1.0 - 0.50 * p, 0.50 * p, 0.0)
        } else {
            let p = ((self.level - 25).min(25) as f64) / 25.0;
            (0.50 - 0.40 * p, 0.50 - 0.20 * p, 0.60 * p)
        };

        let mut roster = Vec::with_capacity(total);
        for idx in 0..total {
            let frac = (idx as f64 + 0.5) / (total.max(1) as f64);
            if frac < critter_ratio {
                roster.push(EnemyKind::Critter);
            } else if frac < critter_ratio + zombie_ratio {
                roster.push(EnemyKind::Zombie);
            } else if ghost_ratio > 0.0 {
                roster.push(EnemyKind::Ghost);
            } else {
                roster.push(EnemyKind::Zombie);
            }
        }
        roster
    }

    fn generate_level(&mut self) {
        use rand::seq::SliceRandom;

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

        // pillars + collect candidate filler tiles (excluding border, pillars, and start pocket)
        let mut filler_candidates = Vec::new();
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                if x % 2 == 0 && y % 2 == 0 {
                    grid[y][x] = Cell::Solid;
                } else if (x == 1 && y == 1)
                    || (x == 2 && y == 1 && w > 2)
                    || (x == 1 && y == 2 && h > 2)
                {
                    grid[y][x] = Cell::Empty;
                } else {
                    filler_candidates.push((x, y));
                }
            }
        }

        let mut rng = rand::thread_rng();
        filler_candidates.shuffle(&mut rng);

        let total_filler = ((filler_candidates.len() as f64) * self.filler_envelope_ratio())
            .round()
            .clamp(1.0, filler_candidates.len() as f64) as usize;

        let num_enemies = ((total_filler as f64) * self.enemy_filler_share())
            .round()
            .clamp(1.0, total_filler as f64) as usize;
        let num_bricks = total_filler.saturating_sub(num_enemies);

        let mut brick_cells = Vec::with_capacity(num_bricks);
        for &(x, y) in filler_candidates.iter().take(num_bricks) {
            grid[y][x] = Cell::Brick;
            brick_cells.push((x, y));
        }

        // hide the door under a brick, far from start if possible
        let far_candidates: Vec<(usize, usize)> = brick_cells
            .iter()
            .copied()
            .filter(|&(x, y)| (x + y) > 6)
            .collect();
        let door_pos = if !far_candidates.is_empty() {
            far_candidates[rng.gen_range(0..far_candidates.len())]
        } else if !brick_cells.is_empty() {
            brick_cells[rng.gen_range(0..brick_cells.len())]
        } else {
            (w - 3, h - 3)
        };

        if grid[door_pos.1][door_pos.0] != Cell::Brick {
            grid[door_pos.1][door_pos.0] = Cell::Door;
        }

        // pick one brick (distinct from the door) to hide a powerup under
        let powerup_candidates: Vec<(usize, usize)> = brick_cells
            .iter()
            .copied()
            .filter(|&p| p != door_pos)
            .collect();
        let powerup_brick = if !powerup_candidates.is_empty() {
            Some(powerup_candidates[rng.gen_range(0..powerup_candidates.len())])
        } else {
            None
        };

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
        let powerup_kind = kinds[rng.gen_range(0..kinds.len())];

        // spawn enemies on empty tiles (preferring manhattan distance >= 5 from (1, 1))
        let now = Instant::now();
        let roster = self.monster_roster(num_enemies);
        let mut far_empty: Vec<(usize, usize)> = Vec::new();
        let mut fallback_empty: Vec<(usize, usize)> = Vec::new();
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                if grid[y][x] == Cell::Empty && !(x == 1 && y == 1) {
                    let manhattan = (x as isize - 1).abs() + (y as isize - 1).abs();
                    if manhattan >= 5 {
                        far_empty.push((x, y));
                    } else {
                        fallback_empty.push((x, y));
                    }
                }
            }
        }
        far_empty.shuffle(&mut rng);
        fallback_empty.shuffle(&mut rng);
        far_empty.extend(fallback_empty);

        let mut enemies = Vec::with_capacity(num_enemies.min(far_empty.len()));
        for (i, &(x, y)) in far_empty.iter().take(num_enemies).enumerate() {
            enemies.push(Enemy::new(roster[i], x, y, now));
        }

        self.grid = grid;
        self.player = (1, 1);
        self.bombs.clear();
        self.explosions.clear();
        self.enemies = enemies;
        self.door_pos = door_pos;
        self.powerup_brick = powerup_brick;
        self.powerup_kind = powerup_kind;
        self.time_bonus = TIME_BONUS_START;
        self.last_timer_tick = now;
    }

    fn next_level(&mut self) {
        self.score += LEVEL_SCORE + self.time_bonus * TIME_BONUS_MULTIPLIER;
        self.sounds.push(Sound::LevelUp);
        if self.level >= WIN_LEVEL {
            self.won = true;
            return;
        }
        self.level += 1;
        self.lives += 1;
        self.generate_level();
    }

    fn is_frozen(&self) -> bool {
        self.game_over || self.won || self.paused
    }

    // -----------------------------------------------------------------------
    // Player actions
    // -----------------------------------------------------------------------

    fn move_player(&mut self, dx: isize, dy: isize) {
        if self.is_frozen() {
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
        if self.explosions.iter().any(|e| e.x == nx && e.y == ny) {
            return;
        }

        self.player = (nx, ny);

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

        if self.grid[ny][nx] == Cell::Door
            && self.enemies.iter().all(|e| !e.alive)
        {
            self.next_level();
        }
    }

    fn place_bomb(&mut self) {
        if self.is_frozen() {
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
            detonated: false,
        });
        self.sounds.push(Sound::Bomb);
    }

    fn respawn(&mut self) {
        self.player = (1, 1);
        self.bombs.clear();
        self.explosions.clear();
        self.pending_respawn_at = None;
        self.invincible_until = Some(Instant::now() + Duration::from_secs(1));
    }

    fn detonate(&mut self) -> bool {
        if self.is_frozen() || !self.has_detonator || self.bombs.is_empty() {
            return false;
        }
        let now = Instant::now();
        for b in &mut self.bombs {
            b.placed_at = now - Duration::from_millis(self.bomb_fuse_ms + 1);
            b.detonated = true;
        }
        self.sounds.push(Sound::Explosion);
        true
    }

    fn is_invincible(&self) -> bool {
        self.invincible_until.map_or(false, |t| Instant::now() < t)
    }

    fn toggle_pause(&mut self) {
        if self.game_over || self.won {
            return;
        }
        let now = Instant::now();
        if !self.paused {
            self.paused = true;
            self.paused_at = Some(now);
        } else {
            if let Some(paused_since) = self.paused_at.take() {
                let paused_dur = now.saturating_duration_since(paused_since);
                for bomb in &mut self.bombs {
                    bomb.placed_at += paused_dur;
                }
                if let Some(until) = self.invincible_until.as_mut() {
                    *until += paused_dur;
                }
                if let Some(respawn_at) = self.pending_respawn_at.as_mut() {
                    *respawn_at += paused_dur;
                }
                self.last_timer_tick += paused_dur;
            }
            self.paused = false;
        }
    }

    pub fn drain_sounds(&mut self) -> Vec<Sound> {
        std::mem::take(&mut self.sounds)
    }

    // -----------------------------------------------------------------------
    // Tick
    // -----------------------------------------------------------------------

    fn update(&mut self) {
        if self.is_frozen() {
            return;
        }
        let now = Instant::now();

        // 0) decrement time bonus counter once per second (floored at 0)
        let elapsed_secs = now.duration_since(self.last_timer_tick).as_secs();
        if elapsed_secs > 0 {
            self.time_bonus = self.time_bonus.saturating_sub(elapsed_secs as u32);
            self.last_timer_tick += Duration::from_secs(elapsed_secs);
        }

        // 1) explode any bombs whose fuse has run out
        let mut bombs_to_explode = Vec::new();
        for (i, bomb) in self.bombs.iter().enumerate() {
            if now.duration_since(bomb.placed_at) >= Duration::from_millis(self.bomb_fuse_ms) {
                bombs_to_explode.push(i);
            }
        }

        let mut play_explosion_sfx = false;
        for i in bombs_to_explode.clone().into_iter().rev() {
            let bomb = self.bombs.remove(i);
            if !bomb.detonated {
                play_explosion_sfx = true;
            }
            let mut new_explosions = Vec::new();
            self.create_explosion(bomb.x, bomb.y, bomb.range, &mut new_explosions);

            for exp in &new_explosions {
                if let Some(pos) = self.bombs.iter().position(|b| b.x == exp.x && b.y == exp.y) {
                    self.bombs[pos].placed_at = now - Duration::from_millis(self.bomb_fuse_ms + 1);
                }
            }

            self.explosions.extend(new_explosions);
        }
        if play_explosion_sfx {
            self.sounds.push(Sound::Explosion);
        }

        // 2) expire explosions
        self.explosions.retain(|e| e.expires_at > now);

        // Complete deferred respawn once the flame animation finishes
        if let Some(respawn_at) = self.pending_respawn_at {
            if now >= respawn_at {
                self.respawn();
            }
        }

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

        // 4) move each enemy on its own cadence
        for i in 0..self.enemies.len() {
            if !self.enemies[i].alive {
                continue;
            }
            let interval = self.enemies[i].kind.move_interval_ms();
            if now.duration_since(self.enemies[i].last_move) >= Duration::from_millis(interval) {
                self.move_enemy(i);
                self.enemies[i].last_move = now;
            }
        }

        // 5) player death checks
        let hit_by_fire = self
            .explosions
            .iter()
            .any(|e| e.x == self.player.0 && e.y == self.player.1);
        let hit_by_enemy = self
            .enemies
            .iter()
            .any(|e| e.alive && e.x == self.player.0 && e.y == self.player.1);

        if (hit_by_fire || hit_by_enemy)
            && !self.is_invincible()
            && self.pending_respawn_at.is_none()
        {
            if self.lives > 1 {
                self.lives -= 1;
                self.sounds.push(Sound::Death);
                if hit_by_fire {
                    let max_exp = self
                        .explosions
                        .iter()
                        .map(|e| e.expires_at)
                        .max()
                        .unwrap_or_else(|| now + Duration::from_millis(EXPLOSION_LIFETIME_MS));
                    self.pending_respawn_at = Some(max_exp);
                } else {
                    self.respawn();
                }
            } else {
                self.game_over = true;
                self.sounds.push(Sound::Death);
            }
        }
    }

    fn try_move_to(
        &self,
        kind: EnemyKind,
        x: usize,
        y: usize,
        dx: isize,
        dy: isize,
    ) -> Option<(usize, usize)> {
        let nx = x as isize + dx;
        let ny = y as isize + dy;
        if nx < 0 || ny < 0 || nx >= self.width as isize || ny >= self.height as isize {
            return None;
        }
        let nx = nx as usize;
        let ny = ny as usize;

        match self.grid[ny][nx] {
            Cell::Solid | Cell::Powerup(_) => return None,
            Cell::Brick if kind != EnemyKind::Ghost => return None,
            _ => {}
        }
        if self.bombs.iter().any(|b| b.x == nx && b.y == ny) {
            return None;
        }
        Some((nx, ny))
    }

    fn valid_moves(&self, kind: EnemyKind, x: usize, y: usize) -> Vec<(isize, isize, usize, usize)> {
        let dirs = [(1isize, 0isize), (-1, 0), (0, 1), (0, -1)];
        dirs.into_iter()
            .filter_map(|(dx, dy)| {
                self.try_move_to(kind, x, y, dx, dy)
                    .map(|(nx, ny)| (dx, dy, nx, ny))
            })
            .collect()
    }

    fn move_enemy(&mut self, idx: usize) {
        let (kind, ex, ey, edx, edy) = {
            let e = &self.enemies[idx];
            (e.kind, e.x, e.y, e.dx, e.dy)
        };

        let options = self.valid_moves(kind, ex, ey);
        if options.is_empty() {
            return;
        }

        let is_intersection = options.len() > 2;

        let chosen = match kind {
            EnemyKind::Critter => {
                if let Some(&(dx, dy, nx, ny)) = options
                    .iter()
                    .find(|&(dx, dy, _, _)| *dx == edx && *dy == edy)
                {
                    (dx, dy, nx, ny)
                } else {
                    options[rand::thread_rng().gen_range(0..options.len())]
                }
            }
            EnemyKind::Zombie => {
                if is_intersection
                    || !options
                        .iter()
                        .any(|&(dx, dy, _, _)| dx == edx && dy == edy)
                {
                    let (px, py) = (self.player.0 as isize, self.player.1 as isize);
                    *options
                        .iter()
                        .min_by_key(|&&(_, _, nx, ny)| {
                            (nx as isize - px).abs() + (ny as isize - py).abs()
                        })
                        .unwrap()
                } else {
                    *options
                        .iter()
                        .find(|&(dx, dy, _, _)| *dx == edx && *dy == edy)
                        .unwrap()
                }
            }
            EnemyKind::Ghost => {
                let current_dir = if options
                    .iter()
                    .any(|&(dx, dy, _, _)| dx == edx && dy == edy)
                {
                    (edx, edy)
                } else {
                    let first = options[0];
                    (first.0, first.1)
                };

                if is_intersection {
                    let roll: f64 = rand::thread_rng().gen();
                    let desired = if roll < 0.50 {
                        current_dir
                    } else if roll < 0.75 {
                        // left turn
                        (-current_dir.1, current_dir.0)
                    } else {
                        // right turn
                        (current_dir.1, -current_dir.0)
                    };

                    options
                        .iter()
                        .copied()
                        .find(|&(dx, dy, _, _)| (dx, dy) == desired)
                        .unwrap_or_else(|| {
                            options[rand::thread_rng().gen_range(0..options.len())]
                        })
                } else if let Some(&same) = options
                    .iter()
                    .find(|&(dx, dy, _, _)| *dx == current_dir.0 && *dy == current_dir.1)
                {
                    same
                } else {
                    options[rand::thread_rng().gen_range(0..options.len())]
                }
            }
        };

        self.enemies[idx].x = chosen.2;
        self.enemies[idx].y = chosen.3;
        self.enemies[idx].dx = chosen.0;
        self.enemies[idx].dy = chosen.1;
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
                            self.grid[ny][nx] = Cell::Door;
                        } else if Some((nx, ny)) == self.powerup_brick {
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
                        out.push(Explosion {
                            x: nx,
                            y: ny,
                            expires_at: expires,
                        });
                    }
                    Cell::Powerup(_) => {
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
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(area);

        let alive_enemies = self.enemies.iter().filter(|e| e.alive).count();
        let bombs_ready = self.max_bombs.saturating_sub(self.bombs.len());
        let detonator = if self.has_detonator { "  |  🧨" } else { "" };
        let status = format!(
            " Level {}/{}  |  Score {}  |  ⏱ {}  |  💖 {}  |  💣 {}/{} (r{})  |  👾 {}{}",
            self.level,
            WIN_LEVEL,
            self.score,
            self.time_bonus,
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

        let mut lines = Vec::with_capacity(self.height);
        for y in 0..self.height {
            let mut line = String::with_capacity(self.width * 2);
            for x in 0..self.width {
                if self.explosions.iter().any(|e| e.x == x && e.y == y) {
                    line.push_str("🔥");
                } else if self.bombs.iter().any(|b| b.x == x && b.y == y) {
                    line.push_str("💣");
                } else if self.player.0 == x
                    && self.player.1 == y
                    && !(self.is_invincible() && (self.frame % 2 == 1))
                {
                    line.push_str("🤖");
                } else if let Some(enemy) = self.enemies.iter().find(|e| e.alive && e.x == x && e.y == y)
                {
                    line.push_str(enemy.kind.glyph());
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

        let board_w = (self.width as u16).saturating_mul(2).min(chunks[1].width);
        let board_h = (self.height as u16).min(chunks[1].height);
        let board_x = chunks[1].x + chunks[1].width.saturating_sub(board_w) / 2;
        let board_y = chunks[1].y + chunks[1].height.saturating_sub(board_h) / 2;
        let board_rect = Rect {
            x: board_x,
            y: board_y,
            width: board_w,
            height: board_h,
        };

        let field =
            Paragraph::new(Text::from(lines.join("\n"))).style(Style::default().fg(Color::White));
        frame.render_widget(field, board_rect);

        if self.won {
            let banner = Paragraph::new("🏆 You win! 🏆 — R to restart, Q to quit")
                .style(
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )
                .alignment(Alignment::Center);
            frame.render_widget(banner, centered_rect(area, 60, 1));
        } else if self.game_over {
            let banner = Paragraph::new("💀 GAME OVER — R to restart, Q to quit")
                .style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
                .alignment(Alignment::Center);
            frame.render_widget(banner, centered_rect(area, 60, 1));
        } else if self.paused {
            let banner = Paragraph::new("⏸  PAUSED — P to resume")
                .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
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
    let total_beats: f32 = notes.iter().map(|n| n.1).sum();
    let n = (total_beats * spb) as usize;
    let mut out = vec![0.0f32; n];
    let mut pos = 0.0f32;
    for (note, beats) in notes {
        let freq = midi_freq(*note);
        let note_len = (beats * spb) as usize;
        let start = pos as usize;
        if freq > 0.0 && note_len > 0 {
            let inc = freq / SR as f32;
            // Slight gate so repeated notes still articulate cleanly
            let gate = (note_len as f32 * 0.88).max(1.0);
            for j in 0..note_len {
                let i = start + j;
                if i >= out.len() {
                    break;
                }
                let e = if (j as f32) < gate {
                    let t = j as f32 / gate;
                    (1.0 - 0.55 * t * t).max(0.0)
                } else {
                    let tail = (j as f32 - gate) / ((note_len as f32 - gate).max(1.0));
                    (0.45 * (1.0 - tail)).max(0.0)
                };
                let s = if triangle {
                    4.0 * ((j as f32 * inc).fract() - 0.5).abs() - 1.0
                } else if (j as f32 * inc).fract() < 0.5 {
                    1.0
                } else {
                    -1.0
                };
                out[i] = s * wave_amp * e;
            }
        }
        pos += note_len as f32;
    }
    out
}

fn generate_music() -> Vec<f32> {
    let bpm = 172.0f32;
    // 240 beats @ 172 BPM ≈ 83.7 seconds (1m24s), right in the 1-2 minute range.
    // MIDI reference:
    // C3=48, D3=50, E3=52, F3=53, G3=55, A3=57, B3=59
    // C4=60, D4=62, E4=64, F4=65, G4=67, A4=69, B4=71, C5=72, D5=74, E5=76

    // --- Section A (8 bars / 32 beats): Brisk arcade theme in C major ---
    let lead_a: &[(u8, f32)] = &[
        // A1: C -> G
        (60, 0.5), (64, 0.5), (67, 0.5), (64, 0.5), (62, 0.5), (64, 0.5), (67, 1.0),
        // A2: F -> G
        (65, 0.5), (69, 0.5), (67, 0.5), (65, 0.5), (64, 0.5), (62, 0.5), (64, 1.0),
        // A3: Am -> F -> G
        (69, 0.5), (72, 0.5), (71, 0.5), (69, 0.5), (65, 0.5), (69, 0.5), (67, 0.5), (64, 0.5),
        // A4: Dm -> G7 -> C
        (62, 0.5), (65, 0.5), (67, 0.5), (71, 0.5), (69, 0.5), (67, 0.5), (60, 1.0),
        // A5: Variation with higher answer
        (60, 0.5), (64, 0.5), (67, 0.5), (72, 0.5), (71, 0.5), (69, 0.5), (67, 1.0),
        // A6: Lift up to E5
        (65, 0.5), (69, 0.5), (72, 0.5), (76, 0.5), (74, 0.5), (72, 0.5), (71, 1.0),
        // A7: Syncopated run
        (67, 0.5), (69, 0.5), (72, 0.5), (69, 0.5), (67, 0.5), (64, 0.5), (60, 0.5), (64, 0.5),
        // A8: Cadence into Section B
        (62, 0.5), (64, 0.5), (67, 0.5), (69, 0.5), (71, 0.5), (72, 1.5),
    ];

    let bass_a: &[(u8, f32)] = &[
        // C
        (48, 0.5), (60, 0.5), (48, 0.5), (55, 0.5), (48, 0.5), (60, 0.5), (55, 0.5), (48, 0.5),
        // F -> G
        (53, 0.5), (60, 0.5), (53, 0.5), (55, 0.5), (55, 0.5), (62, 0.5), (55, 0.5), (50, 0.5),
        // Am -> F
        (57, 0.5), (60, 0.5), (57, 0.5), (52, 0.5), (53, 0.5), (60, 0.5), (53, 0.5), (48, 0.5),
        // Dm -> G -> C
        (50, 0.5), (57, 0.5), (50, 0.5), (55, 0.5), (55, 0.5), (59, 0.5), (48, 1.0),
        // C
        (48, 0.5), (60, 0.5), (48, 0.5), (55, 0.5), (48, 0.5), (60, 0.5), (55, 0.5), (48, 0.5),
        // F -> G
        (53, 0.5), (60, 0.5), (53, 0.5), (55, 0.5), (55, 0.5), (62, 0.5), (55, 0.5), (59, 0.5),
        // Am -> C/G
        (57, 0.5), (60, 0.5), (57, 0.5), (52, 0.5), (55, 0.5), (60, 0.5), (55, 0.5), (48, 0.5),
        // G7 -> C
        (55, 0.5), (59, 0.5), (62, 0.5), (59, 0.5), (55, 0.5), (48, 1.5),
    ];

    // --- Section B (8 bars / 32 beats): Wider leaping motif, less repetitive ---
    let lead_b: &[(u8, f32)] = &[
        // B1: Upper register call
        (72, 0.5), (67, 0.5), (64, 0.5), (67, 0.5), (69, 0.5), (72, 0.5), (74, 1.0),
        // B2: Answer down
        (72, 0.5), (69, 0.5), (67, 0.5), (64, 0.5), (65, 0.5), (67, 0.5), (69, 1.0),
        // B3: Rhythmic staccato pattern
        (67, 0.5), (0, 0.5),  (67, 0.5), (69, 0.5), (72, 0.5), (0, 0.5),  (71, 0.5), (69, 0.5),
        // B4: Roll into bridge
        (67, 0.5), (65, 0.5), (64, 0.5), (62, 0.5), (64, 1.0), (67, 1.0),
        // B5: Second half of B, brighter turn
        (72, 0.5), (74, 0.5), (76, 0.5), (74, 0.5), (72, 0.5), (69, 0.5), (67, 1.0),
        // B6: Descending sequence
        (69, 0.5), (67, 0.5), (65, 0.5), (64, 0.5), (62, 0.5), (64, 0.5), (65, 1.0),
        // B7: Build-up
        (64, 0.5), (67, 0.5), (69, 0.5), (72, 0.5), (74, 0.5), (72, 0.5), (71, 0.5), (69, 0.5),
        // B8: Turnaround
        (67, 0.5), (64, 0.5), (62, 0.5), (64, 0.5), (67, 1.0), (0, 1.0),
    ];

    let bass_b: &[(u8, f32)] = &[
        // C
        (48, 0.5), (55, 0.5), (60, 0.5), (55, 0.5), (48, 0.5), (55, 0.5), (50, 0.5), (55, 0.5),
        // F -> G
        (53, 0.5), (60, 0.5), (53, 0.5), (57, 0.5), (55, 0.5), (62, 0.5), (55, 0.5), (59, 0.5),
        // Am -> F
        (57, 0.5), (64, 0.5), (57, 0.5), (52, 0.5), (53, 0.5), (60, 0.5), (53, 0.5), (57, 0.5),
        // Dm -> G
        (50, 0.5), (57, 0.5), (50, 0.5), (53, 0.5), (55, 0.5), (59, 0.5), (62, 0.5), (55, 0.5),
        // C
        (48, 0.5), (55, 0.5), (60, 0.5), (55, 0.5), (48, 0.5), (55, 0.5), (52, 0.5), (55, 0.5),
        // F -> Dm
        (53, 0.5), (60, 0.5), (53, 0.5), (48, 0.5), (50, 0.5), (57, 0.5), (50, 0.5), (53, 0.5),
        // G -> Em
        (55, 0.5), (62, 0.5), (55, 0.5), (59, 0.5), (52, 0.5), (59, 0.5), (52, 0.5), (55, 0.5),
        // G7
        (55, 0.5), (59, 0.5), (62, 0.5), (65, 0.5), (55, 1.0), (50, 1.0),
    ];

    // --- Section C (8 bars / 32 beats): Shift to A minor / F / C for mood contrast ---
    let lead_c: &[(u8, f32)] = &[
        // C1: Am groove
        (69, 0.5), (64, 0.5), (67, 0.5), (69, 0.5), (72, 0.5), (71, 0.5), (69, 1.0),
        // C2: F major warmth
        (65, 0.5), (69, 0.5), (72, 0.5), (69, 0.5), (67, 0.5), (65, 0.5), (64, 1.0),
        // C3: Dm -> E
        (62, 0.5), (65, 0.5), (69, 0.5), (67, 0.5), (64, 0.5), (68, 0.5), (71, 1.0),
        // C4: Back toward Am
        (69, 0.5), (67, 0.5), (64, 0.5), (60, 0.5), (62, 0.5), (64, 0.5), (67, 1.0),
        // C5: Rising line
        (69, 0.5), (72, 0.5), (74, 0.5), (76, 0.5), (74, 0.5), (72, 0.5), (69, 1.0),
        // C6: Syncopated bounce
        (65, 0.5), (0, 0.5),  (65, 0.5), (69, 0.5), (67, 0.5), (64, 0.5), (62, 1.0),
        // C7: Tension before home key
        (64, 0.5), (67, 0.5), (71, 0.5), (74, 0.5), (72, 0.5), (71, 0.5), (69, 0.5), (67, 0.5),
        // C8: Ramp back into C major
        (64, 0.5), (65, 0.5), (67, 0.5), (69, 0.5), (71, 0.5), (72, 1.5),
    ];

    let bass_c: &[(u8, f32)] = &[
        // Am
        (57, 0.5), (64, 0.5), (57, 0.5), (52, 0.5), (57, 0.5), (64, 0.5), (57, 0.5), (55, 0.5),
        // F
        (53, 0.5), (60, 0.5), (53, 0.5), (48, 0.5), (53, 0.5), (60, 0.5), (53, 0.5), (52, 0.5),
        // Dm -> E
        (50, 0.5), (57, 0.5), (50, 0.5), (53, 0.5), (52, 0.5), (59, 0.5), (64, 0.5), (59, 0.5),
        // Am -> G
        (57, 0.5), (64, 0.5), (57, 0.5), (52, 0.5), (55, 0.5), (62, 0.5), (55, 0.5), (50, 0.5),
        // Am
        (57, 0.5), (64, 0.5), (57, 0.5), (52, 0.5), (57, 0.5), (64, 0.5), (57, 0.5), (55, 0.5),
        // F -> Dm
        (53, 0.5), (60, 0.5), (53, 0.5), (48, 0.5), (50, 0.5), (57, 0.5), (50, 0.5), (53, 0.5),
        // G
        (55, 0.5), (62, 0.5), (55, 0.5), (59, 0.5), (55, 0.5), (62, 0.5), (59, 0.5), (55, 0.5),
        // G7 -> C
        (55, 0.5), (59, 0.5), (62, 0.5), (59, 0.5), (55, 0.5), (48, 1.5),
    ];

    // --- Section D (6 bars / 24 beats): Punchy recap & loop tag ---
    let lead_d: &[(u8, f32)] = &[
        // D1
        (60, 0.5), (64, 0.5), (67, 0.5), (72, 0.5), (69, 0.5), (67, 0.5), (64, 1.0),
        // D2
        (65, 0.5), (69, 0.5), (72, 0.5), (71, 0.5), (69, 0.5), (67, 0.5), (69, 1.0),
        // D3
        (72, 0.5), (69, 0.5), (67, 0.5), (64, 0.5), (62, 0.5), (64, 0.5), (67, 0.5), (69, 0.5),
        // D4
        (72, 0.5), (71, 0.5), (69, 0.5), (67, 0.5), (64, 0.5), (67, 0.5), (65, 1.0),
        // D5
        (64, 0.5), (62, 0.5), (60, 0.5), (64, 0.5), (67, 0.5), (69, 0.5), (71, 1.0),
        // D6: Clean launchpad back to beat 1
        (72, 0.5), (67, 0.5), (64, 0.5), (60, 0.5), (67, 1.0), (0, 1.0),
    ];

    let bass_d: &[(u8, f32)] = &[
        // C
        (48, 0.5), (60, 0.5), (48, 0.5), (55, 0.5), (48, 0.5), (60, 0.5), (55, 0.5), (48, 0.5),
        // F -> G
        (53, 0.5), (60, 0.5), (53, 0.5), (57, 0.5), (55, 0.5), (62, 0.5), (55, 0.5), (50, 0.5),
        // Am -> F
        (57, 0.5), (60, 0.5), (57, 0.5), (52, 0.5), (53, 0.5), (60, 0.5), (53, 0.5), (48, 0.5),
        // Dm -> G
        (50, 0.5), (57, 0.5), (50, 0.5), (53, 0.5), (55, 0.5), (59, 0.5), (53, 0.5), (55, 0.5),
        // C -> G
        (48, 0.5), (55, 0.5), (48, 0.5), (52, 0.5), (55, 0.5), (59, 0.5), (62, 0.5), (55, 0.5),
        // G7 -> C
        (55, 0.5), (59, 0.5), (62, 0.5), (55, 0.5), (48, 1.0), (0, 1.0),
    ];

    let lead_notes = [lead_a, lead_b, lead_c, lead_d].concat();
    let bass_notes = [bass_a, bass_b, bass_c, bass_d].concat();

    // Soft eighth-note hi-hat bed so the rhythm feels like an arcade track, not a chime
    let total_beats: f32 = lead_notes.iter().map(|n| n.1).sum();
    let spb = SR as f32 * 60.0 / bpm;
    let total_samples = (total_beats * spb) as usize;
    let eighth_samples = (0.5 * spb) as usize;
    let hat_pulse = noise_burst(0.035, 0.045);
    let mut hats = vec![0.0f32; total_samples];
    let mut idx = 0usize;
    let mut step = 0usize;
    while idx < total_samples {
        let accent = if step % 2 == 1 { 1.15 } else { 0.75 };
        for (j, &s) in hat_pulse.iter().enumerate() {
            if idx + j >= hats.len() {
                break;
            }
            hats[idx + j] += s * accent;
        }
        idx += eighth_samples;
        step += 1;
    }

    // Slightly softer lead amplitude so it doesn't ring like a doorbell
    let lead = render_melody(&lead_notes, bpm, 0.12, false);
    let bass = render_melody(&bass_notes, bpm, 0.16, true);
    let mixed = mix(&lead, &bass);
    mix(&mixed, &hats)
}

fn explosion_sfx() -> Vec<f32> {
    let n1 = noise_burst(0.45, 0.5);
    let mut sweep = square_wave(110.0, 0.45, 0.25);
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
    let mut file = std::fs::File::create(path).unwrap();
    let data_len = (samples.len() * 2) as u32;
    let file_len = 36 + data_len;
    file.write_all(b"RIFF").unwrap();
    file.write_all(&file_len.to_le_bytes()).unwrap();
    file.write_all(b"WAVE").unwrap();
    file.write_all(b"fmt ").unwrap();
    file.write_all(&16u32.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap();
    file.write_all(&SR.to_le_bytes()).unwrap();
    let byte_rate = SR * 2;
    file.write_all(&byte_rate.to_le_bytes()).unwrap();
    file.write_all(&2u16.to_le_bytes()).unwrap();
    file.write_all(&16u16.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap();
    file.write_all(&data_len.to_le_bytes()).unwrap();
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        file.write_all(&v.to_le_bytes()).unwrap();
    }
}

fn player_cmd() -> &'static str {
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
    // --gamepad <N> takes a mandatory numeric index argument (i += 2).
    // If no numeric value follows --gamepad, the next token is consumed as
    // the failed parse target, so place --gamepad <N> carefully relative to
    // flags like --width and --height.
    let mut gamepad_index: usize = 0;
    let mut no_audio = false;
    let mut width: usize = 15;
    let mut height: usize = 15;
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
        } else if args[i] == "--width" {
            if i + 1 < args.len() {
                width = args[i + 1].parse().unwrap_or(width);
                i += 2;
                continue;
            }
        } else if let Some(val) = args[i].strip_prefix("--width=") {
            width = val.parse().unwrap_or(width);
        } else if args[i] == "--height" {
            if i + 1 < args.len() {
                height = args[i + 1].parse().unwrap_or(height);
                i += 2;
                continue;
            }
        } else if let Some(val) = args[i].strip_prefix("--height=") {
            height = val.parse().unwrap_or(height);
        }
        i += 1;
    }

    fn odd_min9(n: usize) -> usize {
        let v = if n % 2 == 0 { n.saturating_sub(1) } else { n };
        if v < 9 { 9 } else { v }
    }
    width = odd_min9(width);
    height = odd_min9(height);

    // --- gamepad init -----------------------------------------------------
    let mut gilrs = Gilrs::new().ok();
    let gamepad_id = if let Some(g) = gilrs.as_mut() {
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

    #[derive(Clone, Copy)]
    struct Mapping {
        up: Button,
        down: Button,
        left: Button,
        right: Button,
        bomb: Button,
        detonate: Button,
        pause: Button,
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
            pause: map_one("PAUSE"),
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

    let mut game = Game::new(width, height);

    let tick_rate = Duration::from_millis(50);
    let render_rate = Duration::from_millis(33);
    let mut last_tick = Instant::now();
    let move_cooldown = Duration::from_millis(150);
    let mut last_move = Instant::now();
    let mut prev_bomb = false;
    let mut prev_detonate = false;
    let mut prev_pause = false;

    loop {
        terminal.draw(|f| game.render(f))?;

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
                            KeyCode::Char('r') if game.game_over || game.won => {
                                game = Game::new(width, height);
                            }
                            KeyCode::Up | KeyCode::Char('w') => game.move_player(0, -1),
                            KeyCode::Down | KeyCode::Char('s') => game.move_player(0, 1),
                            KeyCode::Left | KeyCode::Char('a') => game.move_player(-1, 0),
                            KeyCode::Right | KeyCode::Char('d') => game.move_player(1, 0),
                            KeyCode::Char(' ') => game.place_bomb(),
                            KeyCode::Char('p') | KeyCode::Char('P') | KeyCode::Esc => {
                                game.toggle_pause()
                            }
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
                let held_pause = gp.is_pressed(m.pause);

                if !game.is_frozen() && last_move.elapsed() >= move_cooldown {
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

                if held_bomb && !prev_bomb && !game.is_frozen() {
                    game.place_bomb();
                }
                prev_bomb = held_bomb;

                if held_detonate && !prev_detonate && !game.is_frozen() {
                    if game.detonate() {
                        game.update();
                    }
                }
                prev_detonate = held_detonate;

                if held_pause && !prev_pause && !game.game_over && !game.won {
                    game.toggle_pause();
                }
                prev_pause = held_pause;
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
}
