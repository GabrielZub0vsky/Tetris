//! Record-replay functionality used for testing.
#![warn(missing_docs)]

use std::{collections::VecDeque, time::Duration};

use bevy::ecs::resource::Resource;
use serde::{Deserialize, Serialize};

use crate::{board::Block, data::Tetromino};

// relevant keys without hauling bevy_input
#[derive(Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
pub enum KeyCode {
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    Escape,
    KeyZ,
    KeyX,
    Space,
}

/// A recording of the events and the game state.
#[derive(Serialize, Deserialize, Resource, Default)]
pub struct GameRecording {
    /// Input events, ordered by time
    pub events: VecDeque<InputEvent>,
    /// State snapshots, ordered by time, used for testing and debugging.
    pub snapshots: VecDeque<(Duration, Snapshot)>,
}

/// An input event (which keys are pressed/released, etc.) with a time stamp.
#[derive(Serialize, Deserialize)]
pub struct InputEvent {
    /// Time this event occurred at, as a duration since the startup
    pub time: Duration,
    /// Set of keys just pressed
    pub just_pressed: Vec<KeyCode>,
    /// Set of keys just released
    pub just_released: Vec<KeyCode>,
}

/// A snapshot of the game state for testing and debugging.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[allow(missing_docs)]
pub struct Snapshot {
    pub active: Option<Tetromino>,
    pub next: Option<Tetromino>,
    /// Obstacle vector, ordered as a block
    pub obstacles: Vec<Block>,
    pub hold: Option<Tetromino>,
    pub hard_drop: bool,
    pub manual_gravity: u32,
    pub score: u32,
    pub lines_cleared: u32,
    pub level: u32,
}

/// Fixed frame rate, to adjust timing for record and replay
pub const FIXED_FRAME_DURATION: Duration = Duration::from_nanos(1_000_000_000 / 64);
