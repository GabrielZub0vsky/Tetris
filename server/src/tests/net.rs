//! Network simulation for testing the server

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

// TODO: IMPORT THE ServerState TYPE YOU DEFINED.

use crate::net::*;
use crate::tests::rr::{FIXED_FRAME_DURATION, GameRecording, Snapshot};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy::time::common_conditions::on_timer;
use common::board::Block;
use common::config::{BagType, GameConfig};
use common::data::*;
use common::protocol::*;
use common::*;
use lightyear::crossbeam::*;
use lightyear::prelude::client::RawClient;
use lightyear::prelude::server::*;
use lightyear::prelude::{Message, *};

const TEST_DATA_DIR: &str = "test_data";

// Number of mismatched states allowed for a test to pass
const MAX_STATE_MISMATCHES: usize = 1;

// Maximum number of states to look ahead for a desync
const MAX_STATE_LOOKAHEAD: usize = 10;

#[derive(Resource, Clone)]
pub struct TestIo(pub CrossbeamIo);

/// A test runner with its own client and server
#[allow(missing_docs)]
pub struct Runner {
    pub clients: Vec<App>,
    pub server: App,
}

impl Runner {
    /// Create a new test runner with no clients
    pub fn new(cfg: GameConfig) -> Runner {
        let mut server = App::new();

        server.add_plugins(bevy::log::LogPlugin::default());
        build_server(&mut server, cfg);

        server.finish();
        server.cleanup();

        Self::spawn_server(&mut server);

        // also explicitly establish the client connection

        Runner {
            clients: vec![],
            server,
        }
    }

    // Spawn the server entity to establish connection, for internal use of Runner only.
    fn spawn_server(server_app: &mut App) {
        // explicitly build the server here rather than delegating it to startup
        let server = server_app
            .world_mut()
            .spawn((DeltaManager::default(), RawServer))
            .id();

        server_app.world_mut().trigger(Start { entity: server });
        // We need to explicitly add Started to the server because Linked -> Started
        // is not implemented for crossbeam.
        //
        // This is potentially related to https://github.com/cBournhonesque/lightyear/discussions/1432
        server_app
            .world_mut()
            .commands()
            .entity(server)
            .insert(Started);
    }

    /// Create a new client to connect to the server
    pub fn add_client(&mut self) {
        let mut client = App::new();
        let (client_io, server_io) = CrossbeamIo::new_pair();

        // create the client's counterpart on the server side
        let server_world = self.server.world_mut();

        // a unique fake peer addr to distinguish the clients
        let peer_addr = PeerAddr(SocketAddr::new(
            Ipv4Addr::LOCALHOST.into(),
            1000 + self.clients.len() as u16,
        ));

        let client_of = server_world
            .spawn((
                Link::new(None),
                peer_addr,
                // ping manager
                PingManager::new(PingConfig {
                    ping_interval: Duration::default(),
                }),
                // We need to explicitly establish the crossbeam connection
                server_io,
            ))
            .id();

        let server = server_world
            .query_filtered::<Entity, With<RawServer>>()
            .single(server_world)
            .expect("The server app should have a single RawServer component.");

        server_world
            .commands()
            .entity(client_of)
            .insert((LinkOf { server }, ClientOf));

        // Explicitly trigger Start on the ClientOf entity
        server_world.commands().trigger(Start { entity: client_of });

        // build and keep track of the client
        build_client(&mut client, TestIo(client_io));
        client.finish();
        client.cleanup();

        self.clients.push(client);
    }

    /// Advance both apps by one frame, server first.
    pub fn step(&mut self) {
        trace!("client update");
        for client in &mut self.clients {
            client.update();
        }
        trace!("end client update");
        trace!("server update");
        self.server.update();
        trace!("end server update");
        // std::thread::sleep(Duration::from_secs_f32(1.0 / FIXED_TIMESTEP_HZ as f32));
    }

    /// Advance both apps until a given condition is set or the given frame limit is reached
    ///
    /// Return whether the condition is met
    pub fn step_until<F: FnMut(&[App], &App) -> bool>(
        &mut self,
        max_frames: usize,
        mut condition: F,
    ) -> bool {
        if condition(&self.clients, &self.server) {
            return true;
        }
        for _ in 0..max_frames {
            self.step();
            if condition(&self.clients, &self.server) {
                return true;
            }
        }
        false
    }

