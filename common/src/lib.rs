//! Shared functionality.

use std::time::Duration;

use bevy::prelude::*;

pub mod bag;
pub mod board;
pub mod config;
pub mod data;
pub mod protocol;

/// A system set to denote the systems that belong to the game.  We use this so
/// that the input systems injected by integration tests do not run into a race
/// condition with the systems from the game.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub struct Game;

/// A system set used by tests to inject systems before the actual game systems.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub struct PreGame;

/// A system set used by tests to inject systems after the actual game systems.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub struct PostGame;

/// Game timestep, shared between the client and the server.
pub const FIXED_TIMESTEP_HZ: f64 = 64.0;
/// How often to try to reconnect
pub const RECONNECT_INTERVAL: Duration = Duration::from_secs(3);
/// The interval for disconnecting a client if we haven't received any messages
/// from it.
pub const DISCONNECT_INTERVAL: Duration = Duration::from_secs(3);
/// The interval for sending heartbeat signals
/// The system for clean up also runs at this interval.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);

/// Return whether the given tetromino collides with any of the obstacles or it
/// is out of bounds.
pub fn there_is_collision<'a>(
    tetromino: &data::Tetromino,
    obstacles: impl Iterator<Item = &'a board::Block>,
) -> bool {
    for b in obstacles {
        if tetromino.cells().contains(&b.cell) {
            return true;
        }
    }

    // also check out-of-bounds
    !tetromino.in_bounds()
}
