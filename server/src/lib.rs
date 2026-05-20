//! Server support library.

// these two flags are allowed because these warnings are triggered by many
// systems.
#![allow(clippy::type_complexity)]
#![allow(clippy::too_many_arguments)]
#![allow(missing_docs)]

use std::collections::HashMap;

use bevy::app::App;
use bevy::ecs::entity::Entity;
use bevy::ecs::query::{QueryData, QueryFilter};
use bevy::ecs::system::Query;
use bevy::prelude::{Resource, States};
use bevy::state::app::StatesPlugin;
use common::*;
use lightyear::prelude::Lifetime::SessionBased;
use lightyear::prelude::{ControlledBy, NetworkTarget, RemoteId, Replicate};

use crate::score::{send_garbage, update_score};

pub mod game_logic;
pub mod net;
pub mod record;
pub mod score;

#[cfg(test)]
pub mod tests;

/// Server-level game state: waiting for players vs. actively playing.
#[derive(States, Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum ServerState {
    #[default]
    Pending,
    Running,
}

/// DB path and ordered player IDs from the web server config.
#[derive(Resource)]
pub struct ServerDbConfig {
    /// Path to tetris.db
    pub db_path: String,
    /// DB user IDs in lobby join order
    pub player_ids: Vec<i64>,
}

/// Tracks the order clients connected this game round.
#[derive(Resource, Default)]
pub struct ClientOrder {
    pub order: Vec<Entity>,
}

/// Maps client entity to their game verdict.
#[derive(Resource, Default)]
pub struct GameOutcomes {
    pub outcomes: HashMap<Entity, String>,
}

/// Inject the systems and plugins for this game into the app.
pub fn build_app(app: &mut App) {
    use bevy::prelude::*;
    use board::*;
    use game_logic::*;
    use net::spawn_client_game_states;

    app.add_plugins(StatesPlugin)
        .init_state::<ServerState>()
        .init_resource::<ClientOrder>()
        .init_resource::<GameOutcomes>()
        .add_systems(
            FixedUpdate,
            (
                gravity,
                deactivate_if_stuck,
                delete_full_lines,
                spawn_next_tetromino,
                check_winner,
            )
                .chain()
                .in_set(Game)
                .run_if(in_state(ServerState::Running)),
        )
        .add_systems(
            OnEnter(ServerState::Running),
            (spawn_client_game_states, record::reset_game_tracking),
        )
        .add_systems(
            OnExit(ServerState::Running),
            record::write_game_result_to_db,
        )
        .add_observer(handle_user_input)
        .add_observer(swap_hold)
        .add_observer(update_score)
        .add_observer(send_garbage)
        .add_observer(update_hard_drop)
        .add_observer(game_over_on_esc)
        .add_observer(disconnect_on_game_over);
}

/// Convert given query to a hash map indexed by the owner of each query data.
/// Assume that there is at most one entity per owner.
pub fn build_per_client_table<'a, C: QueryData, F: QueryFilter>(
    query: Query<'a, 'a, (C, &ControlledBy), F>,
) -> HashMap<Entity, C::Item<'a, 'a>> {
    let mut map_by_client: HashMap<Entity, C::Item<'a, 'a>> = HashMap::new();
    for (item, controlled_by) in query.into_iter() {
        map_by_client.insert(controlled_by.owner, item);
    }
    map_by_client
}
/// Convert given query to a hash map indexed by the owner of each query data item.
pub fn build_per_client_lists<'a, C: QueryData, F: QueryFilter>(
    query: Query<'a, 'a, (C, &ControlledBy), F>,
) -> HashMap<Entity, Vec<C::Item<'a, 'a>>> {
    let mut map_by_client: HashMap<Entity, Vec<C::Item<'a, 'a>>> = HashMap::new();
    for (item, controlled_by) in query.into_iter() {
        map_by_client
            .entry(controlled_by.owner)
            .or_default()
            .push(item);
    }
    map_by_client
}

/// Take out the query elements controlled by given entity.
pub fn filter_controlled_by<'a, C: QueryData, F: QueryFilter>(
    query: Query<'a, 'a, (C, &ControlledBy), F>,
    owner: Entity,
) -> impl Iterator<Item = C::Item<'a, 'a>> {
    let mut items = vec![];
    for (item, controlled_by) in query.into_iter() {
        if controlled_by.owner == owner {
            items.push(item);
        }
    }
    items.into_iter()
}
/// Take out the single query element controlled by given entity.
pub fn take_controlled_by<'a, C: QueryData, F: QueryFilter>(
    query: Query<'a, 'a, (C, &ControlledBy), F>,
    owner: Entity,
) -> Option<C::Item<'a, 'a>> {
    let mut items = filter_controlled_by(query, owner);
    items.next()
}

/// Create the owner info for the client to inject into an entity
pub fn controlled_by(client: Entity) -> ControlledBy {
    ControlledBy {
        owner: client,
        lifetime: SessionBased,
    }
}

/// Create replication info for replicating to a single client
pub fn replicate_to<F: QueryFilter>(client: Entity, peer_ids: &Query<&RemoteId, F>) -> Replicate {
    let client_id = peer_ids
        .get(client)
        .expect("Client entity does not have a RemoteId");
    Replicate::to_clients(NetworkTarget::Single(**client_id))
}
