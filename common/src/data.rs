//! Data structures and fixtures used by the ECS framework

use std::time::Duration;

use bevy::{color::palettes::tailwind, prelude::*};
use rand::rngs::SmallRng;
use serde::{Deserialize, Serialize};

use crate::bag::Bag;

/// A 2-d cell, represented via its x and y coordinates
#[derive(Copy, Clone, PartialEq, Eq, Debug, PartialOrd, Ord, Serialize, Deserialize, Hash)]
pub struct Cell(pub i32, pub i32);
impl Cell {
    fn rotate_90_deg_cw(&self, x: f32, y: f32) -> Cell {
        let dx = self.0 as f32 - x;
        let dy = self.1 as f32 - y;

        Cell((x + dy).round() as i32, (y - dx).round() as i32)
    }

    /// Check whether this cell is in bounds.
    ///
    /// This takes the invisible rows into account
    pub fn in_bounds(&self) -> bool {
        (0..BOARD_WIDTH as i32).contains(&self.0)
            && (0..BOARD_HEIGHT as i32 + INVISIBLE_ROWS as i32).contains(&self.1)
    }

    /// Check whether this cell is in the visible part of the board.
    pub fn is_visible(&self) -> bool {
        (0..BOARD_WIDTH as i32).contains(&self.0) && (0..BOARD_HEIGHT as i32).contains(&self.1)
    }
}

/// A bounding box for a tetromino
#[derive(Copy, Clone, PartialEq, Eq)]
#[allow(missing_docs)]
pub struct Bounds {
    pub top: i32,
    pub bottom: i32,
    pub left: i32,
    pub right: i32,
}

/// Whether this tetromino is the active one
#[derive(Component, Copy, Clone, Serialize, Deserialize, PartialEq)]
pub struct Active;

/// Whether this tetromino is the next one
#[derive(Component, Copy, Clone, Serialize, Deserialize, PartialEq)]
pub struct Next;

/// Whether this tetromino is the one being held
#[derive(Component, Copy, Clone, Serialize, Deserialize, PartialEq)]
pub struct Hold;

/// Whether a block is an obstacle
#[derive(Component, Copy, Clone, Serialize, Deserialize, PartialEq)]
pub struct Obstacle;

/// Whether a block is on the main board
#[derive(Debug, Component, Copy, Clone, PartialEq, Eq, Hash)]
// pub struct MainBoard;
#[allow(missing_docs)]
pub enum GameBoard {
    MainBoard,
    Left,
    Right,
}

/// The cell associated with a tetromino
#[derive(Component, Clone, Copy, Debug, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct Tetromino {
    pub cells: [Cell; 4],
    pub center: (f32, f32),
    pub color: Color,
}

impl Tetromino {
    /// Cells of the tetromino
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// Center of the tetromino for the purposes of rotation
    pub fn center(&self) -> (f32, f32) {
        self.center
    }

    /// The bounding box of the tetromino
    pub fn bounds(&self) -> Bounds {
        debug_assert!(!self.cells.is_empty());
        Bounds {
            left: self.cells.iter().map(|c| c.0).min().unwrap(),
            right: self.cells.iter().map(|c| c.0).max().unwrap(),
            bottom: self.cells.iter().map(|c| c.1).min().unwrap(),
            top: self.cells.iter().map(|c| c.1).max().unwrap(),
        }
    }

    /// Is the tetromino in bounds?
    pub fn in_bounds(&self) -> bool {
        self.cells.iter().all(Cell::in_bounds)
    }

    /// Is this the O tetromino?
    pub fn is_o(&self) -> bool {
        let mut cells = self.cells;
        cells.sort();
        let Cell(x, y) = cells[0];
        cells
            == [
                Cell(x, y),
                Cell(x, y + 1),
                Cell(x + 1, y),
                Cell(x + 1, y + 1),
            ]
    }

