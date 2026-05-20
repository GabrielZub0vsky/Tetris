//! Writing game results to the SQLite database.

use crate::{ClientOrder, GameOutcomes, ServerDbConfig};
use bevy::prelude::*;

/// Clear tracking state at the start of each game round.
pub fn reset_game_tracking(
    mut client_order: ResMut<ClientOrder>,
    mut outcomes: ResMut<GameOutcomes>,
) {
    client_order.order.clear();
    outcomes.outcomes.clear();
}

/// Write the completed game result to the database.
///
/// Runs on OnExit(Running). Skips silently if no ServerDbConfig or empty player list.
pub fn write_game_result_to_db(
    db_cfg: Option<Res<ServerDbConfig>>,
    client_order: Res<ClientOrder>,
    outcomes: Res<GameOutcomes>,
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

    for (i, &entity) in client_order.order.iter().enumerate() {
        let Some(&user_id) = db_cfg.player_ids.get(i) else {
            continue;
        };
        let verdict = outcomes
            .outcomes
            .get(&entity)
            .map(String::as_str)
            .unwrap_or("Lost");
        if let Err(e) = conn.execute(
            "INSERT INTO game_participants (game_id, user_id, verdict) VALUES (?1, ?2, ?3)",
            rusqlite::params![game_id, user_id, verdict],
        ) {
            error!("Failed to insert participant (user_id={user_id}): {e}");
        }
    }

    info!("Game result written to DB (game_id={game_id})");
}
