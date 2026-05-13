//! The tetris board setup

use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::data::*;
use crate::protocol::*;
use bevy::prelude::*;

/// Side-length of an *unscaled* tile in pixels.
pub const TILE_SIDE_LEN: f32 = 40.0;

/// Amount of time before a tile is locked.
pub const LOCKDOWN_DURATION: Duration = SharedGameState::initial_drop_interval();

/// An event signalling that the game is over.
#[derive(EntityEvent)]
pub struct GameOver {
    /// The target of this event
    pub entity: Entity,
}

/// An event signalling that the current program received some input to handle.
#[derive(EntityEvent)]
#[allow(missing_docs)]
pub struct ReceivedInput {
    pub inputs: Inputs,
    #[event_target]
    pub target: Entity,
}

/// An block.  This is one of:
/// - an obstacle (leftovers of an inactive tetromino)
/// - a block used in the preview and held views.
#[derive(Component, Copy, Clone, PartialEq, Debug, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct Block {
    /// Coordinates of this block
    pub cell: Cell,
    /// Color of this block
    pub color: Color,
}

/// A timer to count down when a piece must be inactivated after it can't be pushed down
#[derive(Component)]
pub struct LockdownTimer(pub Option<Timer>);

impl LockdownTimer {
    /// Advance the timer. Start it if it hasn't been started.
    pub fn start_or_advance(&mut self, time: &Time<Fixed>) {
        if let Some(timer) = &mut self.0 {
            timer.tick(time.delta());
        } else {
            self.0 = Some(Timer::new(LOCKDOWN_DURATION, TimerMode::Once));
        }
    }

    /// Has this timer just gone off?
    pub fn just_finished(&self) -> bool {
        self.0.as_ref().is_some_and(Timer::is_finished)
    }

    /// Destroy the underlying timer
    pub fn reset(&mut self) {
        self.0 = None;
    }
}

/// Trigger GameOver when the EndGame key is pressed
pub fn game_over_on_esc(trigger: On<ReceivedInput>, mut commands: Commands) {
    if trigger.inputs.0.contains(&Input::EndGame) {
        commands.trigger(GameOver {
            entity: trigger.target,
        });
    }
}
