//! Route handlers for the web server.

use axum::{
    Form,
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use minijinja::Environment;
use password_auth::generate_hash;
use serde::Deserialize;
use sqlx::SqlitePool;
use std::sync::Arc;

use crate::{
    auth::{AuthSession, Credentials},
    db,
};

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub env: Arc<Environment<'static>>,
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

pub async fn get_login(auth_session: AuthSession, State(state): State<AppState>) -> Response {
    if auth_session.user.is_some() {
        return Redirect::to("/users").into_response();
    }
    render(
        &state.env,
        "login.html",
        minijinja::context! { error => "" },
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

    let hash = generate_hash(&form.password);
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
            minijinja::context! { error => "Username already taken." },
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

pub async fn get_users(auth_session: AuthSession, State(state): State<AppState>) -> Response {
    let current_user = auth_session.user.as_ref().map(|u| u.username.clone());
    match db::list_users(&state.pool).await {
        Ok(users) => render(
            &state.env,
            "users.html",
            minijinja::context! {
                users => users.iter().map(|u| minijinja::context! {
                    id => u.id,
                    username => u.username.clone(),
                    created_at => u.created_at.clone(),
                }).collect::<Vec<_>>(),
                current_user => current_user,
            },
        ),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
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
    let (wins, losses, draws) = db::get_user_stats(&state.pool, user_id)
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
            wins => wins,
            losses => losses,
            draws => draws,
            current_user => current_user,
        },
    )
}

// ── Currently online ─────────────────────────────────────────────────────────

pub async fn get_online(auth_session: AuthSession, State(state): State<AppState>) -> Response {
    let current_user = auth_session.user.as_ref().map(|u| {
        let _ = {
            let pool = state.pool.clone();
            let id = u.id;
            tokio::spawn(async move { db::touch_user(&pool, id).await });
        };
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
