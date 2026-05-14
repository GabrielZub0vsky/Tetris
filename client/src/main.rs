//! The client binary.
#![allow(clippy::type_complexity)]

use ::client::get_web_resource;
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use common::config::GameConfig;
use common::data::Active;
use common::protocol::*;
use common::*;
use core::net::SocketAddr;
use core::time::Duration;
use lightyear::prelude::client::*;
use lightyear::prelude::{Message, *};
use std::collections::HashSet;
use std::net::Ipv4Addr;
use wasm_bindgen::prelude::wasm_bindgen;

const CONFIG_PATH: &str = "config.json";

// The client systems together as a plugin
struct MyClientPlugin {
    cfg: GameConfig,
}

impl Plugin for MyClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_client);
        app.add_systems(
            Update,
            (
                // send_input.run_if(can_send_message::<InputChannel>),
                send_heartbeat
                    .run_if(on_timer(HEARTBEAT_INTERVAL))
                    .run_if(can_send_message::<Heartbeat>),
                // update_block_transform,
                try_reconnect.run_if(on_timer(RECONNECT_INTERVAL)),
                debug_client_state.run_if(on_timer(Duration::from_millis(1000))),
            ),
        );
        ::client::build_app(app, self.cfg);
    }
}

// A run condition to see if we have an established connection that can send a
// given message.
//
// Using this condition lets us simplify some systems (use Single rather than
// Option<Single>).
fn can_send_message<M: Message>(
    senders: Query<(), (With<MessageSender<M>>, With<Client>, With<Connected>)>,
) -> bool {
    !senders.is_empty()
}

// Print the client state for demonstration
fn debug_client_state(
    client: Option<Single<(Entity, &Client)>>,
    tetrominoes: Query<&BelongsTo, With<Active>>,
) {
    if let Some((e, client)) = client.map(Single::into_inner) {
        info!("Client {:?}: {:?}", e, client.state);
        println!(
            "BelongsTo received for: {:?}",
            tetrominoes.iter().map(|b| b.0).collect::<HashSet::<_>>()
        );
    }
}

/// Spawn the lightyear client entity.
fn spawn_client(mut commands: Commands, cfg: Res<GameConfig>) {
    let client_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0);
    let server_addr = SocketAddr::new(cfg.server_addr, cfg.server_port);
    let client = commands
        .spawn((
            RawClient,
            LocalAddr(client_addr),
            PeerAddr(server_addr),
            WebSocketClientIo::from_addr(ClientConfig, WebSocketScheme::Plain),
            Link::new(None),
            ReplicationReceiver::default(),
        ))
        .id();
    commands.trigger(Connect { entity: client });
    info!("Initiating connection to {server_addr}");
}

/// Retry the connection while the client is disconnected.
fn try_reconnect(
    disconnected: Option<Single<Entity, (With<Client>, With<Disconnected>)>>,
    mut commands: Commands,
) {
    let Some(entity) = disconnected else { return };
    info!("Disconnected: retrying connection...");
    commands.trigger(Connect { entity: *entity });
}

/// Send a heartbeat signal to the server
fn send_heartbeat(
    mut sender: Single<&mut MessageSender<Heartbeat>, (With<Client>, With<Connected>)>,
) {
    info!("Sending heartbeat");
    sender.send::<Heartbeat>(Heartbeat);
}

// this is for setting up the panic hook early.
#[wasm_bindgen(start)]
async fn start() {
    // print useful messages on panic.
    console_error_panic_hook::set_once();

    let json = get_web_resource(CONFIG_PATH)
        .await
        .expect("Could not fetch the config file");
    let cfg = GameConfig::load(&json).expect("Could not parse the config file");
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            // This tells Bevy that our window should be projected to the given Canvas.
            primary_window: Some(Window {
                canvas: Some("#canvas".to_string()),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(ClientPlugins {
            tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
        })
        .insert_resource(cfg)
        .add_plugins(ProtocolPlugin)
        .add_plugins(MyClientPlugin { cfg })
        .run();
}

fn main() {}
