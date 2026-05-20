//! Route handlers for the web server.

use axum::{
    Form,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use minijinja::Environment;
use password_auth::generate_hash;
use serde::Deserialize;
use sqlx::SqlitePool;
use std::cmp::Ordering as CmpOrd;
use std::collections::HashMap;
use std::process::Child;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};

use crate::{
    auth::{AuthSession, Credentials},
    db,
};

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub env: Arc<Environment<'static>>,
    /// Handles for spawned game-server child processes, keyed by lobby id.
    pub processes: Arc<Mutex<HashMap<i64, Child>>>,
    /// Next port to allocate for a game server.
    pub next_port: Arc<AtomicU16>,
}

/// Kill and remove the game-server process for a lobby, if one exists.
pub fn kill_lobby_process(processes: &Mutex<HashMap<i64, Child>>, lobby_id: i64) {
    if let Ok(mut map) = processes.lock()
        && let Some(mut child) = map.remove(&lobby_id)
    {
        let _ = child.kill();
    }
}

/// Render a template, returning 500 on failure.
fn render(env: &Environment<'_>, name: &str, ctx: minijinja::Value) -> Response {
    match env.get_template(name) {
        Ok(tmpl) => match tmpl.render(ctx) {
            Ok(html) => Html(html).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Template render error: {e}"),
            )
                .into_response(),
        },
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Template not found: {e}"),
        )
            .into_response(),
    }
}

// ── Login ────────────────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
pub struct LoginQuery {
    pub username: Option<String>,
}

pub async fn get_login(
    auth_session: AuthSession,
    State(state): State<AppState>,
    Query(q): Query<LoginQuery>,
) -> Response {
    if auth_session.user.is_some() {
        return Redirect::to("/users").into_response();
    }
    render(
        &state.env,
        "login.html",
        minijinja::context! {
            error => "",
            prefill_username => q.username.unwrap_or_default(),
        },
    )
}

