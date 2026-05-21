//! Route handlers for the web server.
//!
//! Every public handler here is wired into the axum `Router` in `main.rs`
//! under either the protected (login_required!) or public branch. The file
//! also owns the helpers around the lobby/game-server lifecycle: the port
//! pool, the child-process map, and the spawn helper that forks the game
//! server binary.

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
use std::sync::{Arc, Mutex};

use crate::{
    BASE_GAME_PORT, MAX_GAME_PORT,
    auth::{AuthSession, Credentials},
    db,
};

/// Information stashed when a lobby is destroyed, so the client polling
/// `/lobbies/{id}/status` can be told *why* the lobby disappeared.
#[derive(Debug, Clone)]
pub struct KilledInfo {
    pub reason: String,
    pub killed_at: std::time::Instant,
}

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub env: Arc<Environment<'static>>,
    /// Handles for spawned game-server child processes, keyed by lobby id.
    pub processes: Arc<Mutex<HashMap<i64, Child>>>,
    /// Port pool: port number → whether currently in use.
    /// Range BASE_GAME_PORT..=MAX_GAME_PORT (100 ports).
    pub port_pool: Arc<Mutex<HashMap<u16, bool>>>,
    /// Recently-killed lobby ids with the reason. Entries expire after
    /// ~5 minutes (see `prune_killed`). Lets the lobby-detail page's
    /// JavaScript poller display a friendly notice instead of just
    /// "lobby vanished".
    pub killed_lobbies: Arc<Mutex<HashMap<i64, KilledInfo>>>,
}

/// Record that a lobby was killed and why.
pub fn mark_lobby_killed(map: &Mutex<HashMap<i64, KilledInfo>>, id: i64, reason: &str) {
    if let Ok(mut m) = map.lock() {
        m.insert(
            id,
            KilledInfo {
                reason: reason.to_string(),
                killed_at: std::time::Instant::now(),
            },
        );
    }
}

/// Look up the kill reason for a lobby, if it was recently killed.
pub fn lookup_killed(map: &Mutex<HashMap<i64, KilledInfo>>, id: i64) -> Option<KilledInfo> {
    map.lock().ok()?.get(&id).cloned()
}

/// Drop killed-lobby entries older than 5 minutes so the map doesn't
/// grow without bound over a long-running server.
pub fn prune_killed(map: &Mutex<HashMap<i64, KilledInfo>>) {
    if let Ok(mut m) = map.lock() {
        m.retain(|_, info| info.killed_at.elapsed() < std::time::Duration::from_secs(300));
    }
}

/// Pick the lowest-numbered free port, mark it in-use, return it.
///
/// Iterates `BASE_GAME_PORT..=MAX_GAME_PORT` and grabs the first entry
/// whose value is `false`. Holds the mutex for the entire scan + write
/// so two concurrent `POST /lobbies` requests can never receive the same
/// port. Returns `None` once the pool of 100 ports is exhausted; the
/// caller (`post_create_lobby`) surfaces that as a friendly "at capacity"
/// error rather than a 500.
pub fn allocate_port(pool: &Mutex<HashMap<u16, bool>>) -> Option<u16> {
    let mut map = pool.lock().ok()?;
    for p in BASE_GAME_PORT..=MAX_GAME_PORT {
        if matches!(map.get(&p), Some(false)) {
            map.insert(p, true);
            return Some(p);
        }
    }
    None
}

/// Mark the given port as not-in-use. No-op if port is outside the pool.
///
/// Called from every lobby-destruction path:
///   * `post_leave_lobby`, when the host leaves or the last player exits
///   * the stale-lobby background task in `main.rs`
///   * the create-lobby path itself, if the DB insert fails after we
///     already grabbed a port
///
/// The "outside the pool" guard exists so stale DB rows from an older
/// port range don't accidentally inject phantom entries into the map.
pub fn release_port(pool: &Mutex<HashMap<u16, bool>>, port: u16) {
    if let Ok(mut map) = pool.lock()
        && map.contains_key(&port)
    {
        map.insert(port, false);
    }
}

