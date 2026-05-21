//! Writing game results to the SQLite database.

use crate::net::ToDrop;
use crate::{ClientOrder, GameOutcomes, GameTracking, ServerDbConfig};
use bevy::prelude::*;
use common::data::SharedGameState;
use lightyear::prelude::ControlledBy;
use lightyear::prelude::server::ClientOf;

/// Clear tracking state at the start of each game round.
pub fn reset_game_tracking(
    mut client_order: ResMut<ClientOrder>,
    mut outcomes: ResMut<GameOutcomes>,
    mut tracking: ResMut<GameTracking>,
) {
    client_order.order.clear();
    outcomes.outcomes.clear();
    tracking.start = Some(std::time::Instant::now());
    tracking.elim_at.clear();
    tracking.final_scores.clear();
}

/// Snapshot elim time + final score when a client is marked ToDrop.
pub fn snapshot_on_elimination(
    trigger: On<Add, ToDrop>,
    client_of: Query<Entity, With<ClientOf>>,
    states: Query<(&SharedGameState, &ControlledBy)>,
    mut tracking: ResMut<GameTracking>,
) {
    let entity = trigger.entity;
    if !client_of.contains(entity) {
        return;
    }
    let Some(start) = tracking.start else {
        return;
    };
    if tracking.elim_at.contains_key(&entity) {
        return;
    }
    tracking
        .elim_at
        .insert(entity, start.elapsed().as_secs_f64());
    for (state, controlled) in &states {
        if controlled.owner == entity {
            tracking.final_scores.insert(entity, state.score);
            break;
        }
    }
}

/// Write the completed game result to the database.
///
/// Runs on OnExit(Running). Skips silently if no ServerDbConfig or empty player list.
pub fn write_game_result_to_db(
    db_cfg: Option<Res<ServerDbConfig>>,
    client_order: Res<ClientOrder>,
    outcomes: Res<GameOutcomes>,
    tracking: Res<GameTracking>,
) {
    let Some(db_cfg) = db_cfg else { return };
    if db_cfg.db_path.is_empty() || db_cfg.player_ids.is_empty() {
        return;
    }

    let conn = match rusqlite::Connection::open(&db_cfg.db_path) {
        Ok(c) => c,
        Err(e) => {
            error!("DB open failed: {e}");
            return;
        }
    };

    if let Err(e) = conn.execute("INSERT INTO games (played_at) VALUES (datetime('now'))", []) {
        error!("Failed to insert game record: {e}");
        return;
    }
    let game_id = conn.last_insert_rowid();

    let game_duration = tracking
        .start
        .map(|s| s.elapsed().as_secs_f64())
        .unwrap_or(0.0);

    for (i, &entity) in client_order.order.iter().enumerate() {
        let Some(&user_id) = db_cfg.player_ids.get(i) else {
            continue;
        };
        let verdict = outcomes
            .outcomes
            .get(&entity)
            .map(String::as_str)
            .unwrap_or("Lost");
        let score = tracking.final_scores.get(&entity).copied().unwrap_or(0) as i64;
        let elim_at = tracking.elim_at.get(&entity).copied();
        let played = elim_at.unwrap_or(game_duration);

        if let Err(e) = conn.execute(
            "INSERT INTO game_participants
                (game_id, user_id, verdict, score, eliminated_at_seconds, played_seconds)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![game_id, user_id, verdict, score, elim_at, played],
        ) {
            error!("Failed to insert participant (user_id={user_id}): {e}");
        }
    }

    info!("Game result written to DB (game_id={game_id})");
}
