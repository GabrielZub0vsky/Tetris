//! Seed fake data into the database if it is empty.

use password_auth::generate_hash;
use sqlx::SqlitePool;

pub async fn seed_if_empty(pool: &SqlitePool) -> sqlx::Result<()> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await?;

    if count > 0 {
        return Ok(());
    }

    let users = [
        ("alice", "password123"),
        ("bob", "password123"),
        ("carol", "password123"),
        ("dave", "password123"),
        ("eve", "password123"),
    ];

    let mut user_ids = Vec::new();
    for (username, password) in &users {
        let hash = generate_hash(password);
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash) VALUES (?, ?) RETURNING id",
        )
        .bind(username)
        .bind(&hash)
        .fetch_one(pool)
        .await?;
        user_ids.push(id);
    }

    // Seed some games between pairs of users.
    let games: &[(usize, usize, &str, &str)] = &[
        (0, 1, "Won", "Lost"),
        (1, 2, "Won", "Lost"),
        (0, 2, "Lost", "Won"),
        (3, 4, "Draw", "Draw"),
        (0, 3, "Won", "Lost"),
        (2, 4, "Lost", "Won"),
        (1, 3, "Won", "Lost"),
        (0, 4, "Won", "Lost"),
    ];

    for (i, j, verdict_i, verdict_j) in games {
        let game_id: i64 =
            sqlx::query_scalar("INSERT INTO games DEFAULT VALUES RETURNING id")
                .fetch_one(pool)
                .await?;

        sqlx::query(
            "INSERT INTO game_participants (game_id, user_id, verdict) VALUES (?, ?, ?)",
        )
        .bind(game_id)
        .bind(user_ids[*i])
        .bind(*verdict_i)
        .execute(pool)
        .await?;

        sqlx::query(
            "INSERT INTO game_participants (game_id, user_id, verdict) VALUES (?, ?, ?)",
        )
        .bind(game_id)
        .bind(user_ids[*j])
        .bind(*verdict_j)
        .execute(pool)
        .await?;
    }

    Ok(())
}