    /// Rotate this tetromino 90 degrees clockwise.
    pub fn rotate(&mut self) {
        if self.is_o() {
            return;
        }

        // rotate everything 90 degrees around the center.
        self.cells
            .iter_mut()
            .for_each(|cell| *cell = cell.rotate_90_deg_cw(self.center.0, self.center.1));
    }

    /// Shift all the cells in the tetromino by the given amount
    pub fn shift(&mut self, dx: i32, dy: i32) {
        for Cell(x, y) in self.cells.iter_mut() {
            *x += dx;
            *y += dy;
        }
        self.center.0 += dx as f32;
        self.center.1 += dy as f32;
    }
}

// Custom equality information that ignores the ordering of cells
impl PartialEq for Tetromino {
    fn eq(&self, other: &Self) -> bool {
        let mut cells1 = self.cells;
        cells1.sort();
        let mut cells2 = other.cells;
        cells2.sort();
        cells1 == cells2 && self.center == other.center && self.color == other.color
    }
}

/// Height of the visible part of the board
pub const BOARD_HEIGHT: u32 = 20;
/// Width of the visible part of the board
pub const BOARD_WIDTH: u32 = 10;
/// The number of invisible rows on top of the board
pub const INVISIBLE_ROWS: u32 = 3;
/// Gravity number to be used when hard drop is enabled
pub const HARD_DROP_GRAVITY: u32 = 20;
/// Gravity number to be used when hard drop is disabled
pub const SOFT_DROP_GRAVITY: u32 = 1;

/// Shared game configuration.  This is a component instead of a resource
/// because:
///
/// 1. We want to replicate it between the server and the clients.
/// 2. The server may maintain multiple clients' game state.
///
/// Also note that it is slightly different from milestone 1.
#[derive(Component, Serialize, Deserialize, PartialEq, Eq, Reflect, Debug)]
pub struct SharedGameState {
    /// Total score so far
    pub score: u32,
    /// Total lines cleared
    pub lines_cleared: u32,
    /// Lines cleared since last level up
    pub lines_cleared_since_last_level: u32,
    /// Current level
    pub level: u32,
    /// Whether hard drop is enabled.  We don't keep this as a separate resource
    /// anymore.
    pub hard_drop: bool,
    /// How many blocks to drop on user input.  This value should
    /// change depending on whether hard drop is enabled.
    pub manual_drop_gravity: u32,
    /// Timer for querying when the next automatic drop happens
    pub gravity_timer: Timer,
    /// The Id of the client
    pub client_id: u64,
}

impl SharedGameState {
    const MAX_LEVEL: usize = 29;
    const FRAMERATE: f32 = 60.0;
    const INTERVALS: [f32; Self::MAX_LEVEL] = [
        48.0, 43.0, 38.0, 33.0, 28.0, 23.0, 18.0, 13.0, 8.0, 6.0, 5.0, 5.0, 5.0, 4.0, 4.0, 4.0,
        3.0, 3.0, 3.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 1.0,
    ];

    /// Current score
    pub fn score(&self) -> u32 {
        self.score
    }

    /// Current level
    pub fn level(&self) -> u32 {
        self.level
    }

    /// Auto-drop interval (i.e., gravity)
    pub fn drop_interval(&self) -> Duration {
        Duration::from_secs_f32(
            Self::INTERVALS[(self.level() as usize).min(Self::MAX_LEVEL - 1)] / Self::FRAMERATE,
        )
    }

    /// The drop interval in the beginning of the game.
    pub const fn initial_drop_interval() -> Duration {
        Duration::from_micros((Self::INTERVALS[0] / Self::FRAMERATE * 1e6) as u64)
    }
}

/// Private game configuration, used for internal systems and contains data that
/// cannot/should not be send to the client.
#[derive(Component)]
pub struct PrivateGameState {
    /// The next piece bag
    pub bag: Box<dyn Bag + Sync>,
    /// Whether to send garbage when scoring
    pub send_garbage: bool,
    /// The RNG to use for generating garbage
    pub garbage_rng: SmallRng,
}

