//! Database models and queries.

use sqlx::{SqlitePool, sqlite::SqliteQueryResult};

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
            verdict TEXT NOT NULL CHECK(verdict IN ('Won', 'Lost', 'Draw'))
        )",
    )
    .execute(pool)
    .await?;

    Ok(())
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

/// Count wins/losses/draws for a user.
pub async fn get_user_stats(
    pool: &SqlitePool,
    user_id: i64,
) -> sqlx::Result<(i64, i64, i64)> {
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
