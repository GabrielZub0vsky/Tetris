//! Authentication backend for axum-login.
//!
//! Wires our SQLite `users` table into the `axum-login` session/auth layer
//! used by every protected route. Three pieces fit together here:
//!
//! 1. `impl AuthUser for User` — tells `axum-login` how to identify a user
//!    in a session blob and how to detect a stale session.
//! 2. `Backend` — does the actual SQL lookups and password verification.
//! 3. `AuthSession` — the per-request extractor handlers use to read the
//!    currently logged-in user.

use axum_login::{AuthUser, AuthnBackend, UserId};
use password_auth::verify_password;
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::db::{self, User};

/// Make our `User` row usable as an `axum-login` principal.
///
/// `axum-login` stores `id()` in the session blob and re-fetches the user
/// each request via `Backend::get_user`. `session_auth_hash()` is also
/// stored in the session and re-compared on every request — if the user
/// changes their password, the hash diverges and the session is killed,
/// kicking out anyone holding a stolen cookie.
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
    /// Construct a Backend that will read users from the given pool.
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

/// The `axum-login` plumbing for our `Backend`.
///
/// `authenticate` is invoked when the user POSTs to `/login`; `get_user`
/// runs on every subsequent request that carries a valid session, to
/// reload the user behind the session id.
impl AuthnBackend for Backend {
    type User = User;
    type Credentials = Credentials;
    type Error = Error;

    /// Verify a username/password pair against the database.
    ///
    /// Returns:
    ///   * `Ok(Some(user))` if the username exists and the password matches
    ///   * `Ok(None)` for any "wrong credentials" case (unknown user, bad
    ///     password) — never leaks which one
    ///   * `Err(_)` only if the database itself fails
    ///
    /// `verify_password` runs inside `spawn_blocking` because argon2/bcrypt
    /// burns ~100 ms of CPU per call; doing that on the tokio runtime would
    /// stall every other in-flight request.
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

    /// Re-load the user behind a session.
    ///
    /// `axum-login` calls this once per request after pulling `user_id` out
    /// of the session blob. The result is what `AuthSession.user` exposes
    /// to handlers. Returning `Ok(None)` makes axum-login treat the session
    /// as anonymous (e.g. user row was deleted).
    async fn get_user(&self, user_id: &UserId<Self>) -> Result<Option<Self::User>, Self::Error> {
        Ok(db::get_user_by_id(&self.pool, *user_id).await?)
    }
}

/// Alias for the auth session used throughout handlers.
pub type AuthSession = axum_login::AuthSession<Backend>;