    #[allow(dead_code, missing_docs)]
    pub fn get_server_resource<R: Resource>(&mut self) -> Option<&R> {
        let world = self.server.world_mut();
        world.get_resource::<R>()
    }

    #[allow(dead_code, missing_docs)]
    pub fn get_client_resource<R: Resource>(&mut self, i: usize) -> Option<&R> {
        let world = self.clients[i].world_mut();
        world.get_resource::<R>()
    }

    #[allow(dead_code, missing_docs)]
    pub fn get_server_resource_mut<'a, R: Resource>(&'a mut self) -> Option<Mut<'a, R>> {
        let world = self.server.world_mut();
        world.get_resource_mut::<R>()
    }

    #[allow(dead_code, missing_docs)]
    pub fn get_client_resource_mut<'a, R: Resource>(&'a mut self, i: usize) -> Option<Mut<'a, R>> {
        let world = self.clients[i].world_mut();
        world.get_resource_mut::<R>()
    }

    #[allow(dead_code, missing_docs)]
    pub fn client_world(&mut self, i: usize) -> &mut World {
        self.clients[i].world_mut()
    }
}

/// Inject the game server logic using the given IO channels
fn build_server(app: &mut App, cfg: GameConfig) {
    app.add_plugins(ServerPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
    })
    .insert_resource(cfg)
    .add_plugins(ProtocolPlugin)
    .add_systems(PostStartup, debug_link_entities)
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
    )
    .insert_resource(TimeUpdateStrategy::ManualDuration(FIXED_FRAME_DURATION));

    crate::build_app(app);
}

#[allow(dead_code)]
fn debug_replication_state(senders: Query<(Entity, &ReplicationState)>) {
    for (entity, state) in senders.iter() {
        error!("Replication state added to {entity:?}: {state:?}");
    }
}

#[allow(dead_code)]
fn debug_replication_sender(
    senders: Query<
        (Entity, &ReplicationSender, Option<&ReplicationState>),
        Added<ReplicationSender>,
    >,
) {
    for (entity, sender, state) in senders.iter() {
        error!(
            "ReplicationSender added to {:?}: {:?} state: {state:?}",
            entity, sender
        );
    }
}

fn debug_link_entities(link_query: Query<(Entity, &LinkOf)>, world: &World) {
    for (link_entity, link_of) in link_query.iter() {
        eprintln!(
            "=== Link entity {:?} (parent: {:?}) ===",
            link_entity, link_of
        );

        // Print all component names on this entity
        let entity_ref = world.entity(link_entity);
        let archetype = entity_ref.archetype();
        for component_id in archetype.components() {
            if let Some(info) = world.components().get_info(*component_id) {
                eprintln!("  - {}", info.name());
            }
        }
    }
}

#[allow(dead_code)]
fn debug_link_identity(
    links: Query<(Entity, &RemoteId, Option<&PeerAddr>), (With<LinkOf>, With<Connected>)>,
) {
    for (entity, remote_id, peer_addr) in links.iter() {
        info!(
            "Link {:?}: RemoteId={:?}, PeerAddr={:?}",
            entity, remote_id, peer_addr
        );
    }
}

#[allow(dead_code)]
fn debug_server_state(
    holds: Query<(Entity, &ControlledBy, &Replicate), With<Hold>>,
    states: Query<(Entity, &SharedGameState, &ControlledBy, &Replicate)>,
) {
    for (entity, controlled_by, replicate) in holds {
        info!(
            "Hold {entity} owned by {} replicated to {replicate:?}",
            controlled_by.owner,
        );
    }
    for (entity, state, controlled_by, replicate) in states {
        info!(
            "State {entity} owned by {} replicated to {replicate:?}: {:?}",
            controlled_by.owner, state
        );
    }
}

/// Inject the test client logic using the given IO channels
fn build_client(app: &mut App, io: TestIo) {
    use lightyear::prelude::client::*;

    app.add_plugins(TestClientPlugin)
        .insert_resource(io)
        .add_plugins(ClientPlugins {
            tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
        })
        .insert_resource(TimeUpdateStrategy::ManualDuration(FIXED_FRAME_DURATION))
        .add_plugins(ProtocolPlugin);
}