/// Different types of canonical tetrominos.
#[derive(Copy, Clone)]
#[allow(missing_docs)]
pub enum TetrominoType {
    S,
    Z,
    L,
    J,
    T,
    O,
    I,
}

/// Get a fresh canonical tetromino of the given type.
pub const fn get_tetromino(typ: TetrominoType) -> Tetromino {
    use TetrominoType::*;
    let (cells, center, color) = match typ {
        S => (
            [Cell(-1, 0), Cell(0, 0), Cell(0, 1), Cell(1, 1)],
            (0., 0.),
            tailwind::RED_400,
        ),
        Z => (
            [Cell(-1, 1), Cell(0, 0), Cell(0, 1), Cell(1, 0)],
            (0., 0.),
            tailwind::ORANGE_400,
        ),
        L => (
            [Cell(-1, 0), Cell(0, 0), Cell(1, 0), Cell(-1, 1)],
            (0., 0.),
            tailwind::YELLOW_400,
        ),
        J => (
            [Cell(-1, 0), Cell(0, 0), Cell(1, 0), Cell(1, 1)],
            (0., 0.),
            tailwind::GREEN_400,
        ),
        T => (
            [Cell(0, 1), Cell(-1, 0), Cell(0, 0), Cell(1, 0)],
            (0.0, 0.0),
            tailwind::BLUE_400,
        ),
        O => (
            [Cell(0, 0), Cell(0, 1), Cell(1, 0), Cell(1, 1)],
            (0.5, 0.5),
            tailwind::CYAN_400,
        ),
        I => (
            [Cell(-1, 0), Cell(0, 0), Cell(1, 0), Cell(2, 0)],
            (0.5, -0.5),
            tailwind::PURPLE_400,
        ),
    };

    Tetromino {
        cells,
        center,
        color: Color::Srgba(color),
    }
}

/// All canonical tetrominoes that can occur in the game.
#[allow(unused)]
pub const ALL_TETROMINO_TYPES: [TetrominoType; 7] = [
    TetrominoType::S,
    TetrominoType::Z,
    TetrominoType::L,
    TetrominoType::J,
    TetrominoType::T,
    TetrominoType::O,
    TetrominoType::I,
];

#[cfg(test)]
mod tests {
    use super::*;

    // Create a game state fixture
    fn mk_game_state() -> SharedGameState {
        SharedGameState {
            score: 0,
            lines_cleared: 0,
            lines_cleared_since_last_level: 0,
            level: 0,
            hard_drop: false,
            manual_drop_gravity: 0,
            gravity_timer: Timer::new(Duration::from_millis(1000), TimerMode::Repeating),
            client_id: 0,
        }
    }

    #[test]
    fn rotate_cell() {
        let cell = Cell(3, 4);
        assert_eq!(Cell(4, -3), cell.rotate_90_deg_cw(0.0, 0.0));
        assert_eq!(Cell(4, 7), cell.rotate_90_deg_cw(5.0, 5.0));
        assert_eq!(Cell(3, 7), cell.rotate_90_deg_cw(4.5, 5.5));

        let cell = Cell(-2, 4);
        assert_eq!(Cell(4, 2), cell.rotate_90_deg_cw(0.0, 0.0));

        let cell = Cell(2, -4);
        assert_eq!(Cell(-4, -2), cell.rotate_90_deg_cw(0.0, 0.0));
    }

    #[test]
    fn is_o() {
        for typ in ALL_TETROMINO_TYPES {
            assert_eq!(get_tetromino(typ).is_o(), matches!(typ, TetrominoType::O));
        }
    }

