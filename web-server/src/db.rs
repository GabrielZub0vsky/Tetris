//! Database models and queries.

use sqlx::{SqlitePool, sqlite::SqliteQueryResult};

/// A lobby waiting for or currently running a game.
#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct Lobby {
    pub id: i64,
    pub host_user_id: i64,
    pub max_players: i64,
    pub port: i64,
    /// OS PID of the spawned game server process, if any.
    pub pid: Option<i64>,
    pub status: String,
    pub created_at: String,
    pub last_activity: String,
}

/// Lobby entry for the lobby list view (includes member count and host name).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LobbyEntry {
    pub id: i64,
    pub host_username: String,
    pub max_players: i64,
    pub port: i64,
    pub status: String,
    pub created_at: String,
    pub member_count: i64,
}

/// A lobby member (user id + username) for the detail view.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LobbyMember {
    pub id: i64,
    pub username: String,
}

/// A registered user.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub created_at: String,
    pub last_seen_at: Option<String>,
}

/// A game record.
#[allow(dead_code)]
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Game {
    pub id: i64,
    pub played_at: String,
}

/// A user's participation in a game.
#[allow(dead_code)]
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GameParticipant {
    pub id: i64,
    pub game_id: i64,
    pub user_id: i64,
    pub verdict: String,
}

/// A game entry for display on a user's profile page.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GameEntry {
    pub game_id: i64,
    pub played_at: String,
    pub verdict: String,
    pub opponent_username: Option<String>,
}