fn spawn_client(mut commands: Commands, io: Res<TestIo>) {
    let client = commands
        .spawn((
            Client::default(),
            RawClient,
            Link::new(None),
            io.0.clone(),
            ReplicationReceiver::default(),
        ))
        .id();
    commands.trigger(Connect { entity: client });
    info!("Initiating connection via crossbeam");
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

fn send_heartbeat(
    mut sender: Single<&mut MessageSender<Heartbeat>, (With<Client>, With<Connected>)>,
) {
    trace!("Sending heartbeat");
    sender.send::<Heartbeat>(Heartbeat);
}

pub fn compare_test_state(
    active: Option<Single<&Tetromino, With<Active>>>,
    next: Option<Single<&Tetromino, With<Next>>>,
    hold: Option<Single<&Tetromino, With<Hold>>>,
    hold_: Query<&Hold>,
    obstacles: Query<&Block, With<Obstacle>>,
    state: Query<&SharedGameState>,
    time: Res<Time<Fixed>>,
    verdict: Option<Res<TestVerdict>>,
    mut test_case: ResMut<TestCase>,
    mut commands: Commands,
) {
    if test_case.recording.snapshots.is_empty() || verdict.is_some() {
        return;
    }

    let Ok(state) = state.single() else {
        trace!("Haven't received a game state yet!");
        trace!("{:?}", state.single());
        trace!("{:?}", active.as_deref());
        trace!("# of replicated hold components: {:?}", hold_.iter().len());
        return;
    };

    let first_time = test_case.start_time.is_none();
    let time_since_connect = *test_case.start_time.get_or_insert_with(|| time.elapsed());
    let t_actual = time.elapsed() - time_since_connect;

    if first_time {
        warn!("Established connection at {:?}", time.elapsed());
    }

    let mut obstacles = obstacles.iter().copied().collect::<Vec<Block>>();
    obstacles.sort_by_key(|b| b.cell);

    let actual = Snapshot {
        active: active.map(|s| **s),
        next: next.map(|s| **s),
        hold: hold.map(|s| **s),
        obstacles,
        hard_drop: state.hard_drop,
        manual_gravity: state.manual_drop_gravity,
        score: state.score(),
        lines_cleared: state.lines_cleared,
        level: state.level(),
    };

    let actual = if test_case.ignore_score {
        Snapshot {
            score: 0,
            lines_cleared: 0,
            level: 0,
            ..actual
        }
    } else {
        actual
    };

    trace!("Recorded new snapshot {actual:?}");

    // do a linear scan until we match a state
    let Some((skipped, _)) = test_case.recording.snapshots.iter().enumerate().take(MAX_STATE_LOOKAHEAD).find(|(i, (t_expected, expected))| {
        if actual == *expected {
                if *i > 0 && *t_expected >= t_actual + FIXED_FRAME_DURATION {
                    info!(
                        "Possible jitter at time {t_actual:?} but the state at time {t_expected:?} matches. Skipped {i} states"
                    );
                }
            true
        } else {
            false
        }
    }) else {
        if let Some((_, expected))= test_case.recording.snapshots.front() {
        warn!(r#"The states diverge at time {t_actual:?} and fast-forward is not possible.
Actual state:
{actual:?}
Expected state:
{expected:?}
Next state:
{:?}
"#, test_case.recording.snapshots.get(1));
        }
        test_case.mismatches += 1;
        if test_case.mismatches > MAX_STATE_MISMATCHES {
            commands.insert_resource(TestVerdict(false));
        }
        return;
    };

    // drop all the states we skipped.
    //
    // using split_off because truncate_front is not stable yet.
    test_case.recording.snapshots = test_case.recording.snapshots.split_off(skipped);
}

#[allow(dead_code)]
fn debug_client_entities(
    client_entities: Query<Entity, With<Client>>,
    link_entities: Query<
        (
            Entity,
            Option<&LinkOf>,
            Option<&ReplicationReceiver>,
            Option<&MessageManager>,
            Option<&Connected>,
        ),
        With<Link>,
    >,
) {
    for e in client_entities.iter() {
        info!("Client entity: {:?}", e);
    }
    for (entity, link_of, rep_recv, msg_mgr, connected) in link_entities.iter() {
        info!(
            "Link {:?} -> {:?} | ReplicationReceiver: {} | MessageManager: {} | Connected: {}",
            entity,
            link_of.map(|l| l.server),
            rep_recv.is_some(),
            msg_mgr.is_some(),
            connected.is_some(),
        );
    }
}

fn on_client_linked(trigger: On<Add, Linked>, mut commands: Commands) {
    trace!("CrossbeamIo client Linked! Adding Connected");
    commands
        .entity(trigger.entity)
        .insert((Connected, RemoteId(PeerId::Server)));
}

fn send_input(
    mut test_case: ResMut<TestCase>,
    time: Res<Time<Fixed>>,
    mut commands: Commands,
    verdict: Option<Res<TestVerdict>>,
    mut sender: Single<&mut MessageSender<Inputs>, (With<Client>, With<Connected>)>,
) {
    use super::rr::KeyCode::*;

    if verdict.is_some() {
        return;
    }

    let Some(start_time) = test_case.start_time else {
        return;
    };
    let time_since_connect = time.elapsed() - start_time;

    if let Some(event) = test_case
        .recording
        .events
        .pop_front_if(|event| event.time <= time_since_connect)
    {
        let mut inputs = Inputs::default();
        for key in &event.just_pressed {
            inputs.0.insert(match key {
                ArrowDown => Input::Down,
                ArrowLeft => Input::Left,
                ArrowRight => Input::Right,
                ArrowUp => Input::Rotate,
                Escape => Input::EndGame,
                KeyZ => Input::HardDrop,
                KeyX => Input::Hold,
                Space => Input::Rotate,
            });
        }

        sender.send::<InputChannel>(inputs);
    }

    if test_case.recording.events.is_empty() {
        info!("Time since connection: {:?}", time_since_connect);

        if test_case.mismatches <= MAX_STATE_MISMATCHES {
            info!("Passed with {} mismatched states", test_case.mismatches);
            commands.insert_resource(TestVerdict(true))
        } else {
            error!("Failed with {} mismatched states", test_case.mismatches);
            commands.insert_resource(TestVerdict(false))
        }
    }
}

struct TestClientPlugin;

impl Plugin for TestClientPlugin {
    fn build(&self, app: &mut App) {
        use common::*;

        app.add_systems(Startup, spawn_client)
            .add_systems(
                Update,
                (
                    // debug_client_entities.run_if(on_timer(HEARTBEAT_INTERVAL)),
                    send_heartbeat
                        .run_if(on_timer(HEARTBEAT_INTERVAL))
                        .run_if(can_send_message::<Heartbeat>),
                ),
            )
            .add_systems(Update, send_input)
            .add_systems(FixedPostUpdate, compare_test_state)
            .add_observer(on_client_linked)
            .init_resource::<TestCase>();
    }
}

/// A test case including timing data, inputs, and expected client state
#[derive(Resource, Default)]
pub struct TestCase {
    /// The recording to check
    recording: GameRecording,
    /// The fixed timestep that we established a connection in
    start_time: Option<Duration>,
    /// Number of mismatched states
    mismatches: usize,
    /// Whether this test case should not record score values
    ignore_score: bool,
}

/// Store whether a test passes or not.
#[derive(Resource)]
pub(super) struct TestVerdict(bool);

#[test]
fn test_replication_on_connect() {
    let mut runner = Runner::new(GameConfig {
        bag: config::BagType::Deterministic,
        ..default()
    });

    runner.add_client();

    // Connection should be established in 3 frames
    for _ in 0..3 {
        runner.step();
    }

    let world = runner.client_world(0);
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
        1,
        "There must be exactly one active tetromino"
    );
    assert_eq!(
        world
            .query_filtered::<&Tetromino, With<Next>>()
            .iter(world)
            .len(),
        1,
        "There must be exactly one next tetromino"
    );
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

pub fn run_recorded_test(recording_file: &str, bag: BagType, check_scores: bool) {
    // how many extra frames to give until the test is over
    const GRACE_FRAMES: usize = 10;

    let recording_file: PathBuf = [TEST_DATA_DIR, recording_file].iter().collect();
    let recording: GameRecording = serde_json::from_slice(
        &std::fs::read(recording_file).expect("Cannot read the recording file"),
    )
    .expect("The recording is ill-formatted");

    let max_steps = (recording.events.back().unwrap().time.as_secs_f64()
        / FIXED_FRAME_DURATION.as_secs_f64())
    .ceil() as usize
        + GRACE_FRAMES;

    let mut runner = Runner::new(GameConfig { bag, ..default() });
    runner.add_client();

    let mut test_case = runner
        .client_world(0)
        .get_resource_mut::<TestCase>()
        .unwrap();

    test_case.recording = recording;
    test_case.ignore_score = !check_scores;

    runner.step_until(max_steps, |clients, _server| {
        clients[0].world().get_resource::<TestVerdict>().is_some()
    });

    assert!(
        runner
            .get_client_resource::<TestVerdict>(0)
            .expect("The test did not finish in time")
            .0
    );
}
