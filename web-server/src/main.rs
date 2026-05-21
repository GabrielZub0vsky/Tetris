//! Web server entry point: user auth, profiles, lobbies, and game history.
//!
//! ## Module responsibilities
//!
//! * `auth`    — axum-login glue around the SQLite `users` table.
//! * `db`      — schema initialization and every SQL query used by the app.
//! * `routes`  — every HTTP handler, including lobby creation, the leaderboard, and the port-pool helpers used to spawn game servers.
//! * `tests`   — integration tests built against an in-memory SQLite database.
//!
//! ## What main() actually does
//!
//! 1. Open (or create) `tetris.db` and apply `db::init_schema`.
//! 2. Reap any lobby rows left over from a previous run (their child
//!    game-server processes are dead — they died with the previous web-server).
//! 3. Build the tower-sessions session store on top of the same pool.
//! 4. Build the auth layer (`axum-login`) on top of the session layer.
//! 5. Build the template environment (`minijinja`).
//! 6. Build the shared port pool (1338..=1437) used by `routes::allocate_port`
//!    when spawning a game server for a new lobby.
//! 7. Spawn a background task that kills idle lobbies every 30 s.
//! 8. Assemble the protected + public routers and serve on `0.0.0.0:3000`.

use std::collections::HashMap;
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
pub const BASE_GAME_PORT: u16 = 1338;

/// Last allowable game-server port (inclusive). 100 ports total.
pub const MAX_GAME_PORT: u16 = 1437;

/// Inactivity timeout in minutes before a lobby is killed.
const LOBBY_TIMEOUT_MINUTES: i64 = 1;

/// Boot the web server.
///
/// Wires together: SQLite pool → schema bootstrap → orphan cleanup →
/// session store → auth backend → minijinja templates → port pool →
/// background cleanup task → axum router → TCP listener on port 3000.
///
/// See the module-level docs above for the full step-by-step.
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

    // Initialize port pool: all 100 ports start as not-in-use.
    let mut initial_pool = HashMap::new();
    for p in BASE_GAME_PORT..=MAX_GAME_PORT {
        initial_pool.insert(p, false);
    }
    let port_pool = Arc::new(Mutex::new(initial_pool));
    let killed_lobbies = Arc::new(Mutex::new(HashMap::new()));

    let state = AppState {
        pool: pool.clone(),
        env: Arc::new(env),
        processes: processes.clone(),
        port_pool: port_pool.clone(),
        killed_lobbies: killed_lobbies.clone(),
    };

    // Background task: kill stale lobbies every 15 s (so the worst-case
    // detection lag stays well under the 1-minute timeout the user sees).
    let cleanup_pool = pool.clone();
    let cleanup_procs = processes.clone();
    let cleanup_ports = port_pool.clone();
    let cleanup_killed = killed_lobbies.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(15));
        loop {
            interval.tick().await;
            if let Ok(stale) = db::get_stale_lobbies(&cleanup_pool, LOBBY_TIMEOUT_MINUTES).await {
                for lobby in stale {
                    routes::kill_lobby_process(&cleanup_procs, lobby.id);
                    routes::release_port(&cleanup_ports, lobby.port as u16);
                    // Record WHY the lobby died before deleting the row,
                    // so the front-end poller can show an "inactivity"
                    // notice instead of a generic "lobby ended" message.
                    routes::mark_lobby_killed(
                        &cleanup_killed,
                        lobby.id,
                        "inactivity (no activity for 1 minute)",
                    );
                    let _ = db::delete_lobby(&cleanup_pool, lobby.id).await;
                }
            }
            // Drop kill records older than 5 minutes so the map can't
            // grow forever.
            routes::prune_killed(&cleanup_killed);
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
        .route("/lobbies/{id}/status", get(routes::get_lobby_status))
        .route("/lobbies/{id}/join", post(routes::post_join_lobby))
        .route("/lobbies/{id}/leave", post(routes::post_leave_lobby))
        .route("/play/forfeit", post(routes::post_play_forfeit))
        .route_layer(login_required!(auth::Backend, login_url = "/login"));

    let public = Router::new()
        .route("/login", get(routes::get_login).post(routes::post_login))
        .route("/signup", get(routes::get_signup).post(routes::post_signup))
        // Game-client (WASM) pages. Public so the freshly-spawned tetris
        // window can fetch its assets without needing the session cookie.
        .route("/play/{port}/", get(routes::get_play_page))
        .route("/play/{port}/config.json", get(routes::get_play_config));

    let app = Router::new()
        .merge(protected)
        .merge(public)
        // Serve the compiled WASM bundle (client.js, client_bg.wasm, etc.)
        // and the static config templates so /play/*/ can `import` them.
        .nest_service("/static", tower_http::services::ServeDir::new("static"))
        .layer(auth_layer)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    println!("Listening on http://localhost:3000");
    axum::serve(listener, app.into_make_service()).await?;

    Ok(())
}

/// On startup, delete any lobbies still marked active from a previous run.
///
/// When the web server exits its child game-server processes are killed
/// (or already dead). The lobby rows they wrote, however, persist in
/// SQLite. Without this sweep those rows would dangle forever, blocking
/// users from creating new lobbies (because `get_user_current_lobby` would
/// still match them) and showing up on `/lobbies` as ghost entries.
///
/// We also issue a `kill -9` for the stored PID just in case the previous
/// process is somehow still alive (e.g. the web server crashed without
/// killing its children) — best-effort, errors are ignored.
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
///
/// Each template is read at compile time with `include_str!` so the binary
/// is self-contained — no runtime FS reads, no `templates/` directory has
/// to be shipped alongside the executable.
///
/// The returned `Environment` is wrapped in `Arc` and stored on `AppState`
/// so every handler shares one parsed template set.
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