/// Initialize the database schema.
pub async fn init_schema(pool: &SqlitePool) -> sqlx::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_seen_at TEXT
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS games (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            played_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS game_participants (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            game_id INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            verdict TEXT NOT NULL CHECK(verdict IN ('Won', 'Lost', 'Draw')),
            score INTEGER,
            eliminated_at_seconds REAL,
            played_seconds REAL
        )",
    )
    .execute(pool)
    .await?;

    // Idempotent migrations for older DBs missing the new columns.
    for col in [
        "ALTER TABLE game_participants ADD COLUMN score INTEGER",
        "ALTER TABLE game_participants ADD COLUMN eliminated_at_seconds REAL",
        "ALTER TABLE game_participants ADD COLUMN played_seconds REAL",
    ] {
        let _ = sqlx::query(col).execute(pool).await;
    }

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS lobbies (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            host_user_id INTEGER NOT NULL REFERENCES users(id),
            max_players INTEGER NOT NULL CHECK(max_players BETWEEN 1 AND 3),
            port INTEGER NOT NULL UNIQUE,
            pid INTEGER,
            status TEXT NOT NULL DEFAULT 'waiting'
                CHECK(status IN ('waiting', 'running', 'finished')),
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_activity TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS lobby_members (
            lobby_id INTEGER NOT NULL REFERENCES lobbies(id) ON DELETE CASCADE,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            PRIMARY KEY (lobby_id, user_id)
        )",
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Create a lobby; returns the new lobby id.
pub async fn create_lobby(
    pool: &SqlitePool,
    host_user_id: i64,
    max_players: i64,
    port: i64,
) -> sqlx::Result<i64> {
    let r: SqliteQueryResult =
        sqlx::query("INSERT INTO lobbies (host_user_id, max_players, port) VALUES (?, ?, ?)")
            .bind(host_user_id)
            .bind(max_players)
            .bind(port)
            .execute(pool)
            .await?;
    Ok(r.last_insert_rowid())
}

/// Store the OS PID for a lobby's game server process.
pub async fn set_lobby_pid(pool: &SqlitePool, lobby_id: i64, pid: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE lobbies SET pid = ? WHERE id = ?")
        .bind(pid)
        .bind(lobby_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Add a user to a lobby's member list.
pub async fn add_lobby_member(pool: &SqlitePool, lobby_id: i64, user_id: i64) -> sqlx::Result<()> {
    sqlx::query("INSERT OR IGNORE INTO lobby_members (lobby_id, user_id) VALUES (?, ?)")
        .bind(lobby_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Remove a user from a lobby.
pub async fn remove_lobby_member(
    pool: &SqlitePool,
    lobby_id: i64,
    user_id: i64,
) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM lobby_members WHERE lobby_id = ? AND user_id = ?")
        .bind(lobby_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Fetch a lobby by id.
pub async fn get_lobby(pool: &SqlitePool, lobby_id: i64) -> sqlx::Result<Option<Lobby>> {
    sqlx::query_as::<_, Lobby>("SELECT * FROM lobbies WHERE id = ?")
        .bind(lobby_id)
        .fetch_optional(pool)
        .await
}

/// List all lobbies that are waiting or running, with member counts.
pub async fn list_active_lobbies(pool: &SqlitePool) -> sqlx::Result<Vec<LobbyEntry>> {
    sqlx::query_as::<_, LobbyEntry>(
        "SELECT l.id, u.username AS host_username, l.max_players, l.port,
                l.status, l.created_at,
                COUNT(lm.user_id) AS member_count
         FROM lobbies l
         JOIN users u ON u.id = l.host_user_id
         LEFT JOIN lobby_members lm ON lm.lobby_id = l.id
         WHERE l.status IN ('waiting', 'running')
         GROUP BY l.id
         ORDER BY l.created_at DESC",
    )
    .fetch_all(pool)
    .await
}

/// Return the active lobby the given user is currently in, if any.
pub async fn get_user_current_lobby(
    pool: &SqlitePool,
    user_id: i64,
) -> sqlx::Result<Option<Lobby>> {
    sqlx::query_as::<_, Lobby>(
        "SELECT l.* FROM lobbies l
         JOIN lobby_members lm ON lm.lobby_id = l.id
         WHERE lm.user_id = ? AND l.status IN ('waiting', 'running')
         LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
}

/// Count current members in a lobby.
pub async fn get_lobby_member_count(pool: &SqlitePool, lobby_id: i64) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM lobby_members WHERE lobby_id = ?")
        .bind(lobby_id)
        .fetch_one(pool)
        .await
}

/// Fetch all members (id + username) for a lobby.
pub async fn get_lobby_members(pool: &SqlitePool, lobby_id: i64) -> sqlx::Result<Vec<LobbyMember>> {
    sqlx::query_as::<_, LobbyMember>(
        "SELECT u.id, u.username FROM lobby_members lm
         JOIN users u ON u.id = lm.user_id
         WHERE lm.lobby_id = ?",
    )
    .bind(lobby_id)
    .fetch_all(pool)
    .await
}

/// Delete a lobby (cascades to lobby_members).
pub async fn delete_lobby(pool: &SqlitePool, lobby_id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM lobbies WHERE id = ?")
        .bind(lobby_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Update a lobby's status field.
pub async fn set_lobby_status(pool: &SqlitePool, lobby_id: i64, status: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE lobbies SET status = ? WHERE id = ?")
        .bind(status)
        .bind(lobby_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Refresh a lobby's last_activity timestamp to now.
pub async fn touch_lobby(pool: &SqlitePool, lobby_id: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE lobbies SET last_activity = datetime('now') WHERE id = ?")
        .bind(lobby_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Return lobbies that have had no activity for more than `minutes` minutes.
pub async fn get_stale_lobbies(pool: &SqlitePool, minutes: i64) -> sqlx::Result<Vec<Lobby>> {
    sqlx::query_as::<_, Lobby>(
        "SELECT * FROM lobbies
         WHERE status IN ('waiting', 'running')
           AND last_activity <= datetime('now', printf('-%d minutes', ?))",
    )
    .bind(minutes)
    .fetch_all(pool)
    .await
}

/// Return all active lobbies (used for startup cleanup).
pub async fn get_all_active_lobbies(pool: &SqlitePool) -> sqlx::Result<Vec<Lobby>> {
    sqlx::query_as::<_, Lobby>("SELECT * FROM lobbies WHERE status IN ('waiting', 'running')")
        .fetch_all(pool)
        .await
}

/// Fetch a user by username.
pub async fn get_user_by_username(pool: &SqlitePool, username: &str) -> sqlx::Result<Option<User>> {
    sqlx::query_as::<_, User>("SELECT * FROM users WHERE username = ?")
        .bind(username)
        .fetch_optional(pool)
        .await
}

/// Fetch a user by ID.
pub async fn get_user_by_id(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<User>> {
    sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// Fetch all users ordered by username.
pub async fn list_users(pool: &SqlitePool) -> sqlx::Result<Vec<User>> {
    sqlx::query_as::<_, User>("SELECT * FROM users ORDER BY username")
        .fetch_all(pool)
        .await
}

/// Fetch users seen within the last 5 minutes.
pub async fn list_online_users(pool: &SqlitePool) -> sqlx::Result<Vec<User>> {
    sqlx::query_as::<_, User>(
        "SELECT * FROM users
         WHERE last_seen_at >= datetime('now', '-5 minutes')
         ORDER BY last_seen_at DESC",
    )
    .fetch_all(pool)
    .await
}

/// Insert a new user. Returns the new user's ID.
pub async fn create_user(
    pool: &SqlitePool,
    username: &str,
    password_hash: &str,
) -> sqlx::Result<i64> {
    let result: SqliteQueryResult =
        sqlx::query("INSERT INTO users (username, password_hash) VALUES (?, ?)")
            .bind(username)
            .bind(password_hash)
            .execute(pool)
            .await?;
    Ok(result.last_insert_rowid())
}

/// Update a user's last_seen_at to now.
pub async fn touch_user(pool: &SqlitePool, user_id: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE users SET last_seen_at = datetime('now') WHERE id = ?")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Fetch game history for a user with opponent info.
pub async fn get_user_games(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Vec<GameEntry>> {
    sqlx::query_as::<_, GameEntry>(
        "SELECT
            gp.game_id,
            g.played_at,
            gp.verdict,
            opp.username AS opponent_username
         FROM game_participants gp
         JOIN games g ON g.id = gp.game_id
         LEFT JOIN game_participants opp_part
               ON opp_part.game_id = gp.game_id AND opp_part.user_id != gp.user_id
         LEFT JOIN users opp ON opp.id = opp_part.user_id
         WHERE gp.user_id = ?
         ORDER BY g.played_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

/// Record a fresh "Lost" game for a user who quit a running game.
///
/// Used by `post_leave_lobby` when the user clicks Leave while the lobby
/// is in the `running` state. Creates a brand-new `games` row plus a
/// single `game_participants` row marked `Lost`, so the forfeit shows up
/// in their career stats and game history immediately.
pub async fn record_forfeit_loss(pool: &SqlitePool, user_id: i64) -> sqlx::Result<()> {
    let game_id: i64 = sqlx::query_scalar("INSERT INTO games DEFAULT VALUES RETURNING id")
        .fetch_one(pool)
        .await?;
    sqlx::query(
        "INSERT INTO game_participants (game_id, user_id, verdict) VALUES (?, ?, 'Lost')",
    )
    .bind(game_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Career-long aggregate stats for one user.
#[derive(Debug, Clone, Default)]
pub struct CareerStats {
    pub wins: i64,
    pub losses: i64,
    pub draws: i64,
    pub win_pct: f64,
    pub highest_score: i64,
    /// Quickest time (seconds) the user knocked out an opponent
    /// (= min eliminated_at among opponents in games the user won).
    pub fastest_elim_seconds: Option<f64>,
    pub total_play_seconds: f64,
}

/// Compute career stats for one user.
pub async fn get_user_career_stats(pool: &SqlitePool, user_id: i64) -> sqlx::Result<CareerStats> {
    let (wins, losses, draws) = get_user_stats(pool, user_id).await?;
    let total = (wins + losses + draws) as f64;
    let win_pct = if total > 0.0 {
        wins as f64 / total * 100.0
    } else {
        0.0
    };

    let highest_score: Option<i64> =
        sqlx::query_scalar("SELECT MAX(score) FROM game_participants WHERE user_id = ?")
            .bind(user_id)
            .fetch_one(pool)
            .await?;

    let fastest_elim_seconds: Option<f64> = sqlx::query_scalar(
        "SELECT MIN(opp.eliminated_at_seconds)
         FROM game_participants me
         JOIN game_participants opp
           ON opp.game_id = me.game_id AND opp.user_id != me.user_id
         WHERE me.user_id = ? AND me.verdict = 'Won'
           AND opp.eliminated_at_seconds IS NOT NULL",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;

    let total_play_seconds: Option<f64> =
        sqlx::query_scalar("SELECT SUM(played_seconds) FROM game_participants WHERE user_id = ?")
            .bind(user_id)
            .fetch_one(pool)
            .await?;

    Ok(CareerStats {
        wins,
        losses,
        draws,
        win_pct,
        highest_score: highest_score.unwrap_or(0),
        fastest_elim_seconds,
        total_play_seconds: total_play_seconds.unwrap_or(0.0),
    })
}

/// Count wins/losses/draws for a user.
pub async fn get_user_stats(pool: &SqlitePool, user_id: i64) -> sqlx::Result<(i64, i64, i64)> {
    let wins: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM game_participants WHERE user_id = ? AND verdict = 'Won'",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;

    let losses: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM game_participants WHERE user_id = ? AND verdict = 'Lost'",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;

    let draws: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM game_participants WHERE user_id = ? AND verdict = 'Draw'",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;

    Ok((wins, losses, draws))
}
