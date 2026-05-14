//! The networking code, the bulk of the server.

use crate::{ServerState, controlled_by, replicate_to};
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use common::board::{LockdownTimer, ReceivedInput};
use common::config::GameConfig;
use common::protocol::*;
use common::*;
use core::net::SocketAddr;
use core::time::Duration;
use lightyear::core::tick::TickDuration;
use lightyear::core::time::TickInstant;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use std::collections::HashSet;
use std::net::{Ipv4Addr, Ipv6Addr};

/// The last time we received a heartbeat from a client.
#[derive(Component)]
pub struct LastReceived(Duration);

/// Marker component for clients that should be dropped soon
#[derive(Component)]
pub struct ToDrop;

/// Timer for deleting clients
#[derive(Component)]
#[allow(unused)] // remove after implementing everything in this file
pub struct DeleteTimer(Timer);

#[allow(unused)] // remove after implementing everything in this file
impl DeleteTimer {
    fn new() -> Self {
        Self(Timer::from_seconds(0.5, TimerMode::Once))
    }
}

// A helper to convert lightyear ticks to durations.  Not fully necessary.
fn current_time(timeline: &LocalTimeline, tick_duration: &TickDuration) -> Duration {
    TickInstant::from(timeline.tick()).as_duration(**tick_duration)
}

/// Inject the actual game server logic
pub struct ServerPlugin;

impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, startup)
            .add_observer(handle_new_client)
            .add_observer(handle_connected)
            .add_observer(handle_force_disconnect)
            .add_systems(
                Update,
                (
                    handle_input,
                    handle_heartbeat,
                    debug_server.run_if(on_timer(Duration::from_millis(1000))),
                    disconnect_unresponsive_clients.run_if(on_timer(HEARTBEAT_INTERVAL)),
                ),
            )
            .add_systems(
                PostUpdate,
                (drop_clients, detect_empty_lobby, tick_delete_timers),
            );
        crate::build_app(app);
    }
}

/// Spawn the WebSocket server entity and begin listening.
fn startup(mut commands: Commands, cfg: Res<GameConfig>) {
    let server_addr = if cfg.server_addr.is_ipv4() {
        Ipv4Addr::UNSPECIFIED.into()
    } else {
        Ipv6Addr::UNSPECIFIED.into()
    };
    let bind_addr = SocketAddr::new(server_addr, cfg.server_port);
    let ws_config = ServerConfig::builder()
        .with_bind_default(cfg.server_port)
        .with_no_encryption();
    let server = commands
        .spawn((
            RawServer,
            WebSocketServerIo { config: ws_config },
            LocalAddr(bind_addr),
        ))
        .id();
    commands.trigger(Start { entity: server });
    info!("Starting server on {bind_addr}");
}

/// Periodically log the server state
pub fn debug_server(servers: Query<(Entity, &Server)>) {
    for (entity, server) in servers {
        info!("server {:?}: {:?}", entity, server);
    }
}

/// When a new client link is established, enable replication to that client.
pub fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands, cfg: Res<GameConfig>) {
    info!("new client for {:?}", trigger.entity);
    commands
        .entity(trigger.entity)
        .insert(ReplicationSender::new(
            cfg.replication_interval(),
            SendUpdatesMode::SinceLastAck,
            false,
        ));
}

/// Change the game state when enough clients are connected
pub fn handle_connected(
    trigger: On<Add, Connected>,
    peer_ids: Query<Entity, With<ClientOf>>,
    cfg: Res<GameConfig>,
    mut commands: Commands,
    state: Res<State<ServerState>>,
    mut next_state: ResMut<NextState<ServerState>>,
) {
    if *state.get() == ServerState::Running {
        warn!(
            "Game already running, rejecting late client {:?}",
            trigger.entity
        );
        commands.entity(trigger.entity).insert(ToDrop);
        return;
    }

    let count = peer_ids.iter().count();
    info!(
        "Client connected ({count}/{} expected)",
        cfg.expected_players
    );
    if count >= cfg.expected_players {
        info!("Starting game");
        next_state.set(ServerState::Running);
    }
}

/// Drop clients that are queued to drop
pub fn drop_clients(to_drop: Query<Entity, With<ToDrop>>, mut commands: Commands) {
    if !to_drop.is_empty() {
        info!("dropping clients");
        for entity in &to_drop {
            commands.entity(entity).remove::<ToDrop>();
        }
        commands.trigger(ForceDisconnect {
            clients: to_drop.into_iter().collect(),
        });
    }
}

