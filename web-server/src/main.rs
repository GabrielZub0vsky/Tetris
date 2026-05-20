//! Web server entry point: user auth, profiles, lobbies, and game history.

use std::collections::HashMap;
use std::sync::atomic::AtomicU16;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    Router,
    response::Redirect,
    routing::{get, post},
};
use axum_login::{AuthManagerLayerBuilder, login_required};
use minijinja::Environment;
use sqlx::sqlite::SqlitePoolOptions;
use tower_sessions::SessionManagerLayer;
use tower_sessions_sqlx_store::SqliteStore;

mod auth;
mod db;
mod routes;

#[cfg(test)]
mod tests;

use routes::AppState;

/// Starting port for dynamically allocated game servers.
const BASE_GAME_PORT: u16 = 1338;

/// Inactivity timeout in minutes before a lobby is killed.
const LOBBY_TIMEOUT_MINUTES: i64 = 1;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect("sqlite:tetris.db?mode=rwc")
        .await?;

    db::init_schema(&pool).await?;

    // Kill any lobbies left over from a previous run.
    cleanup_orphaned_lobbies(&pool).await;

    let session_store = SqliteStore::new(pool.clone());
    session_store.migrate().await?;

    let session_layer = SessionManagerLayer::new(session_store);

    let backend = auth::Backend::new(pool.clone());
    let auth_layer = AuthManagerLayerBuilder::new(backend, session_layer).build();

    let env = build_template_env();

    let processes = Arc::new(Mutex::new(HashMap::new()));
    let next_port = Arc::new(AtomicU16::new(BASE_GAME_PORT));

    let state = AppState {
        pool: pool.clone(),
        env: Arc::new(env),
        processes: processes.clone(),
        next_port: next_port.clone(),
    };

    // Background task: kill stale lobbies every 30 s.
    let cleanup_pool = pool.clone();
    let cleanup_procs = processes.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            if let Ok(stale) = db::get_stale_lobbies(&cleanup_pool, LOBBY_TIMEOUT_MINUTES).await {
                for lobby in stale {
                    routes::kill_lobby_process(&cleanup_procs, lobby.id);
                    let _ = db::delete_lobby(&cleanup_pool, lobby.id).await;
                }
            }
        }
    });

    let protected = Router::new()
        .route("/", get(|| async { Redirect::to("/lobbies") }))
        .route("/users", get(routes::get_users))
        .route("/users/{id}", get(routes::get_user_detail))
        .route("/online", get(routes::get_online))
        .route("/logout", post(routes::post_logout))
        .route(
            "/lobbies",
            get(routes::get_lobbies).post(routes::post_create_lobby),
        )
        .route("/lobbies/{id}", get(routes::get_lobby_detail))
        .route("/lobbies/{id}/join", post(routes::post_join_lobby))
        .route("/lobbies/{id}/leave", post(routes::post_leave_lobby))
        .route_layer(login_required!(auth::Backend, login_url = "/login"));

    let public = Router::new()
        .route("/login", get(routes::get_login).post(routes::post_login))
        .route("/signup", get(routes::get_signup).post(routes::post_signup));

    let app = Router::new()
        .merge(protected)
        .merge(public)
        .layer(auth_layer)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    println!("Listening on http://localhost:3000");
    axum::serve(listener, app.into_make_service()).await?;

    Ok(())
}

/// On startup, delete any lobbies still marked active from a previous run.
/// Their game-server processes are already dead (orphaned on web-server exit).
async fn cleanup_orphaned_lobbies(pool: &sqlx::SqlitePool) {
    if let Ok(active) = db::get_all_active_lobbies(pool).await {
        for lobby in active {
            // Try to kill by stored PID in case the OS reused none of them.
            if let Some(pid) = lobby.pid {
                let _ = std::process::Command::new("kill")
                    .args(["-9", &pid.to_string()])
                    .status();
            }
            let _ = db::delete_lobby(pool, lobby.id).await;
        }
    }
}

/// Build the MiniJinja template environment with all embedded templates.
pub fn build_template_env() -> Environment<'static> {
    let mut env = Environment::new();

    env.add_template_owned(
        "base.html",
        include_str!("../../templates/base.html").to_string(),
    )
    .unwrap();
    env.add_template_owned(
        "login.html",
        include_str!("../../templates/login.html").to_string(),
    )
    .unwrap();
    env.add_template_owned(
        "signup.html",
        include_str!("../../templates/signup.html").to_string(),
    )
    .unwrap();
    env.add_template_owned(
        "users.html",
        include_str!("../../templates/users.html").to_string(),
    )
    .unwrap();
    env.add_template_owned(
        "user_detail.html",
        include_str!("../../templates/user_detail.html").to_string(),
    )
    .unwrap();
    env.add_template_owned(
        "online.html",
        include_str!("../../templates/online.html").to_string(),
    )
    .unwrap();
    env.add_template_owned(
        "lobbies.html",
        include_str!("../../templates/lobbies.html").to_string(),
    )
    .unwrap();
    env.add_template_owned(
        "lobby_detail.html",
        include_str!("../../templates/lobby_detail.html").to_string(),
    )
    .unwrap();

    env
}
