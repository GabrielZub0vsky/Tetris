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
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    db::init_schema(&pool).await.unwrap();

    let session_store = SqliteStore::new(pool.clone());
    session_store.migrate().await.unwrap();
    let session_layer = SessionManagerLayer::new(session_store);

    let backend = auth::Backend::new(pool.clone());
    let auth_layer = AuthManagerLayerBuilder::new(backend, session_layer).build();

    let env = build_template_env();
    let state = AppState {
        pool: pool.clone(),
        env: Arc::new(env),
        processes: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        next_port: std::sync::Arc::new(std::sync::atomic::AtomicU16::new(1338)),
    };

    let protected = Router::new()
        .route(
            "/",
            get(|| async { axum::response::Redirect::to("/lobbies") }),
        )
        .route("/users", get(crate::routes::get_users))
        .route("/users/{id}", get(crate::routes::get_user_detail))
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
        .with_state(state);

    (app, pool)
}

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

#[tokio::test]
async fn test_duplicate_username_fails() {
    let (_app, pool) = build_test_app().await;

    let hash = generate_hash("password");
    db::create_user(&pool, "dupuser", &hash).await.unwrap();

    let result = db::create_user(&pool, "dupuser", &hash).await;
    assert!(result.is_err());
}

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
