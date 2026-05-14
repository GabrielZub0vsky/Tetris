//! Basic (non-garbage) multiplayer tests

// TODO: IMPORT THE ServerState TYPE YOU DEFINED
// if the line below works, you don't need to change anything.
use crate::ServerState;

use bevy::prelude::*;
use common::config::{BagType, GameConfig};
use common::{FIXED_TIMESTEP_HZ, data::*};
use lightyear::prelude::LinkOf;

use super::net::*;

// Common situation in both uninitialized and barely-initialized client game states.
fn assert_common(world: &mut World) {
    assert_eq!(
        world
            .query_filtered::<&Tetromino, With<Hold>>()
            .iter(world)
            .len(),
        0,
        "There should be no held tetrominoes"
    );
    assert_eq!(
        world.query::<&Obstacle>().iter(world).len(),
        0,
        "There should be no obstacles"
    );
}

fn assert_world_is_initialized(world: &mut World, num_players: usize) {
    assert_eq!(
        world.query::<&SharedGameState>().iter(world).len(),
        1,
        "There must be exactly one shared game state"
    );
    assert_eq!(
        world
            .query_filtered::<&Tetromino, With<Active>>()
            .iter(world)
            .len(),
        num_players,
    );
    assert_eq!(
        world
            .query_filtered::<&Tetromino, With<Next>>()
            .iter(world)
            .len(),
        1,
    );
    assert_common(world);
}

fn assert_world_is_uninitialized(world: &mut World) {
    assert_eq!(world.query::<&SharedGameState>().iter(world).len(), 0,);
    assert_eq!(
        world
            .query_filtered::<&Tetromino, With<Active>>()
            .iter(world)
            .len(),
        0,
    );
    assert_eq!(
        world
            .query_filtered::<&Tetromino, With<Next>>()
            .iter(world)
            .len(),
        0,
    );
    assert_common(world);
}

#[test]
fn test_waiting_2_players() {
    let mut runner = Runner::new(GameConfig {
        bag: BagType::Deterministic,
        expected_players: 2,
        ..Default::default()
    });

    runner.add_client();

    // Connection should be established in 3 frames
    for _ in 0..3 {
        let state = runner.server.world().resource::<State<ServerState>>();
        info!("server state: {state:?}");
        runner.step();
    }

    let state = runner.server.world().resource::<State<ServerState>>();
    assert_eq!(*state, ServerState::Pending);
    assert_world_is_uninitialized(runner.client_world(0));

    // add the second client
    runner.add_client();

    // Connection should be established in 3 frames
    for _ in 0..3 {
        let state = runner.server.world().resource::<State<ServerState>>();
        info!("server state: {state:?}");
        runner.step();
    }

    let state = runner.server.world().resource::<State<ServerState>>().get();
    assert_eq!(*state, ServerState::Running);
    assert_world_is_initialized(runner.client_world(0), 2);
    assert_world_is_initialized(runner.client_world(1), 2);
}

#[test]
fn test_waiting_3_players() {
    let mut runner = Runner::new(GameConfig {
        bag: BagType::Deterministic,
        expected_players: 3,
        ..Default::default()
    });

    runner.add_client();
    runner.add_client();

    // Connection should be established in 3 frames
    for _ in 0..3 {
        let state = runner.server.world().resource::<State<ServerState>>();
        info!("server state: {state:?}");
        runner.step();
    }

    let state = runner.server.world().resource::<State<ServerState>>().get();
    assert_eq!(*state, ServerState::Pending);
    assert_world_is_uninitialized(runner.client_world(0));
    assert_world_is_uninitialized(runner.client_world(1));

    // add the last client
    runner.add_client();

    // Connection should be established in 3 frames
    for _ in 0..3 {
        let state = runner.server.world().resource::<State<ServerState>>();
        info!("server state: {state:?}");
        runner.step();
    }

    let state = runner.server.world().resource::<State<ServerState>>().get();
    assert_eq!(*state, ServerState::Running);
    assert_world_is_initialized(runner.client_world(0), 3);
    assert_world_is_initialized(runner.client_world(1), 3);
    assert_world_is_initialized(runner.client_world(2), 3);
}

#[test]
fn test_disconnect_after_game_starts() {
    let mut runner = Runner::new(GameConfig {
        bag: BagType::Deterministic,
        expected_players: 2,
        ..Default::default()
    });

    runner.add_client();
    runner.add_client();

    // Connection should be established in 3 frames
    for _ in 0..3 {
        let state = runner.server.world().resource::<State<ServerState>>();
        info!("server state: {state:?}");
        runner.step();
    }

    let state = runner.server.world().resource::<State<ServerState>>().get();
    assert_eq!(*state, ServerState::Running);
    assert_world_is_initialized(runner.client_world(0), 2);
    assert_world_is_initialized(runner.client_world(1), 2);

    // this client should fail to connect
    runner.add_client();

    for _ in 0..3 {
        let state = runner.server.world().resource::<State<ServerState>>();
        info!("server state: {state:?}");
        runner.step();
    }

    let state = runner.server.world().resource::<State<ServerState>>().get();
    assert_eq!(*state, ServerState::Running);
    assert_world_is_initialized(runner.client_world(0), 2);
    assert_world_is_initialized(runner.client_world(1), 2);

    let world = runner.client_world(2);
    // the client queued to disconnect will initially receive the data broadcast
    // to other clients but not the shared game state.
    assert_eq!(world.query::<&SharedGameState>().iter(world).len(), 0,);
    assert_eq!(
        world
            .query_filtered::<&Tetromino, With<Active>>()
            .iter(world)
            .len(),
        2
    );
    assert_eq!(
        world
            .query_filtered::<&Tetromino, With<Next>>()
            .iter(world)
            .len(),
        0,
    );
    assert_common(world);

    // pass the time until a disconnect is triggered (half a second should be
    // enough).
    for _ in 0..(FIXED_TIMESTEP_HZ / 2.0) as usize {
        runner.step();
    }

    // not asserting a disconnect on the client state because that's not
    // detected via the raw protocol+the crossbeam channels we're using.
    let server_world = runner.server.world_mut();
    let num_clients = server_world.query::<&LinkOf>().iter(server_world).len();
    assert_eq!(num_clients, 2);

    let state = server_world.resource::<State<ServerState>>().get();
    assert_eq!(*state, ServerState::Running);
    assert_world_is_initialized(runner.client_world(0), 2);
    assert_world_is_initialized(runner.client_world(1), 2);
}
