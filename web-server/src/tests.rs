//! Integration tests for the web server.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{
    Router,
    routing::{get, post},
};
use axum_login::{AuthManagerLayerBuilder, login_required};
use password_auth::generate_hash;
use sqlx::SqlitePool;
use tower::util::ServiceExt;
use tower_sessions::SessionManagerLayer;
use tower_sessions_sqlx_store::SqliteStore;

use crate::{auth, build_template_env, db, routes::AppState};

async fn build_test_app() -> (Router, SqlitePool) {
    let (app, pool, _state) = build_test_app_with_state().await;
    (app, pool)
}

async fn build_test_app_with_state() -> (Router, SqlitePool, AppState) {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    db::init_schema(&pool).await.unwrap();

    let session_store = SqliteStore::new(pool.clone());
    session_store.migrate().await.unwrap();
    let session_layer = SessionManagerLayer::new(session_store);

    let backend = auth::Backend::new(pool.clone());
    let auth_layer = AuthManagerLayerBuilder::new(backend, session_layer).build();

    let env = build_template_env();
    let mut initial_ports = std::collections::HashMap::new();
    for p in crate::BASE_GAME_PORT..=crate::MAX_GAME_PORT {
        initial_ports.insert(p, false);
    }
    let state = AppState {
        pool: pool.clone(),
        env: Arc::new(env),
        processes: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        port_pool: std::sync::Arc::new(std::sync::Mutex::new(initial_ports)),
        killed_lobbies: std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
    };

    let protected = Router::new()
        .route(
            "/",
            get(|| async { axum::response::Redirect::to("/lobbies") }),
        )
        .route("/users", get(crate::routes::get_users))
        .route("/users/{id}", get(crate::routes::get_user_detail))
        .route(
            "/lobbies/{id}/status",
            get(crate::routes::get_lobby_status),
        )
        .route("/online", get(crate::routes::get_online))
        .route("/logout", post(crate::routes::post_logout))
        .route_layer(login_required!(auth::Backend, login_url = "/login"));

    let public = Router::new()
        .route(
            "/login",
            get(crate::routes::get_login).post(crate::routes::post_login),
        )
        .route(
            "/signup",
            get(crate::routes::get_signup).post(crate::routes::post_signup),
        );

    let app = Router::new()
        .merge(protected)
        .merge(public)
        .layer(auth_layer)
        .with_state(state.clone());

    (app, pool, state)
}