    #[test]
    fn rotate_cells() {
        use TetrominoType::*;

        let mut tetromino = get_tetromino(S);
        tetromino.rotate();
        assert_eq!(
            tetromino.cells,
            [Cell(0, 1), Cell(0, 0), Cell(1, 0), Cell(1, -1)]
        );
        tetromino.rotate();
        assert_eq!(
            tetromino.cells,
            [Cell(1, 0), Cell(0, 0), Cell(0, -1), Cell(-1, -1)]
        );
        tetromino.rotate();
        assert_eq!(
            tetromino.cells,
            [Cell(0, -1), Cell(0, 0), Cell(-1, 0), Cell(-1, 1)]
        );
        tetromino.rotate();
        assert_eq!(tetromino, get_tetromino(S));

        let mut tetromino = get_tetromino(Z);
        tetromino.rotate();
        assert_eq!(
            tetromino.cells,
            [Cell(1, 1), Cell(0, 0), Cell(1, 0), Cell(0, -1)]
        );

        let mut tetromino = get_tetromino(L);
        tetromino.rotate();
        assert_eq!(
            tetromino.cells,
            [Cell(0, 1), Cell(0, 0), Cell(0, -1), Cell(1, 1)]
        );

        let mut tetromino = get_tetromino(J);
        tetromino.rotate();
        assert_eq!(
            tetromino.cells,
            [Cell(0, 1), Cell(0, 0), Cell(0, -1), Cell(1, -1)]
        );

        let mut tetromino = get_tetromino(T);
        tetromino.rotate();
        assert_eq!(
            tetromino.cells,
            [Cell(1, 0), Cell(0, 1), Cell(0, 0), Cell(0, -1)]
        );

        let mut tetromino = get_tetromino(O);
        tetromino.rotate();
        assert_eq!(
            tetromino.cells,
            [Cell(0, 0), Cell(0, 1), Cell(1, 0), Cell(1, 1)]
        );

        let mut tetromino = get_tetromino(I);
        tetromino.rotate();
        assert_eq!(
            tetromino.cells,
            [Cell(1, 1), Cell(1, 0), Cell(1, -1), Cell(1, -2)]
        );
    }

    #[test]
    fn rotate_semi_involution() {
        for typ in ALL_TETROMINO_TYPES {
            let mut tetromino = get_tetromino(typ);
            tetromino.rotate();
            tetromino.rotate();
            tetromino.rotate();
            tetromino.rotate();
            assert_eq!(tetromino, get_tetromino(typ));
        }
    }

    #[test]
    fn rotate_test_center() {
        for typ in ALL_TETROMINO_TYPES {
            let mut tetromino = get_tetromino(typ);
            let old_tetromino = tetromino;
            tetromino.rotate();
            assert_eq!(tetromino.center, old_tetromino.center);
            let old_tetromino = tetromino;
            tetromino.rotate();
            assert_eq!(tetromino.center, old_tetromino.center);
            let old_tetromino = tetromino;
            tetromino.rotate();
            assert_eq!(tetromino.center, old_tetromino.center);
            let old_tetromino = tetromino;
            tetromino.rotate();
            assert_eq!(tetromino.center, old_tetromino.center);
        }
    }

    #[test]
    fn shift() {
        let mut tetromino = get_tetromino(TetrominoType::L);
        tetromino.shift(3, -2);
        assert_eq!(
            tetromino,
            Tetromino {
                cells: [Cell(2, -2), Cell(3, -2), Cell(4, -2), Cell(2, -1)],
                center: (3.0, -2.0),
                color: Color::from(tailwind::YELLOW_400),
            }
        );
    }

    #[test]
    fn initial_drop_interval() {
        assert_eq!(
            SharedGameState::initial_drop_interval(),
            Duration::from_millis(800)
        );
    }

    #[test]
    fn drop_interval() {
        let mut state = mk_game_state();
        for (level, frames) in SharedGameState::INTERVALS.into_iter().enumerate() {
            state.level = level as u32;
            assert_eq!(
                state.drop_interval().as_secs_f32(),
                frames / SharedGameState::FRAMERATE
            );
        }
    }
}
