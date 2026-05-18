//! Web server entry point: user auth, profiles, and game history.

use std::sync::Arc;

use axum::{
    Router,
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
mod seed;

#[cfg(test)]
mod tests;

use routes::AppState;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect("sqlite:tetris.db?mode=rwc")
        .await?;

    db::init_schema(&pool).await?;
    seed::seed_if_empty(&pool).await?;

    let session_store = SqliteStore::new(pool.clone());
    session_store.migrate().await?;

    let session_layer = SessionManagerLayer::new(session_store);

    let backend = auth::Backend::new(pool.clone());
    let auth_layer = AuthManagerLayerBuilder::new(backend, session_layer).build();

    let env = build_template_env();

    let state = AppState {
        pool,
        env: Arc::new(env),
    };

    let protected = Router::new()
        .route("/users", get(routes::get_users))
        .route("/users/{id}", get(routes::get_user_detail))
        .route("/online", get(routes::get_online))
        .route("/logout", post(routes::post_logout))
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

fn build_template_env() -> Environment<'static> {
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

    env
}