pub async fn post_login(
    mut auth_session: AuthSession,
    State(state): State<AppState>,
    Form(creds): Form<Credentials>,
) -> Response {
    let next = creds.next.clone().unwrap_or_else(|| "/users".to_string());
    match auth_session.authenticate(creds).await {
        Ok(Some(user)) => {
            if auth_session.login(&user).await.is_err() {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            let _ = db::touch_user(&state.pool, user.id).await;
            Redirect::to(&next).into_response()
        }
        Ok(None) => render(
            &state.env,
            "login.html",
            minijinja::context! { error => "Invalid username or password." },
        ),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── Signup ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SignupForm {
    pub username: String,
    pub password: String,
    pub confirm_password: String,
}

pub async fn get_signup(auth_session: AuthSession, State(state): State<AppState>) -> Response {
    if auth_session.user.is_some() {
        return Redirect::to("/users").into_response();
    }
    render(
        &state.env,
        "signup.html",
        minijinja::context! { error => "" },
    )
}

pub async fn post_signup(
    mut auth_session: AuthSession,
    State(state): State<AppState>,
    Form(form): Form<SignupForm>,
) -> Response {
    let username = form.username.trim().to_string();

    if username.is_empty() {
        return render(
            &state.env,
            "signup.html",
            minijinja::context! { error => "Username cannot be empty." },
        );
    }

    if form.password != form.confirm_password {
        return render(
            &state.env,
            "signup.html",
            minijinja::context! { error => "Passwords do not match." },
        );
    }

    if form.password.len() < 6 {
        return render(
            &state.env,
            "signup.html",
            minijinja::context! { error => "Password must be at least 6 characters." },
        );
    }

    let password = form.password.clone();
    let hash = match tokio::task::spawn_blocking(move || generate_hash(&password)).await {
        Ok(h) => h,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    match db::create_user(&state.pool, &username, &hash).await {
        Ok(user_id) => {
            let _ = db::touch_user(&state.pool, user_id).await;
            if let Ok(Some(user)) = db::get_user_by_id(&state.pool, user_id).await {
                let _ = auth_session.login(&user).await;
            }
            Redirect::to("/users").into_response()
        }
        Err(e) if e.to_string().contains("UNIQUE constraint failed") => render(
            &state.env,
            "signup.html",
            minijinja::context! {
                error => "",
                username_taken => true,
                taken_username => username,
            },
        ),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── Logout ───────────────────────────────────────────────────────────────────

pub async fn post_logout(mut auth_session: AuthSession) -> Response {
    let _ = auth_session.logout().await;
    Redirect::to("/login").into_response()
}

// ── User list ────────────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
pub struct UsersQuery {
    pub sort: Option<String>,
}

pub async fn get_users(
    auth_session: AuthSession,
    State(state): State<AppState>,
    Query(q): Query<UsersQuery>,
) -> Response {
    let current_user = auth_session.user.as_ref().map(|u| u.username.clone());
    let users = match db::list_users(&state.pool).await {
        Ok(u) => u,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let mut entries: Vec<(db::User, db::CareerStats)> = Vec::with_capacity(users.len());
    for u in users {
        let stats = db::get_user_career_stats(&state.pool, u.id)
            .await
            .unwrap_or_default();
        entries.push((u, stats));
    }

    let sort = q.sort.as_deref().unwrap_or("wins");
    sort_entries(&mut entries, sort);

    let rows: Vec<_> = entries
        .iter()
        .enumerate()
        .map(|(i, (u, s))| {
            minijinja::context! {
                rank => i + 1,
                id => u.id,
                username => u.username.clone(),
                created_at => u.created_at.clone(),
                wins => s.wins,
                losses => s.losses,
                win_pct => format!("{:.1}", s.win_pct),
                highest_score => s.highest_score,
                fastest_elim => s.fastest_elim_seconds
                    .map(|x| format!("{:.1}s", x))
                    .unwrap_or_else(|| "—".to_string()),
                total_play => format_duration(s.total_play_seconds),
            }
        })
        .collect();

    render(
        &state.env,
        "users.html",
        minijinja::context! {
            users => rows,
            current_user => current_user,
            sort => sort,
        },
    )
}

/// Sort users by the requested leaderboard key.
/// Numeric metrics sort descending (bigger = better) except `fastest_elim`
/// which sorts ascending with None pushed to the end.
fn sort_entries(entries: &mut [(db::User, db::CareerStats)], key: &str) {
    match key {
        "username" => entries.sort_by(|a, b| {
            a.0.username
                .to_lowercase()
                .cmp(&b.0.username.to_lowercase())
        }),
        "losses" => entries.sort_by_key(|e| std::cmp::Reverse(e.1.losses)),
        "win_pct" => entries.sort_by(|a, b| {
            b.1.win_pct
                .partial_cmp(&a.1.win_pct)
                .unwrap_or(CmpOrd::Equal)
        }),
        "highest_score" => entries.sort_by_key(|e| std::cmp::Reverse(e.1.highest_score)),
        "fastest_elim" => {
            entries.sort_by(
                |a, b| match (a.1.fastest_elim_seconds, b.1.fastest_elim_seconds) {
                    (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(CmpOrd::Equal),
                    (Some(_), None) => CmpOrd::Less,
                    (None, Some(_)) => CmpOrd::Greater,
                    _ => CmpOrd::Equal,
                },
            )
        }
        "total_play" => entries.sort_by(|a, b| {
            b.1.total_play_seconds
                .partial_cmp(&a.1.total_play_seconds)
                .unwrap_or(CmpOrd::Equal)
        }),
        // Default: wins desc.
        _ => entries.sort_by_key(|e| std::cmp::Reverse(e.1.wins)),
    }
}

fn format_duration(seconds: f64) -> String {
    if seconds <= 0.0 {
        return "—".to_string();
    }
    let total = seconds as i64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

// ── User detail ──────────────────────────────────────────────────────────────

pub async fn get_user_detail(
    auth_session: AuthSession,
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
) -> Response {
    let current_user = auth_session.user.as_ref().map(|u| u.username.clone());
    let user = match db::get_user_by_id(&state.pool, user_id).await {
        Ok(Some(u)) => u,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let games = db::get_user_games(&state.pool, user_id)
        .await
        .unwrap_or_default();
    let stats = db::get_user_career_stats(&state.pool, user_id)
        .await
        .unwrap_or_default();

    render(
        &state.env,
        "user_detail.html",
        minijinja::context! {
            profile_user => minijinja::context! {
                id => user.id,
                username => user.username,
                created_at => user.created_at,
            },
            games => games.iter().map(|g| minijinja::context! {
                game_id => g.game_id,
                played_at => g.played_at.clone(),
                verdict => g.verdict.clone(),
                opponent => g.opponent_username.clone().unwrap_or_else(|| "Unknown".to_string()),
            }).collect::<Vec<_>>(),
            wins => stats.wins,
            losses => stats.losses,
            draws => stats.draws,
            win_pct => format!("{:.1}", stats.win_pct),
            highest_score => stats.highest_score,
            fastest_elim => stats.fastest_elim_seconds
                .map(|s| format!("{:.1}s", s))
                .unwrap_or_else(|| "—".to_string()),
            total_play => format_duration(stats.total_play_seconds),
            current_user => current_user,
        },
    )
}

// ── Currently online ─────────────────────────────────────────────────────────

pub async fn get_online(auth_session: AuthSession, State(state): State<AppState>) -> Response {
    let current_user = auth_session.user.as_ref().map(|u| {
        {
            let pool = state.pool.clone();
            let id = u.id;
            tokio::spawn(async move { db::touch_user(&pool, id).await });
        }
        u.username.clone()
    });

    match db::list_online_users(&state.pool).await {
        Ok(users) => render(
            &state.env,
            "online.html",
            minijinja::context! {
                online_users => users.iter().map(|u| minijinja::context! {
                    id => u.id,
                    username => u.username.clone(),
                    last_seen_at => u.last_seen_at.clone().unwrap_or_default(),
                }).collect::<Vec<_>>(),
                current_user => current_user,
            },
        ),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── Lobbies ──────────────────────────────────────────────────────────────────

/// Form for creating a new lobby.
#[derive(Deserialize)]
pub struct CreateLobbyForm {
    pub max_players: i64,
}

/// List all active lobbies.
pub async fn get_lobbies(auth_session: AuthSession, State(state): State<AppState>) -> Response {
    let current_user = auth_session.user.as_ref().map(|u| u.username.clone());
    let user_lobby_id = match &auth_session.user {
        Some(u) => db::get_user_current_lobby(&state.pool, u.id)
            .await
            .ok()
            .flatten()
            .map(|l| l.id),
        None => None,
    };
    match db::list_active_lobbies(&state.pool).await {
        Ok(lobbies) => render(
            &state.env,
            "lobbies.html",
            minijinja::context! {
                lobbies => lobbies.iter().map(|l| minijinja::context! {
                    id => l.id,
                    host_username => l.host_username.clone(),
                    max_players => l.max_players,
                    port => l.port,
                    status => l.status.clone(),
                    created_at => l.created_at.clone(),
                    member_count => l.member_count,
                }).collect::<Vec<_>>(),
                current_user => current_user,
                user_lobby_id => user_lobby_id,
                error => "",
            },
        ),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Create a new lobby and spawn its game server.
pub async fn post_create_lobby(
    auth_session: AuthSession,
    State(state): State<AppState>,
    Form(form): Form<CreateLobbyForm>,
) -> Response {
    let user = match &auth_session.user {
        Some(u) => u.clone(),
        None => return Redirect::to("/login").into_response(),
    };

    let current_user = user.username.clone();

    if let Ok(Some(_)) = db::get_user_current_lobby(&state.pool, user.id).await {
        return render_lobbies_error(&state, &current_user, "Already in a lobby.").await;
    }

    if !(1..=3).contains(&form.max_players) {
        return render_lobbies_error(&state, &current_user, "Lobby size must be 1–3.").await;
    }

    let port = state.next_port.fetch_add(1, Ordering::Relaxed) as i64;

    let lobby_id = match db::create_lobby(&state.pool, user.id, form.max_players, port).await {
        Ok(id) => id,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let _ = db::add_lobby_member(&state.pool, lobby_id, user.id).await;

    let new_count = db::get_lobby_member_count(&state.pool, lobby_id)
        .await
        .unwrap_or(0);
    if new_count >= form.max_players {
        let _ = db::set_lobby_status(&state.pool, lobby_id, "running").await;
    }

    spawn_game_server(&state, lobby_id, port, form.max_players).await;

    Redirect::to(&format!("/lobbies/{lobby_id}")).into_response()
}

/// Show lobby detail page.
pub async fn get_lobby_detail(
    auth_session: AuthSession,
    State(state): State<AppState>,
    Path(lobby_id): Path<i64>,
) -> Response {
    let current_user = auth_session.user.as_ref().map(|u| u.username.clone());
    let user_id = auth_session.user.as_ref().map(|u| u.id);

    let lobby = match db::get_lobby(&state.pool, lobby_id).await {
        Ok(Some(l)) => l,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let members = db::get_lobby_members(&state.pool, lobby_id)
        .await
        .unwrap_or_default();

    let user_in_lobby = user_id
        .map(|uid| members.iter().any(|m| m.id == uid))
        .unwrap_or(false);

    let is_full = members.len() as i64 >= lobby.max_players;

    render(
        &state.env,
        "lobby_detail.html",
        minijinja::context! {
            lobby => minijinja::context! {
                id => lobby.id,
                max_players => lobby.max_players,
                port => lobby.port,
                status => lobby.status,
                created_at => lobby.created_at,
            },
            members => members.iter().map(|m| minijinja::context! {
                id => m.id,
                username => m.username.clone(),
            }).collect::<Vec<_>>(),
            current_user => current_user,
            user_in_lobby => user_in_lobby,
            is_full => is_full,
        },
    )
}

/// Join an existing lobby.
pub async fn post_join_lobby(
    auth_session: AuthSession,
    State(state): State<AppState>,
    Path(lobby_id): Path<i64>,
) -> Response {
    let user = match &auth_session.user {
        Some(u) => u.clone(),
        None => return Redirect::to("/login").into_response(),
    };

    if let Ok(Some(_)) = db::get_user_current_lobby(&state.pool, user.id).await {
        return Redirect::to("/lobbies").into_response();
    }

    let lobby = match db::get_lobby(&state.pool, lobby_id).await {
        Ok(Some(l)) => l,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };

    if lobby.status != "waiting" {
        return Redirect::to("/lobbies").into_response();
    }

    let count = db::get_lobby_member_count(&state.pool, lobby_id)
        .await
        .unwrap_or(i64::MAX);
    if count >= lobby.max_players {
        return Redirect::to("/lobbies").into_response();
    }

    let _ = db::add_lobby_member(&state.pool, lobby_id, user.id).await;
    let _ = db::touch_lobby(&state.pool, lobby_id).await;

    let new_count = db::get_lobby_member_count(&state.pool, lobby_id)
        .await
        .unwrap_or(0);
    if new_count >= lobby.max_players {
        let _ = db::set_lobby_status(&state.pool, lobby_id, "running").await;
    }

    Redirect::to(&format!("/lobbies/{lobby_id}")).into_response()
}

/// Leave (or close) a lobby.
pub async fn post_leave_lobby(
    auth_session: AuthSession,
    State(state): State<AppState>,
    Path(lobby_id): Path<i64>,
) -> Response {
    let user = match &auth_session.user {
        Some(u) => u.clone(),
        None => return Redirect::to("/login").into_response(),
    };

    let lobby = match db::get_lobby(&state.pool, lobby_id).await {
        Ok(Some(l)) => l,
        _ => return Redirect::to("/lobbies").into_response(),
    };

    let _ = db::remove_lobby_member(&state.pool, lobby_id, user.id).await;
    let _ = db::touch_lobby(&state.pool, lobby_id).await;

    let remaining = db::get_lobby_member_count(&state.pool, lobby_id)
        .await
        .unwrap_or(0);

    if remaining == 0 || lobby.host_user_id == user.id {
        kill_lobby_process(&state.processes, lobby_id);
        let _ = db::delete_lobby(&state.pool, lobby_id).await;
    }

    Redirect::to("/lobbies").into_response()
}

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn render_lobbies_error(state: &AppState, current_user: &str, error: &str) -> Response {
    let lobbies = db::list_active_lobbies(&state.pool)
        .await
        .unwrap_or_default();
    render(
        &state.env,
        "lobbies.html",
        minijinja::context! {
            lobbies => lobbies.iter().map(|l| minijinja::context! {
                id => l.id,
                host_username => l.host_username.clone(),
                max_players => l.max_players,
                port => l.port,
                status => l.status.clone(),
                created_at => l.created_at.clone(),
                member_count => l.member_count,
            }).collect::<Vec<_>>(),
            current_user => current_user,
            user_lobby_id => Option::<i64>::None,
            error => error,
        },
    )
}

/// Spawn a game-server child process for the given lobby and store its handle.
pub async fn spawn_game_server(state: &AppState, lobby_id: i64, port: i64, max_players: i64) {
    let base_config = match max_players {
        2 => "config-2.json",
        3 => "config-3.json",
        _ => "config.json",
    };
    let config_str = match std::fs::read_to_string(base_config) {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut config: serde_json::Value = match serde_json::from_str(&config_str) {
        Ok(v) => v,
        Err(_) => return,
    };
    config["server_port"] = serde_json::json!(port);

    let members = db::get_lobby_members(&state.pool, lobby_id)
        .await
        .unwrap_or_default();
    let player_ids: Vec<i64> = members.iter().map(|m| m.id).collect();
    config["player_ids"] = serde_json::json!(player_ids);
    config["db_path"] = serde_json::json!("tetris.db");

    let config_path = format!("/tmp/lobby_{lobby_id}.json");
    if std::fs::write(&config_path, config.to_string()).is_err() {
        return;
    }

    let result = tokio::task::spawn_blocking(move || {
        std::process::Command::new("./target/debug/server")
            .arg("--config")
            .arg(&config_path)
            .spawn()
    })
    .await;

    if let Ok(Ok(child)) = result {
        let pid = child.id() as i64;
        state.processes.lock().unwrap().insert(lobby_id, child);
        let pool = state.pool.clone();
        tokio::spawn(async move {
            let _ = db::set_lobby_pid(&pool, lobby_id, pid).await;
        });
    }
}