/// GET /login is a public route — must return 200 OK even without a session.
#[tokio::test]
async fn test_login_page_returns_200() {
    let (app, _pool) = build_test_app().await;

    let response = ServiceExt::<Request<Body>>::oneshot(
        app,
        Request::builder()
            .uri("/login")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

/// GET /signup is a public route — must return 200 OK even without a session.
#[tokio::test]
async fn test_signup_page_returns_200() {
    let (app, _pool) = build_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/signup")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

/// /users is behind login_required! — unauthenticated requests must be
/// bounced with a 307 redirect (handled by the axum-login middleware).
#[tokio::test]
async fn test_users_page_redirects_when_not_logged_in() {
    let (app, _pool) = build_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/users")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
}

/// End-to-end exercise of `db::create_user` + the auth Backend:
/// insert a user with a hashed password, then verify Backend::authenticate
/// returns the same user when given matching credentials.
#[tokio::test]
async fn test_create_user_and_authenticate() {
    let (_app, pool) = build_test_app().await;

    let hash = generate_hash("mypassword");
    let id = db::create_user(&pool, "testuser", &hash).await.unwrap();
    assert!(id > 0);

    let user = db::get_user_by_username(&pool, "testuser")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.username, "testuser");

    let backend = auth::Backend::new(pool.clone());
    use axum_login::AuthnBackend;
    let result = backend
        .authenticate(auth::Credentials {
            username: "testuser".to_string(),
            password: "mypassword".to_string(),
            next: None,
        })
        .await
        .unwrap();
    assert!(result.is_some());
}

/// Backend::authenticate must reject a real user when the supplied password
/// does not match the stored argon2/bcrypt hash. Returns Ok(None), not Err.
#[tokio::test]
async fn test_wrong_password_fails_auth() {
    let (_app, pool) = build_test_app().await;

    let hash = generate_hash("correctpassword");
    db::create_user(&pool, "authuser", &hash).await.unwrap();

    let backend = auth::Backend::new(pool.clone());
    use axum_login::AuthnBackend;
    let result = backend
        .authenticate(auth::Credentials {
            username: "authuser".to_string(),
            password: "wrongpassword".to_string(),
            next: None,
        })
        .await
        .unwrap();
    assert!(result.is_none());
}

/// `users.username` carries a UNIQUE constraint — inserting the same name
/// twice must surface a sqlx error rather than silently succeed.
#[tokio::test]
async fn test_duplicate_username_fails() {
    let (_app, pool) = build_test_app().await;

    let hash = generate_hash("password");
    db::create_user(&pool, "dupuser", &hash).await.unwrap();

    let result = db::create_user(&pool, "dupuser", &hash).await;
    assert!(result.is_err());
}

/// A fresh user with no game_participants rows should report 0/0/0
/// for (wins, losses, draws). Guards against COUNT() over an empty
/// result set returning anything other than zero.
#[tokio::test]
async fn test_user_stats_empty() {
    let (_app, pool) = build_test_app().await;

    let hash = generate_hash("password");
    let id = db::create_user(&pool, "statsuser", &hash).await.unwrap();

    let (wins, losses, draws) = db::get_user_stats(&pool, id).await.unwrap();
    assert_eq!(wins, 0);
    assert_eq!(losses, 0);
    assert_eq!(draws, 0);
}

/// Inserting one game with a Won/Lost pair of participants must:
///   * give the winner +1 wins, 0 losses
///   * give the loser  0 wins, +1 losses
///   * show up in get_user_games() for both players
/// Covers the join between games + game_participants + users used by
/// the user-detail page.
#[tokio::test]
async fn test_game_history_recorded() {
    let (_app, pool) = build_test_app().await;

    let hash = generate_hash("password");
    let id1 = db::create_user(&pool, "player1", &hash).await.unwrap();
    let id2 = db::create_user(&pool, "player2", &hash).await.unwrap();

    let game_id: i64 = sqlx::query_scalar("INSERT INTO games DEFAULT VALUES RETURNING id")
        .fetch_one(&pool)
        .await
        .unwrap();

    sqlx::query("INSERT INTO game_participants (game_id, user_id, verdict) VALUES (?, ?, ?)")
        .bind(game_id)
        .bind(id1)
        .bind("Won")
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query("INSERT INTO game_participants (game_id, user_id, verdict) VALUES (?, ?, ?)")
        .bind(game_id)
        .bind(id2)
        .bind("Lost")
        .execute(&pool)
        .await
        .unwrap();

    let (wins, losses, _) = db::get_user_stats(&pool, id1).await.unwrap();
    assert_eq!(wins, 1);
    assert_eq!(losses, 0);

    let (wins2, losses2, _) = db::get_user_stats(&pool, id2).await.unwrap();
    assert_eq!(wins2, 0);
    assert_eq!(losses2, 1);

    let games = db::get_user_games(&pool, id1).await.unwrap();
    assert_eq!(games.len(), 1);
    assert_eq!(games[0].verdict, "Won");
}

/// A newly-created user has `last_seen_at = NULL` so they must NOT appear
/// in list_online_users() until touch_user() updates that column.
#[tokio::test]
async fn test_online_users_empty_initially() {
    let (_app, pool) = build_test_app().await;

    let hash = generate_hash("password");
    db::create_user(&pool, "onlineuser", &hash).await.unwrap();

    let online = db::list_online_users(&pool).await.unwrap();
    assert!(online.is_empty());
}

// ── Recently added features ──────────────────────────────────────────────────

/// Helper: insert a full game row (two participants) with all stat columns.
async fn insert_game(
    pool: &SqlitePool,
    p1: i64,
    p2: i64,
    p1_verdict: &str,
    p2_verdict: &str,
    p1_score: i64,
    p2_score: i64,
    p1_elim: Option<f64>,
    p2_elim: Option<f64>,
    p1_played: f64,
    p2_played: f64,
) -> i64 {
    let game_id: i64 = sqlx::query_scalar("INSERT INTO games DEFAULT VALUES RETURNING id")
        .fetch_one(pool)
        .await
        .unwrap();
    for (uid, v, sc, el, pl) in [
        (p1, p1_verdict, p1_score, p1_elim, p1_played),
        (p2, p2_verdict, p2_score, p2_elim, p2_played),
    ] {
        sqlx::query(
            "INSERT INTO game_participants
                (game_id, user_id, verdict, score, eliminated_at_seconds, played_seconds)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(game_id)
        .bind(uid)
        .bind(v)
        .bind(sc)
        .bind(el)
        .bind(pl)
        .execute(pool)
        .await
        .unwrap();
    }
    game_id
}

/// CareerStats for a user with no participations should default to all
/// zeros / None — not blow up on MAX/MIN/SUM over zero rows.
#[tokio::test]
async fn test_career_stats_empty_user() {
    let (_app, pool) = build_test_app().await;
    let hash = generate_hash("password");
    let id = db::create_user(&pool, "nobody", &hash).await.unwrap();

    let stats = db::get_user_career_stats(&pool, id).await.unwrap();
    assert_eq!(stats.wins, 0);
    assert_eq!(stats.losses, 0);
    assert_eq!(stats.draws, 0);
    assert_eq!(stats.win_pct, 0.0);
    assert_eq!(stats.highest_score, 0);
    assert!(stats.fastest_elim_seconds.is_none());
    assert_eq!(stats.total_play_seconds, 0.0);
}

/// win_pct = wins / (wins + losses + draws) * 100. Draws count toward the
/// denominator. This test seeds 2 wins, 1 loss, 1 draw → expect exactly 50.0%.
#[tokio::test]
async fn test_career_stats_win_pct() {
    let (_app, pool) = build_test_app().await;
    let hash = generate_hash("password");
    let a = db::create_user(&pool, "a", &hash).await.unwrap();
    let b = db::create_user(&pool, "b", &hash).await.unwrap();

    // a wins 2, loses 1, draws 1 → 50%
    insert_game(
        &pool,
        a,
        b,
        "Won",
        "Lost",
        100,
        50,
        None,
        Some(30.0),
        60.0,
        30.0,
    )
    .await;
    insert_game(
        &pool,
        a,
        b,
        "Won",
        "Lost",
        200,
        80,
        None,
        Some(40.0),
        70.0,
        40.0,
    )
    .await;
    insert_game(
        &pool,
        a,
        b,
        "Lost",
        "Won",
        30,
        150,
        Some(20.0),
        None,
        20.0,
        60.0,
    )
    .await;
    insert_game(&pool, a, b, "Draw", "Draw", 90, 90, None, None, 90.0, 90.0).await;

    let stats = db::get_user_career_stats(&pool, a).await.unwrap();
    assert_eq!(stats.wins, 2);
    assert_eq!(stats.losses, 1);
    assert_eq!(stats.draws, 1);
    assert!((stats.win_pct - 50.0).abs() < 1e-6);
}

/// highest_score = MAX(score) across all of a user's games regardless of
/// verdict — a user can post their personal best in a game they lost.
#[tokio::test]
async fn test_career_stats_highest_score() {
    let (_app, pool) = build_test_app().await;
    let hash = generate_hash("password");
    let a = db::create_user(&pool, "a", &hash).await.unwrap();
    let b = db::create_user(&pool, "b", &hash).await.unwrap();

    insert_game(
        &pool,
        a,
        b,
        "Won",
        "Lost",
        500,
        50,
        None,
        Some(10.0),
        30.0,
        10.0,
    )
    .await;
    insert_game(
        &pool,
        a,
        b,
        "Lost",
        "Won",
        1234,
        50,
        Some(5.0),
        None,
        5.0,
        20.0,
    )
    .await;
    insert_game(
        &pool,
        a,
        b,
        "Won",
        "Lost",
        100,
        50,
        None,
        Some(10.0),
        15.0,
        10.0,
    )
    .await;

    let stats = db::get_user_career_stats(&pool, a).await.unwrap();
    assert_eq!(stats.highest_score, 1234);
}

/// "Fastest KO" is the shortest opponent elimination time across games
/// the user WON. Eliminations from games the user lost must be ignored —
/// otherwise a player who died early would look like a fast killer.
#[tokio::test]
async fn test_career_stats_fastest_elim_only_counts_wins() {
    let (_app, pool) = build_test_app().await;
    let hash = generate_hash("password");
    let a = db::create_user(&pool, "a", &hash).await.unwrap();
    let b = db::create_user(&pool, "b", &hash).await.unwrap();

    // a wins → opponent eliminated at 30s, then a wins again → opp at 12s.
    insert_game(
        &pool,
        a,
        b,
        "Won",
        "Lost",
        100,
        50,
        None,
        Some(30.0),
        30.0,
        30.0,
    )
    .await;
    insert_game(
        &pool,
        a,
        b,
        "Won",
        "Lost",
        100,
        50,
        None,
        Some(12.0),
        12.0,
        12.0,
    )
    .await;
    // Game a LOST: opp elim at 3s — must NOT count for a's fastest KO.
    insert_game(
        &pool,
        a,
        b,
        "Lost",
        "Won",
        5,
        50,
        Some(3.0),
        None,
        3.0,
        25.0,
    )
    .await;

    let stats = db::get_user_career_stats(&pool, a).await.unwrap();
    assert_eq!(stats.fastest_elim_seconds, Some(12.0));
}

/// fastest_elim_seconds must be None for a user who has played but never
/// won — the leaderboard renders an em-dash for None.
#[tokio::test]
async fn test_career_stats_fastest_elim_none_when_no_wins() {
    let (_app, pool) = build_test_app().await;
    let hash = generate_hash("password");
    let a = db::create_user(&pool, "a", &hash).await.unwrap();
    let b = db::create_user(&pool, "b", &hash).await.unwrap();

    insert_game(
        &pool,
        a,
        b,
        "Lost",
        "Won",
        10,
        100,
        Some(5.0),
        None,
        5.0,
        30.0,
    )
    .await;

    let stats = db::get_user_career_stats(&pool, a).await.unwrap();
    assert!(stats.fastest_elim_seconds.is_none());
}

/// total_play_seconds = SUM(played_seconds) across the user's games.
/// Uses a float comparison with small epsilon. Catches accidental MAX/AVG.
#[tokio::test]
async fn test_career_stats_total_play_seconds_sums() {
    let (_app, pool) = build_test_app().await;
    let hash = generate_hash("password");
    let a = db::create_user(&pool, "a", &hash).await.unwrap();
    let b = db::create_user(&pool, "b", &hash).await.unwrap();

    insert_game(
        &pool,
        a,
        b,
        "Won",
        "Lost",
        100,
        50,
        None,
        Some(10.0),
        25.5,
        10.0,
    )
    .await;
    insert_game(
        &pool,
        a,
        b,
        "Lost",
        "Won",
        30,
        150,
        Some(40.0),
        None,
        40.0,
        80.0,
    )
    .await;

    let stats = db::get_user_career_stats(&pool, a).await.unwrap();
    assert!((stats.total_play_seconds - 65.5).abs() < 1e-6);
}

/// A lobby with two members joining (max_players=2) should transition from
/// 'waiting' to 'running'. Mirrors the runtime logic in post_join_lobby /
/// post_create_lobby so the state transition is locked down at the db layer.
#[tokio::test]
async fn test_lobby_full_marks_running() {
    let (_app, pool) = build_test_app().await;
    let hash = generate_hash("password");
    let host = db::create_user(&pool, "host", &hash).await.unwrap();
    let other = db::create_user(&pool, "other", &hash).await.unwrap();

    let lobby_id = db::create_lobby(&pool, host, 2, 1338).await.unwrap();
    db::add_lobby_member(&pool, lobby_id, host).await.unwrap();

    // Before second join: should still be 'waiting'.
    let lobby = db::get_lobby(&pool, lobby_id).await.unwrap().unwrap();
    assert_eq!(lobby.status, "waiting");

    // Simulate post_join_lobby: add member, check count, set status.
    db::add_lobby_member(&pool, lobby_id, other).await.unwrap();
    let count = db::get_lobby_member_count(&pool, lobby_id).await.unwrap();
    assert_eq!(count, 2);
    db::set_lobby_status(&pool, lobby_id, "running")
        .await
        .unwrap();

    let lobby = db::get_lobby(&pool, lobby_id).await.unwrap().unwrap();
    assert_eq!(lobby.status, "running");
}

/// db::set_lobby_status writes the new value back to SQLite and a subsequent
/// get_lobby reads it back. Guards against the CHECK constraint or UPDATE
/// statement silently failing.
#[tokio::test]
async fn test_set_lobby_status_persists() {
    let (_app, pool) = build_test_app().await;
    let hash = generate_hash("password");
    let host = db::create_user(&pool, "host", &hash).await.unwrap();
    let lobby_id = db::create_lobby(&pool, host, 1, 1339).await.unwrap();

    db::set_lobby_status(&pool, lobby_id, "running")
        .await
        .unwrap();
    let lobby = db::get_lobby(&pool, lobby_id).await.unwrap().unwrap();
    assert_eq!(lobby.status, "running");
}

/// Hits /users?sort=wins with a logged-in session and confirms the user
/// with more wins appears earlier in the rendered HTML than the user with
/// fewer wins. Exercises the full path: Query extractor → sort_entries →
/// template render.
#[tokio::test]
async fn test_users_page_sort_by_wins_works() {
    let (app, pool) = build_test_app().await;

    // Sign up via route so we get a logged-in session cookie.
    let response = login_as(&app, "manywins", "password").await;
    let cookie = extract_session_cookie(&response);
    let _ = login_as(&app, "fewwins", "password").await;

    let many = db::get_user_by_username(&pool, "manywins")
        .await
        .unwrap()
        .unwrap()
        .id;
    let few = db::get_user_by_username(&pool, "fewwins")
        .await
        .unwrap()
        .unwrap()
        .id;
    insert_game(
        &pool,
        many,
        few,
        "Won",
        "Lost",
        50,
        10,
        None,
        Some(5.0),
        10.0,
        5.0,
    )
    .await;
    insert_game(
        &pool,
        many,
        few,
        "Won",
        "Lost",
        50,
        10,
        None,
        Some(5.0),
        10.0,
        5.0,
    )
    .await;
    insert_game(
        &pool,
        many,
        few,
        "Won",
        "Lost",
        50,
        10,
        None,
        Some(5.0),
        10.0,
        5.0,
    )
    .await;
    insert_game(
        &pool,
        few,
        many,
        "Won",
        "Lost",
        10,
        5,
        None,
        Some(2.0),
        5.0,
        2.0,
    )
    .await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/users?sort=wins")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = std::str::from_utf8(&body).unwrap();
    // manywins (3 wins) appears before fewwins (1 win) in the HTML.
    let pos_many = html.find("manywins").expect("manywins missing");
    let pos_few = html.find("fewwins").expect("fewwins missing");
    assert!(
        pos_many < pos_few,
        "manywins should appear before fewwins when sorted by wins desc"
    );
}

/// fastest_elim is the only metric that sorts ASCENDING (smaller = faster).
/// This test makes sure the comparator wasn't accidentally written backwards
/// like the other "bigger = better" columns.
#[tokio::test]
async fn test_users_page_sort_by_fastest_elim_ascending() {
    let (app, pool) = build_test_app().await;

    let response = login_as(&app, "fastko", "password").await;
    let cookie = extract_session_cookie(&response);
    let _ = login_as(&app, "slowko", "password").await;

    let fast = db::get_user_by_username(&pool, "fastko")
        .await
        .unwrap()
        .unwrap()
        .id;
    let slow = db::get_user_by_username(&pool, "slowko")
        .await
        .unwrap()
        .unwrap()
        .id;
    // slow KO at 30s
    insert_game(
        &pool,
        slow,
        fast,
        "Won",
        "Lost",
        10,
        5,
        None,
        Some(30.0),
        30.0,
        30.0,
    )
    .await;
    // fast KO at 2s
    insert_game(
        &pool,
        fast,
        slow,
        "Won",
        "Lost",
        10,
        5,
        None,
        Some(2.0),
        2.0,
        2.0,
    )
    .await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/users?sort=fastest_elim")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = std::str::from_utf8(&body).unwrap();
    let pos_fast = html.find("fastko").unwrap();
    let pos_slow = html.find("slowko").unwrap();
    assert!(
        pos_fast < pos_slow,
        "fastko (KO 2s) should sort before slowko (KO 30s) when sort=fastest_elim"
    );
}

/// Pull the session cookie value (`id=...`) out of all Set-Cookie headers.
fn extract_session_cookie(response: &axum::http::Response<Body>) -> String {
    let cookies: Vec<String> = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|h| h.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or(s).to_string())
        .collect();
    assert!(
        !cookies.is_empty(),
        "signup did not set any cookies; status={:?} headers={:?}",
        response.status(),
        response.headers(),
    );
    cookies.join("; ")
}

/// Helper: sign up a user and return the response with set-cookie.
async fn login_as(app: &Router, username: &str, password: &str) -> axum::http::Response<Body> {
    let body = format!("username={username}&password={password}&confirm_password={password}",);
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/signup")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

/// When a signup POST hits the UNIQUE constraint on username, the response
/// must:
///   * still be 200 (render the signup page, not 500)
///   * contain the phrase "already exists"
///   * highlight the conflicting username inside <strong>...</strong>
///   * include a /login?username=NAME deep-link so the user can switch over.
#[tokio::test]
async fn test_signup_duplicate_username_shows_login_link() {
    let (app, _pool) = build_test_app().await;

    // First signup succeeds.
    let _ = login_as(&app, "dupname", "password").await;

    // Second signup with same name returns signup page with duplicate warning.
    let body = format!("username=dupname&password=password&confirm_password=password");
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/signup")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = std::str::from_utf8(&body).unwrap();
    assert!(
        html.contains("already exists"),
        "expected 'already exists' in body: {html}"
    );
    assert!(
        html.contains("/login?username=dupname"),
        "expected /login?username=dupname link in body"
    );
    assert!(
        html.contains("<strong>dupname</strong>"),
        "expected username highlighted with <strong>"
    );
}

/// /login?username=alice should prefill the username input with `alice`
/// so the redirect from the duplicate-signup page lands users on a half-
/// completed login form.
#[tokio::test]
async fn test_login_page_prefills_username_from_query() {
    let (app, _pool) = build_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/login?username=alice")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = std::str::from_utf8(&body).unwrap();
    assert!(
        html.contains(r#"value="alice""#),
        "username input should be prefilled with 'alice': {html}"
    );
}

/// allocate_port must return the numerically lowest free port each time.
/// After releasing a middle port, the next allocation should reuse it
/// before continuing past the highest previously-issued port.
#[tokio::test]
async fn test_port_pool_allocates_lowest_first() {
    use crate::routes::{allocate_port, release_port};
    use std::collections::HashMap;
    use std::sync::Mutex;

    let mut map = HashMap::new();
    for p in crate::BASE_GAME_PORT..=crate::MAX_GAME_PORT {
        map.insert(p, false);
    }
    let pool = Mutex::new(map);

    // Should hand out the lowest port first.
    assert_eq!(allocate_port(&pool), Some(1338));
    assert_eq!(allocate_port(&pool), Some(1339));
    assert_eq!(allocate_port(&pool), Some(1340));

    // Release 1339 → next allocation reuses it before going past 1340.
    release_port(&pool, 1339);
    assert_eq!(allocate_port(&pool), Some(1339));
    assert_eq!(allocate_port(&pool), Some(1341));
}

/// The pool holds exactly 100 ports (1338..=1437). After 100 allocations
/// the next call must return None (so post_create_lobby can show an
/// "at capacity" error). Releasing one port then re-allocates that exact
/// port number.
#[tokio::test]
async fn test_port_pool_exhaustion_returns_none() {
    use crate::routes::{allocate_port, release_port};
    use std::collections::HashMap;
    use std::sync::Mutex;

    let mut map = HashMap::new();
    for p in crate::BASE_GAME_PORT..=crate::MAX_GAME_PORT {
        map.insert(p, false);
    }
    let pool = Mutex::new(map);

    // Drain all 100 ports.
    for _ in 0..100 {
        assert!(allocate_port(&pool).is_some());
    }
    // Pool exhausted.
    assert_eq!(allocate_port(&pool), None);

    // Release one → next allocation succeeds again.
    release_port(&pool, 1400);
    assert_eq!(allocate_port(&pool), Some(1400));
    assert_eq!(allocate_port(&pool), None);
}

/// release_port called with a port that isn't part of the configured pool
/// must NOT insert a new entry or panic — defensive against stale DB rows
/// from a previous code version with a different range.
#[tokio::test]
async fn test_release_port_outside_pool_is_noop() {
    use crate::routes::release_port;
    use std::collections::HashMap;
    use std::sync::Mutex;

    let mut map = HashMap::new();
    map.insert(1338u16, true);
    let pool = Mutex::new(map);

    release_port(&pool, 9999);
    // 1338 stays in use; 9999 not inserted.
    let guard = pool.lock().unwrap();
    assert_eq!(guard.get(&1338), Some(&true));
    assert!(!guard.contains_key(&9999));
}

/// Root path "/" is registered behind login_required! and redirects to
/// /lobbies once authenticated. An unauthenticated hit should produce
/// a redirect (307 / 303 / 302 are all acceptable) rather than 404 or 500.
#[tokio::test]
async fn test_root_redirects_when_not_logged_in() {
    let (app, _pool) = build_test_app().await;

    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    // login_required middleware bounces unauthenticated users → 303 to /login.
    assert!(
        response.status() == StatusCode::TEMPORARY_REDIRECT
            || response.status() == StatusCode::SEE_OTHER
            || response.status() == StatusCode::FOUND,
        "got {:?}",
        response.status()
    );
}

/// `record_forfeit_loss` writes one new game row + one `Lost`
/// participant for the user — guarantees the user's wins/losses jump
/// immediately when they bail on a running game.
#[tokio::test]
async fn test_record_forfeit_loss_adds_lost_record() {
    let (_app, pool) = build_test_app().await;
    let hash = generate_hash("password");
    let user = db::create_user(&pool, "leaver", &hash).await.unwrap();

    let (wins0, losses0, _) = db::get_user_stats(&pool, user).await.unwrap();
    assert_eq!((wins0, losses0), (0, 0));

    db::record_forfeit_loss(&pool, user).await.unwrap();

    let (wins, losses, draws) = db::get_user_stats(&pool, user).await.unwrap();
    assert_eq!(wins, 0);
    assert_eq!(losses, 1, "forfeit must count as a loss");
    assert_eq!(draws, 0);

    // History endpoint should also surface it.
    let games = db::get_user_games(&pool, user).await.unwrap();
    assert_eq!(games.len(), 1);
    assert_eq!(games[0].verdict, "Lost");
}

/// Two back-to-back forfeit calls create two distinct game rows — no
/// accidental deduplication / UPSERT logic in `record_forfeit_loss`.
#[tokio::test]
async fn test_record_forfeit_loss_creates_distinct_games() {
    let (_app, pool) = build_test_app().await;
    let hash = generate_hash("password");
    let user = db::create_user(&pool, "serialleaver", &hash)
        .await
        .unwrap();

    db::record_forfeit_loss(&pool, user).await.unwrap();
    db::record_forfeit_loss(&pool, user).await.unwrap();

    let (_, losses, _) = db::get_user_stats(&pool, user).await.unwrap();
    assert_eq!(losses, 2);
    let games = db::get_user_games(&pool, user).await.unwrap();
    assert_eq!(games.len(), 2);
}

/// `get_user_current_lobby` should return Some for a user in a `waiting`
/// lobby AND a `running` lobby — both states block joining elsewhere.
/// `finished` lobbies must NOT match, so once a game is over the user is
/// free to join another lobby without manual cleanup.
#[tokio::test]
async fn test_get_user_current_lobby_only_matches_active_states() {
    let (_app, pool) = build_test_app().await;
    let hash = generate_hash("password");
    let user = db::create_user(&pool, "u1", &hash).await.unwrap();

    // waiting → matches
    let l1 = db::create_lobby(&pool, user, 2, 1340).await.unwrap();
    db::add_lobby_member(&pool, l1, user).await.unwrap();
    assert!(
        db::get_user_current_lobby(&pool, user)
            .await
            .unwrap()
            .is_some(),
        "waiting lobby should match"
    );

    // running → still matches
    db::set_lobby_status(&pool, l1, "running").await.unwrap();
    assert!(
        db::get_user_current_lobby(&pool, user)
            .await
            .unwrap()
            .is_some(),
        "running lobby should match"
    );

    // finished → no longer matches
    db::set_lobby_status(&pool, l1, "finished").await.unwrap();
    assert!(
        db::get_user_current_lobby(&pool, user)
            .await
            .unwrap()
            .is_none(),
        "finished lobby must NOT count as the user's current lobby"
    );
}

/// After the user's lobby is deleted (e.g. host left, all left,
/// inactivity), the lobby_members cascade frees the user and they can
/// be added to a fresh lobby. Models the "wait for game to end" path.
#[tokio::test]
async fn test_user_free_to_join_after_lobby_destroyed() {
    let (_app, pool) = build_test_app().await;
    let hash = generate_hash("password");
    let user = db::create_user(&pool, "wanderer", &hash).await.unwrap();

    let first = db::create_lobby(&pool, user, 1, 1341).await.unwrap();
    db::add_lobby_member(&pool, first, user).await.unwrap();
    db::set_lobby_status(&pool, first, "running").await.unwrap();
    db::delete_lobby(&pool, first).await.unwrap();

    // ON DELETE CASCADE wipes lobby_members; user is now lobby-free.
    assert!(
        db::get_user_current_lobby(&pool, user)
            .await
            .unwrap()
            .is_none()
    );

    // Joining a new lobby works.
    let second = db::create_lobby(&pool, user, 1, 1342).await.unwrap();
    db::add_lobby_member(&pool, second, user).await.unwrap();
    assert!(
        db::get_user_current_lobby(&pool, user)
            .await
            .unwrap()
            .is_some()
    );
}

/// After forfeiting (leaving) a running lobby — i.e. record_forfeit_loss
/// + remove_lobby_member — the user has no current lobby and can join
/// another one. Models the "leave + accept forfeit" path.
#[tokio::test]
async fn test_user_can_join_after_forfeit_leave() {
    let (_app, pool) = build_test_app().await;
    let hash = generate_hash("password");
    let host = db::create_user(&pool, "hh", &hash).await.unwrap();
    let guest = db::create_user(&pool, "gg", &hash).await.unwrap();

    // Two-player running lobby with both members.
    let lobby = db::create_lobby(&pool, host, 2, 1343).await.unwrap();
    db::add_lobby_member(&pool, lobby, host).await.unwrap();
    db::add_lobby_member(&pool, lobby, guest).await.unwrap();
    db::set_lobby_status(&pool, lobby, "running").await.unwrap();

    // Guest forfeits: record loss + remove.
    db::record_forfeit_loss(&pool, guest).await.unwrap();
    db::remove_lobby_member(&pool, lobby, guest).await.unwrap();

    // Guest is now free; they can be added to a different lobby.
    assert!(
        db::get_user_current_lobby(&pool, guest)
            .await
            .unwrap()
            .is_none()
    );
    let other = db::create_lobby(&pool, guest, 1, 1344).await.unwrap();
    db::add_lobby_member(&pool, other, guest).await.unwrap();
    assert!(
        db::get_user_current_lobby(&pool, guest)
            .await
            .unwrap()
            .is_some()
    );

    // And the loss is on the record.
    let (_, losses, _) = db::get_user_stats(&pool, guest).await.unwrap();
    assert_eq!(losses, 1);
}

/// A non-host member calling `remove_lobby_member` must remove only
/// themselves; the lobby and the other members stay. The host-leaving
/// branch in `post_leave_lobby` then handles full teardown separately.
#[tokio::test]
async fn test_non_host_leave_keeps_lobby_alive() {
    let (_app, pool) = build_test_app().await;
    let hash = generate_hash("password");
    let host = db::create_user(&pool, "lobbyhost", &hash).await.unwrap();
    let guest = db::create_user(&pool, "guest", &hash).await.unwrap();

    let lobby_id = db::create_lobby(&pool, host, 3, 1340).await.unwrap();
    db::add_lobby_member(&pool, lobby_id, host).await.unwrap();
    db::add_lobby_member(&pool, lobby_id, guest).await.unwrap();

    // Guest leaves. Lobby still has the host.
    db::remove_lobby_member(&pool, lobby_id, guest).await.unwrap();
    let count = db::get_lobby_member_count(&pool, lobby_id).await.unwrap();
    assert_eq!(count, 1, "host should still be in lobby after guest leaves");

    // Lobby row itself still exists.
    let lobby = db::get_lobby(&pool, lobby_id).await.unwrap();
    assert!(lobby.is_some(), "lobby row must remain when a guest leaves");
}

/// `GET /lobbies/{id}/status` for a live lobby reports `alive=true` and
/// `killed_reason=null`. Drives the front-end poller's "stay put" path.
#[tokio::test]
async fn test_lobby_status_alive_for_active_lobby() {
    let (app, pool) = build_test_app().await;
    let hash = generate_hash("password");
    let host = db::create_user(&pool, "lobbyhost", &hash).await.unwrap();
    let lobby_id = db::create_lobby(&pool, host, 1, 1338).await.unwrap();

    // Need a session for the protected route.
    let response = login_as(&app, "lobbyhost2", "password").await;
    let cookie = extract_session_cookie(&response);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/lobbies/{lobby_id}/status"))
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["alive"], serde_json::json!(true));
    assert_eq!(json["killed_reason"], serde_json::json!(null));
}

/// After `mark_lobby_killed` has been called and the row is deleted, the
/// status endpoint must report `alive=false` and surface the recorded
/// reason so the JS poller can show a "killed due to X" popup.
#[tokio::test]
async fn test_lobby_status_reports_killed_with_reason() {
    use crate::routes::mark_lobby_killed;

    let (app, pool, state) = build_test_app_with_state().await;
    let hash = generate_hash("password");
    let host = db::create_user(&pool, "lobbyhost", &hash).await.unwrap();
    let lobby_id = db::create_lobby(&pool, host, 1, 1339).await.unwrap();

    let response = login_as(&app, "watcher", "password").await;
    let cookie = extract_session_cookie(&response);

    // Simulate the cleanup task: mark killed, then delete the row.
    mark_lobby_killed(&state.killed_lobbies, lobby_id, "inactivity (test)");
    db::delete_lobby(&pool, lobby_id).await.unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/lobbies/{lobby_id}/status"))
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["alive"], serde_json::json!(false));
    assert_eq!(
        json["killed_reason"],
        serde_json::json!("inactivity (test)")
    );
}

/// `prune_killed` removes entries older than 5 minutes. We fast-forward
/// by injecting an entry with an old `killed_at` instant and verifying
/// it disappears after a prune call.
#[tokio::test]
async fn test_prune_killed_drops_old_entries() {
    use crate::routes::{KilledInfo, prune_killed};
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    let mut map = HashMap::new();
    map.insert(
        1i64,
        KilledInfo {
            reason: "old".into(),
            killed_at: Instant::now() - Duration::from_secs(600),
        },
    );
    map.insert(
        2i64,
        KilledInfo {
            reason: "fresh".into(),
            killed_at: Instant::now(),
        },
    );
    let pool = Mutex::new(map);
    prune_killed(&pool);

    let guard = pool.lock().unwrap();
    assert!(!guard.contains_key(&1), "10-min-old entry should be pruned");
    assert!(guard.contains_key(&2), "fresh entry should survive");
}

/// touch_user updates last_seen_at to `datetime('now')` — afterwards the
/// user should appear in list_online_users(), which filters for activity
/// within the last 5 minutes.
#[tokio::test]
async fn test_touch_user_marks_online() {
    let (_app, pool) = build_test_app().await;

    let hash = generate_hash("password");
    let id = db::create_user(&pool, "touchuser", &hash).await.unwrap();

    db::touch_user(&pool, id).await.unwrap();

    let online = db::list_online_users(&pool).await.unwrap();
    assert_eq!(online.len(), 1);
    assert_eq!(online[0].username, "touchuser");
}