/// Kill and remove the game-server process for a lobby, if one exists.
///
/// Sends SIGKILL to the child via `Child::kill()`. The OS reclaims the
/// TCP port the process was bound to once the socket closes; the entry in
/// `AppState.port_pool` is freed separately by `release_port`.
pub fn kill_lobby_process(processes: &Mutex<HashMap<i64, Child>>, lobby_id: i64) {
    if let Ok(mut map) = processes.lock()
        && let Some(mut child) = map.remove(&lobby_id)
    {
        let _ = child.kill();
    }
}

/// Render a minijinja template and wrap the result as an axum `Response`.
///
/// Every handler in this module ultimately calls into this helper to turn
/// a `minijinja::context! { ... }` value into an HTML 200 response. The
/// two failure modes (unknown template name, render error) are both
/// surfaced as 500 with a developer-facing error string in the body.
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

/// Query string for `GET /login`.
///
/// `?username=NAME` deep-links from the duplicate-signup error page to a
/// pre-filled login form so users don't have to retype the conflicting
/// username they just tried.
#[derive(Deserialize, Default)]
pub struct LoginQuery {
    pub username: Option<String>,
}

/// `GET /login` — render the login page.
///
/// If the visitor already has a valid session, jump straight to /users so
/// the login form isn't shown to logged-in users. Otherwise render
/// `login.html`, prefilling the username field if `?username=` was passed.
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

