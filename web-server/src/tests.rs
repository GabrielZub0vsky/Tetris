//! Integration tests for the web server.

use std::sync::Arc;

use axum::{Router, routing::{get, post}};
use axum_login::{AuthManagerLayerBuilder, login_required};
use password_auth::generate_hash;
use sqlx::SqlitePool;
use tower_sessions::SessionManagerLayer;
use tower_sessions_sqlx_store::SqliteStore;
use tower::util::ServiceExt;
use axum::http::{Request, StatusCode};
use axum::body::Body;

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
    };

    let protected = Router::new()
        .route("/users", get(crate::routes::get_users))
        .route("/users/{id}", get(crate::routes::get_user_detail))
        .route("/online", get(crate::routes::get_online))
        .route("/logout", post(crate::routes::post_logout))
        .route_layer(login_required!(auth::Backend, login_url = "/login"));

    let public = Router::new()
        .route("/login", get(crate::routes::get_login).post(crate::routes::post_login))
        .route("/signup", get(crate::routes::get_signup).post(crate::routes::post_signup));

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

    let game_id: i64 =
        sqlx::query_scalar("INSERT INTO games DEFAULT VALUES RETURNING id")
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