/// Detect when the lobby is empty, and switch back to the pending state
pub fn detect_empty_lobby(
    clients: Query<Entity, With<ClientOf>>,
    state: Res<State<ServerState>>,
    mut next_state: ResMut<NextState<ServerState>>,
) {
    if *state.get() == ServerState::Running && clients.is_empty() {
        info!("All clients gone, returning to Pending");
        next_state.set(ServerState::Pending);
    }
}

/// Consume received inputs, dispatch them only if the game is running.
pub fn handle_input(
    state: Res<State<ServerState>>,
    mut receivers: Query<(Entity, &mut MessageReceiver<Inputs>), With<ClientOf>>,
    mut commands: Commands,
) {
    for (client, mut receiver) in &mut receivers {
        let messages: Vec<Inputs> = receiver.receive_with_tick().map(|m| m.data).collect();
        if *state.get() != ServerState::Running {
            continue;
        }
        for inputs in messages {
            commands.trigger(ReceivedInput {
                inputs,
                target: client,
            });
        }
    }
}

// TODO: Create a system to spawn the game state for all clients
/// Spawn SharedGameState + PrivateGameState for every connected client when the
/// game transitions into the Running state.
pub fn spawn_client_game_states(
    clients: Query<Entity, (With<ClientOf>, With<Connected>)>,
    cfg: Res<GameConfig>,
    mut commands: Commands,
    peer_ids: Query<&RemoteId, With<ClientOf>>,
) {
    for client in &clients {
        let mut shared = cfg.build_shared_game_state();
        shared.client_id = client.to_bits();
        commands.spawn((
            shared,
            cfg.build_private_game_state(),
            LockdownTimer(None),
            controlled_by(client),
            replicate_to(client, &peer_ids),
        ));
    }
}

/// Refresh clients that send a heartbeat signal
pub fn handle_heartbeat(
    receivers: Query<(Entity, &mut MessageReceiver<Heartbeat>)>,
    mut commands: Commands,
    timeline: Res<LocalTimeline>,
    tick_duration: Res<TickDuration>,
) {
    for (client, mut receiver) in receivers {
        if receiver.receive().last().is_some() {
            info!("Heartbeat received from {client:?}");
            commands
                .entity(client)
                .insert(LastReceived(current_time(&timeline, &tick_duration)));
        }
    }
}

// TODO: Create a system to spawn the game state for all clients
//
// You should also register this system to run when the server switches to the
// running state.

/// Event to trigger clean-up of client resources when using Replicate::All
#[derive(Event)]
pub struct ForceDisconnect {
    clients: HashSet<Entity>,
}

/// Disconnect clients that we haven't received a message from in a long time.
pub fn disconnect_unresponsive_clients(
    clients: Query<(Entity, &LastReceived)>,
    timeline: Res<LocalTimeline>,
    tick_duration: Res<TickDuration>,
    mut commands: Commands,
) {
    let current_time = current_time(&timeline, &tick_duration);
    let mut clients_to_drop = HashSet::new();

    for (entity, LastReceived(last_received_time)) in &clients {
        if current_time - *last_received_time > DISCONNECT_INTERVAL {
            warn!("{entity:?} has been stale! disconnecting.");
            clients_to_drop.insert(entity);
        }
    }

    if !clients_to_drop.is_empty() {
        commands.trigger(ForceDisconnect {
            clients: clients_to_drop,
        });
    }

    // NOTE: this system just triggers the ForceDisconnect event, you need to
    // implement the stuff described below to handle these events.
}

// TODO: create systems and event handlers for:
//
// 1. To clean up entities controlled by a client on a disconnect we initiate,
//    and to set a timer to remove the client
// 2. To actually disconnect clients whose deletion timers go off
//
// Register these systems and handlers

/// Clean up entities controlled by disconnecting clients and set their delete timers.
pub fn handle_force_disconnect(
    trigger: On<ForceDisconnect>,
    controlled_entities: Query<(Entity, &ControlledBy)>,
    mut commands: Commands,
) {
    for &client in &trigger.clients {
        for (entity, controlled_by) in &controlled_entities {
            if controlled_by.owner == client {
                commands.entity(entity).despawn();
            }
        }
        commands.entity(client).insert(DeleteTimer::new());
    }
}

/// Tick delete timers and despawn clients whose timers have elapsed.
pub fn tick_delete_timers(
    mut timers: Query<(Entity, &mut DeleteTimer)>,
    time: Res<Time>,
    mut commands: Commands,
) {
    for (entity, mut timer) in &mut timers {
        timer.0.tick(time.delta());
        if timer.0.is_finished() {
            commands.entity(entity).despawn();
        }
    }
}
