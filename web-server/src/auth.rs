//! Authentication backend for axum-login.

use axum_login::{AuthUser, AuthnBackend, UserId};
use password_auth::verify_password;
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::db::{self, User};

impl AuthUser for User {
    type Id = i64;

    fn id(&self) -> Self::Id {
        self.id
    }

    fn session_auth_hash(&self) -> &[u8] {
        self.password_hash.as_bytes()
    }
}

/// Credentials submitted at login.
#[derive(Debug, Clone, Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
    pub next: Option<String>,
}

/// SQLite-backed authentication backend.
#[derive(Debug, Clone)]
pub struct Backend {
    pool: SqlitePool,
}

impl Backend {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

/// Errors that can occur during authentication.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
}

impl AuthnBackend for Backend {
    type User = User;
    type Credentials = Credentials;
    type Error = Error;

    async fn authenticate(
        &self,
        creds: Self::Credentials,
    ) -> Result<Option<Self::User>, Self::Error> {
        let user = db::get_user_by_username(&self.pool, &creds.username).await?;
        let Some(user) = user else { return Ok(None) };
        let password = creds.password.clone();
        let hash = user.password_hash.clone();
        let ok = tokio::task::spawn_blocking(move || verify_password(&password, &hash).is_ok())
            .await
            .unwrap_or(false);
        Ok(if ok { Some(user) } else { None })
    }

    async fn get_user(&self, user_id: &UserId<Self>) -> Result<Option<Self::User>, Self::Error> {
        Ok(db::get_user_by_id(&self.pool, *user_id).await?)
    }
}

/// Alias for the auth session used throughout handlers.
pub type AuthSession = axum_login::AuthSession<Backend>;
