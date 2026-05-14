//! Our application-level communication protocol

use std::collections::HashSet;

use bevy::prelude::*;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    board::Block,
    data::{Active, Hold, Next, Obstacle, SharedGameState, Tetromino},
};

/// The entity that owns a tetromino/obstacle
#[derive(Component, Copy, Clone, Serialize, Deserialize, PartialEq)]
pub struct BelongsTo(pub u64);

/// Channel for keyboard inputs to send over the network.
#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Clone, Reflect)]
pub struct InputChannel;

/// Inputs from the client
#[derive(PartialEq, Eq, Hash, Clone, Copy, Debug, Serialize, Deserialize)]
#[allow(missing_docs)]
pub enum Input {
    Left,
    Right,
    Down,
    Rotate,
    Hold,
    HardDrop,
    EndGame,
}

/// Set of logical inputs to be sent as a message
#[derive(PartialEq, Eq, Clone, Serialize, Deserialize, Default, Debug)]
pub struct Inputs(pub HashSet<Input>);

/// Heartbeat message and channel
#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Clone, Reflect)]
pub struct Heartbeat;

/// A message to signal a client that its game has ended.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub enum GameOverMessage {
    /// The receiver lost the game
    Lost,
    /// The receiver won the game
    Won,
}

/// A channel for game state changes
#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Clone, Reflect)]
pub struct StateChange;

/// Protocol plugin added to both server and client
pub struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // Register components that should be replicated
        app.register_component::<SharedGameState>();
        app.register_component::<Tetromino>();
        app.register_component::<Block>();
        app.register_component::<Active>();
        app.register_component::<Next>();
        app.register_component::<Obstacle>();
        app.register_component::<Hold>();
        app.register_component::<BelongsTo>();

        // Create a channel for inputs
        app.add_channel::<InputChannel>(ChannelSettings {
            mode: ChannelMode::SequencedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::ClientToServer);

        // Create a channel for heartbeat signals
        app.add_channel::<Heartbeat>(ChannelSettings {
            mode: ChannelMode::UnorderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::ClientToServer);

        // Create a channel for game state changes
        app.add_channel::<StateChange>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::ServerToClient);

        // Register the messages we can send
        app.register_message::<Inputs>()
            .add_direction(NetworkDirection::ClientToServer);
        app.register_message::<Heartbeat>()
            .add_direction(NetworkDirection::ClientToServer);
        app.register_message::<GameOverMessage>()
            .add_direction(NetworkDirection::ServerToClient);
    }
}