/// `POST /login` — verify credentials and start a session.
///
/// Hands the form-decoded `Credentials` to `axum-login`, which calls into
/// `auth::Backend::authenticate`. On success we mint a session, touch
/// `last_seen_at` (so the user shows up on `/online`) and redirect to
/// either the form's `next` field or `/users`. On a wrong password we
/// re-render `login.html` with a generic error (no user enumeration).
pub async fn post_login(
    mut auth_session: AuthSession,
    State(state): State<AppState>,
    Form(creds): Form<Credentials>,
) -> Response {
    // `next=` lets the login_required! middleware send users back to the
    // page they originally requested after they sign in. Default = /users.
    let next = creds.next.clone().unwrap_or_else(|| "/users".to_string());

    match auth_session.authenticate(creds).await {
        // Happy path: credentials matched.
        Ok(Some(user)) => {
            // login() writes user_id into the session blob. If this fails
            // the session-store itself is broken (out of disk, etc.) — 500.
            if auth_session.login(&user).await.is_err() {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            // Bump last_seen_at so the user shows up on /online. Failure
            // here is non-fatal; the login itself already succeeded.
            let _ = db::touch_user(&state.pool, user.id).await;
            Redirect::to(&next).into_response()
        }
        // Edge case: wrong password OR unknown username. We MUST surface
        // the same message in both sub-cases to avoid user enumeration:
        // a different error for "no such user" would let an attacker
        // probe valid usernames.
        Ok(None) => render(
            &state.env,
            "login.html",
            minijinja::context! { error => "Invalid username or password." },
        ),
        // Database error during the lookup → 500.
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

/// `GET /signup` — render the signup form, or bounce already-logged-in
/// visitors over to `/users`.
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

/// `POST /signup` — create a new account and auto-login.
///
/// Validation order:
///   1. Trimmed username must be non-empty
///   2. password == confirm_password
///   3. password length ≥ 6
///   4. argon2/bcrypt hash via `password_auth::generate_hash` (offloaded
///      to `spawn_blocking` because hashing is CPU-bound)
///   5. INSERT into `users`; on UNIQUE collision we render the friendly
///      "username already exists" error with a deep-link to `/login`
///
/// On success the new user is immediately logged in via
/// `auth_session.login`, so the session cookie comes back on the same
/// response.
pub async fn post_signup(
    mut auth_session: AuthSession,
    State(state): State<AppState>,
    Form(form): Form<SignupForm>,
) -> Response {
    // Trim leading/trailing whitespace — copy/paste of usernames from
    // chat clients tends to include stray spaces. A bare "   " username
    // would otherwise pass the non-empty check below.
    let username = form.username.trim().to_string();

    // Edge case: empty (or whitespace-only) username.
    if username.is_empty() {
        return render(
            &state.env,
            "signup.html",
            minijinja::context! { error => "Username cannot be empty." },
        );
    }

    // Edge case: confirm_password doesn't match. Guards against typos
    // before we go anywhere near the (expensive) hashing step.
    if form.password != form.confirm_password {
        return render(
            &state.env,
            "signup.html",
            minijinja::context! { error => "Passwords do not match." },
        );
    }

    // Edge case: weak password. 6 is the absolute floor; longer is
    // enforced by argon2's cost parameters in production.
    if form.password.len() < 6 {
        return render(
            &state.env,
            "signup.html",
            minijinja::context! { error => "Password must be at least 6 characters." },
        );
    }

    // Hash the password BEFORE the INSERT so we never write plaintext.
    // `generate_hash` is argon2 (via the `password-auth` crate) — CPU-
    // bound, so we offload to a blocking thread. If the JoinError fires
    // the runtime is in serious trouble → 500.
    let password = form.password.clone();
    let hash = match tokio::task::spawn_blocking(move || generate_hash(&password)).await {
        Ok(h) => h,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    match db::create_user(&state.pool, &username, &hash).await {
        // Happy path: row created. Immediately mark online + auto-login
        // so the user isn't dropped onto the login form after signing up.
        Ok(user_id) => {
            let _ = db::touch_user(&state.pool, user_id).await;
            if let Ok(Some(user)) = db::get_user_by_id(&state.pool, user_id).await {
                let _ = auth_session.login(&user).await;
            }
            Redirect::to("/users").into_response()
        }
        // Edge case: username already exists (UNIQUE constraint on the
        // `users.username` column). String-matching the error message is
        // brittle but sqlx doesn't expose a typed constraint enum for
        // SQLite. Re-render the signup page with a flag so the template
        // can show the styled "exists" message + a /login deep-link.
        Err(e) if e.to_string().contains("UNIQUE constraint failed") => render(
            &state.env,
            "signup.html",
            minijinja::context! {
                error => "",
                username_taken => true,
                taken_username => username,
            },
        ),
        // Any other DB failure (lock, disk, schema mismatch) → 500.
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── Logout ───────────────────────────────────────────────────────────────────

/// `POST /logout` — clear the session and bounce back to `/login`.
///
/// `auth_session.logout()` drops `user_id` from the session blob. The
/// cookie itself remains in the browser but becomes anonymous.
pub async fn post_logout(mut auth_session: AuthSession) -> Response {
    let _ = auth_session.logout().await;
    Redirect::to("/login").into_response()
}

// ── User list ────────────────────────────────────────────────────────────────

/// Query string for `GET /users`.
///
/// `?sort=KEY` controls leaderboard ordering. KEY is one of `wins`,
/// `losses`, `win_pct`, `highest_score`, `fastest_elim`, `total_play`,
/// `username`. Anything else (or missing) falls back to `wins`.
#[derive(Deserialize, Default)]
pub struct UsersQuery {
    pub sort: Option<String>,
}

/// `GET /users` — leaderboard page, listing every registered user with
/// their career stats.
///
/// Pipeline:
///   1. Load every user row.
///   2. Compute `CareerStats` per user via a separate query each (N+1 —
///      fine for a class project, would batch in production).
///   3. Sort by `?sort=` key (default `wins` desc).
///   4. Build a per-row minijinja context with all display-ready strings
///      (durations formatted, em-dashes for absent values).
///   5. Render `users.html`.
pub async fn get_users(
    auth_session: AuthSession,
    State(state): State<AppState>,
    Query(q): Query<UsersQuery>,
) -> Response {
    // The auth layer guarantees this handler only runs with a logged-in
    // session, but we still keep `current_user` optional so the same
    // template can be rendered in places where it might be None.
    let current_user = auth_session.user.as_ref().map(|u| u.username.clone());

    // Load all users. If the DB itself errors (locked, corrupt, etc.)
    // surface a 500 — we don't have a graceful empty-state for this.
    let users = match db::list_users(&state.pool).await {
        Ok(u) => u,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    // Pair every user with their CareerStats. Each call hits the DB
    // (N+1) — acceptable here because the user count is small.
    // `unwrap_or_default()` means a stats-query failure for one user
    // shows zeros for that user rather than breaking the whole page.
    let mut entries: Vec<(db::User, db::CareerStats)> = Vec::with_capacity(users.len());
    for u in users {
        let stats = db::get_user_career_stats(&state.pool, u.id)
            .await
            .unwrap_or_default();
        entries.push((u, stats));
    }

    // Sort key comes from the URL. `sort_entries` knows the right
    // direction per column (descending for "more is better", ascending
    // for "Fastest KO").
    let sort = q.sort.as_deref().unwrap_or("wins");
    sort_entries(&mut entries, sort);

    // Build the per-row template context. `rank` is the 1-based index
    // after sorting. Durations and the elim time are pre-formatted here
    // so the template stays free of formatting logic. `None` for
    // `fastest_elim_seconds` becomes an em-dash so the column never
    // shows "0.0s" for users who never won a game.
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

    // `sort` is passed back to the template so the active column header
    // can be highlighted with a ↓ marker.
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

/// Format a duration in seconds as `Xh Ym Zs` / `Ym Zs` / `Zs`.
///
/// Returns an em-dash for zero / negative input so the leaderboard never
/// renders "0s" for users who haven't played anything.
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

/// `GET /users/{id}` — single-user profile page.
///
/// Three things happen:
///   1. Look up the target user by id (404 if no row, 500 on DB error).
///   2. Pull their game history (game_id, played_at, verdict, opponent).
///   3. Pull their full `CareerStats`.
/// 
/// Both stats lookups use `unwrap_or_default()` — a transient query failure shouldn't blank the page.
pub async fn get_user_detail(
    auth_session: AuthSession,
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
) -> Response {
    // `current_user` is the *viewer*; `profile_user` (below) is whose
    // profile is being viewed. Templates display "Logged in as ..." with
    // the former and the page header / stats with the latter.
    let current_user = auth_session.user.as_ref().map(|u| u.username.clone());

    // Look up the profile target. Distinguish "no such user" (404) from
    // "database broke" (500) so users typing a bad URL don't see a 500.
    let user = match db::get_user_by_id(&state.pool, user_id).await {
        Ok(Some(u)) => u,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    // Game history + aggregate stats. Either failing falls back to an
    // empty list / zeroed struct so the page still renders.
    let games = db::get_user_games(&state.pool, user_id)
        .await
        .unwrap_or_default();
    let stats = db::get_user_career_stats(&state.pool, user_id)
        .await
        .unwrap_or_default();

    // The template expects flat fields, so we destructure stats here and
    // pre-format the floating-point values (win %, durations) into
    // display strings. Opponent name falls back to "Unknown" when the
    // opponent row has been deleted (game_participants survive the
    // ON DELETE CASCADE of the user row? no — actually they don't, but
    // historical rows might still reference a missing username via the
    // LEFT JOIN in get_user_games).
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

    // Same single-lobby rule as post_join_lobby: must finish or forfeit
    // the current lobby before creating a new one.
    if let Ok(Some(existing)) = db::get_user_current_lobby(&state.pool, user.id).await {
        let msg = if existing.status == "running" {
            format!(
                "You are already in lobby #{} (game in progress). \
                 Leave it (counts as a forfeit) before creating another.",
                existing.id
            )
        } else {
            format!(
                "You are already in lobby #{}. Leave it before creating another.",
                existing.id
            )
        };
        return render_lobbies_error(&state, &current_user, &msg).await;
    }

    if !(1..=3).contains(&form.max_players) {
        return render_lobbies_error(&state, &current_user, "Lobby size must be 1–3.").await;
    }

    let port = match allocate_port(&state.port_pool) {
        Some(p) => p as i64,
        None => {
            return render_lobbies_error(
                &state,
                &current_user,
                "Server is at capacity (no free ports). Try again later.",
            )
            .await;
        }
    };

    let lobby_id = match db::create_lobby(&state.pool, user.id, form.max_players, port).await {
        Ok(id) => id,
        Err(_) => {
            // Release the port we just claimed before bailing out.
            release_port(&state.port_pool, port as u16);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
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
    let current_user = user.username.clone();

    // Block: user already in some other lobby. The lobbies page UI hides
    // the Join button in this case, but enforce it server-side too in
    // case someone crafts a direct POST. `get_user_current_lobby` only
    // matches lobbies with status `waiting` or `running`, so once the
    // current lobby is destroyed (host leaves, all leave, inactivity)
    // or the user forfeits out, they're free to join again.
    if let Ok(Some(existing)) = db::get_user_current_lobby(&state.pool, user.id).await {
        let msg = if existing.status == "running" {
            format!(
                "You are already in lobby #{} (game in progress). \
                 Leave it (counts as a forfeit) before joining another.",
                existing.id
            )
        } else {
            format!(
                "You are already in lobby #{}. Leave it before joining another.",
                existing.id
            )
        };
        return render_lobbies_error(&state, &current_user, &msg).await;
    }

    let lobby = match db::get_lobby(&state.pool, lobby_id).await {
        Ok(Some(l)) => l,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };

    // Can only join a lobby that's still waiting for players.
    if lobby.status != "waiting" {
        return render_lobbies_error(
            &state,
            &current_user,
            "That lobby is no longer accepting new players.",
        )
        .await;
    }

    let count = db::get_lobby_member_count(&state.pool, lobby_id)
        .await
        .unwrap_or(i64::MAX);
    if count >= lobby.max_players {
        return render_lobbies_error(&state, &current_user, "That lobby is full.").await;
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
///
/// If the lobby is in the `running` state when a member leaves, we treat
/// the departure as a forfeit and immediately record a `Lost` game for
/// the leaver so their career stats reflect the abandoned match. The
/// regular leave/host-leaves/last-member tear-down still runs afterwards.
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

    // Forfeit detection: only count it as a loss if the user is actually
    // a member AND the game has already started. Doing this *before* the
    // remove_lobby_member call keeps the membership check honest.
    if lobby.status == "running" {
        let is_member = db::get_lobby_members(&state.pool, lobby_id)
            .await
            .map(|m| m.iter().any(|x| x.id == user.id))
            .unwrap_or(false);
        if is_member {
            let _ = db::record_forfeit_loss(&state.pool, user.id).await;
        }
    }

    let _ = db::remove_lobby_member(&state.pool, lobby_id, user.id).await;
    let _ = db::touch_lobby(&state.pool, lobby_id).await;

    let remaining = db::get_lobby_member_count(&state.pool, lobby_id)
        .await
        .unwrap_or(0);

    if remaining == 0 || lobby.host_user_id == user.id {
        kill_lobby_process(&state.processes, lobby_id);
        release_port(&state.port_pool, lobby.port as u16);
        let reason = if lobby.host_user_id == user.id {
            "host left the lobby"
        } else {
            "all players left"
        };
        mark_lobby_killed(&state.killed_lobbies, lobby_id, reason);
        let _ = db::delete_lobby(&state.pool, lobby_id).await;
    }

    Redirect::to("/lobbies").into_response()
}

/// `GET /lobbies/{id}/status` — lightweight JSON endpoint polled by the
/// lobby-detail page. Tells the client whether the lobby still exists,
/// and if not, why it disappeared. Used to drive the inactivity popup.
pub async fn get_lobby_status(
    State(state): State<AppState>,
    Path(lobby_id): Path<i64>,
) -> Response {
    // Sweep stale entries on every call — cheap, and avoids a separate
    // background task just for this map.
    prune_killed(&state.killed_lobbies);

    let lobby_exists = matches!(db::get_lobby(&state.pool, lobby_id).await, Ok(Some(_)));
    let killed = lookup_killed(&state.killed_lobbies, lobby_id);

    // Lobby is alive when the row still exists AND we haven't recorded
    // a kill for it. A row plus a kill entry shouldn't happen, but if
    // it does we trust the kill marker (DB is eventually consistent
    // with the cleanup task).
    let alive = lobby_exists && killed.is_none();
    let body = serde_json::json!({
        "alive": alive,
        "killed_reason": killed.map(|k| k.reason),
    });
    axum::Json(body).into_response()
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
