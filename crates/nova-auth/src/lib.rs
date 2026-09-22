//! Password authentication, bearer sessions, and role authorization.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, MutexGuard};

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use nova_core::error::{NovaError, Result};

const SALT_BYTES: usize = 16;
const TOKEN_BYTES: usize = 32;
const MIN_PASSWORD_BYTES: usize = 8;
const MAX_PASSWORD_BYTES: usize = 1024;
const HEX: &[u8; 16] = b"0123456789abcdef";

/// Coarse database roles enforced before query execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    ReadOnly,
    ReadWrite,
    Admin,
}

/// Operation classes understood by authorization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Read,
    Write,
    Manage,
}

impl Role {
    /// Returns whether this role grants a permission.
    #[must_use]
    pub const fn allows(self, permission: Permission) -> bool {
        match self {
            Self::ReadOnly => matches!(permission, Permission::Read),
            Self::ReadWrite => matches!(permission, Permission::Read | Permission::Write),
            Self::Admin => true,
        }
    }
}

/// Authenticated identity returned by successful authorization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub username: String,
    pub role: Role,
}

/// Opaque process-local bearer session token.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionToken(String);

impl SessionToken {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Debug for SessionToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SessionToken([REDACTED])")
    }
}

#[derive(Debug)]
struct UserRecord {
    password_hash: String,
    role: Role,
    enabled: bool,
}

#[derive(Debug, Default)]
struct State {
    users: BTreeMap<String, UserRecord>,
    sessions: BTreeMap<String, String>,
}

/// Thread-safe authentication and deterministic role authorization service.
pub struct AuthService {
    state: Mutex<State>,
    dummy_hash: String,
}

impl fmt::Debug for AuthService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthService")
            .field("credentials", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl AuthService {
    /// Creates an empty service and a dummy hash used for unknown-user checks.
    ///
    /// # Errors
    /// Returns a typed entropy or password-hashing error.
    pub fn new() -> Result<Self> {
        Ok(Self {
            state: Mutex::new(State::default()),
            dummy_hash: hash_password("novadb-dummy-credential")?,
        })
    }

    /// Adds a user with a freshly salted Argon2id password hash.
    ///
    /// # Errors
    /// Returns a typed validation, duplicate-user, entropy, or hashing error.
    pub fn create_user(&self, username: &str, password: &str, role: Role) -> Result<()> {
        validate_username(username)?;
        validate_password(password)?;
        let password_hash = hash_password(password)?;
        let mut state = self.lock()?;
        if state.users.contains_key(username) {
            return Err(NovaError::AlreadyExists(format!("user {username:?}")));
        }
        state.users.insert(
            username.to_owned(),
            UserRecord {
                password_hash,
                role,
                enabled: true,
            },
        );
        Ok(())
    }

    /// Verifies credentials and creates a random bearer session.
    ///
    /// Unknown, disabled, and wrong-password cases intentionally return the
    /// same public error.
    ///
    /// # Errors
    /// Returns a generic auth error or a typed entropy/internal error.
    pub fn authenticate(&self, username: &str, password: &str) -> Result<SessionToken> {
        let mut state = self.lock()?;
        let (encoded, valid_user) = state
            .users
            .get(username)
            .map_or((self.dummy_hash.as_str(), false), |user| {
                (user.password_hash.as_str(), user.enabled)
            });
        let parsed = PasswordHash::new(encoded).map_err(|error| {
            NovaError::Internal(format!("stored password hash invalid: {error}"))
        })?;
        let verified = Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok();
        if !valid_user || !verified {
            return Err(NovaError::Auth("invalid credentials".to_owned()));
        }
        let token = random_token()?;
        state.sessions.insert(token.0.clone(), username.to_owned());
        Ok(token)
    }

    /// Resolves a bearer token and checks its role.
    ///
    /// # Errors
    /// Returns [`NovaError::Auth`] for invalid sessions and
    /// [`NovaError::PermissionDenied`] for insufficient roles.
    pub fn authorize(&self, token: &str, permission: Permission) -> Result<Principal> {
        let state = self.lock()?;
        let username = state
            .sessions
            .get(token)
            .ok_or_else(|| NovaError::Auth("invalid or expired session".to_owned()))?;
        let user = state
            .users
            .get(username)
            .filter(|user| user.enabled)
            .ok_or_else(|| NovaError::Auth("invalid or expired session".to_owned()))?;
        if !user.role.allows(permission) {
            return Err(NovaError::PermissionDenied(format!(
                "role {:?} does not grant {permission:?}",
                user.role
            )));
        }
        Ok(Principal {
            username: username.clone(),
            role: user.role,
        })
    }

    /// Revokes one bearer session. Revocation is idempotent.
    ///
    /// # Errors
    /// Returns a typed synchronization error.
    pub fn revoke(&self, token: &str) -> Result<()> {
        self.lock()?.sessions.remove(token);
        Ok(())
    }

    /// Changes a user's role and revokes every existing session for that user.
    ///
    /// # Errors
    /// Returns a typed unknown-user or synchronization error.
    pub fn set_role(&self, username: &str, role: Role) -> Result<()> {
        let mut state = self.lock()?;
        state
            .users
            .get_mut(username)
            .ok_or_else(|| NovaError::NotFound(format!("user {username:?}")))?
            .role = role;
        revoke_user_sessions(&mut state, username);
        Ok(())
    }

    /// Enables or disables a user, revoking sessions when disabled.
    ///
    /// # Errors
    /// Returns a typed unknown-user or synchronization error.
    pub fn set_enabled(&self, username: &str, enabled: bool) -> Result<()> {
        let mut state = self.lock()?;
        state
            .users
            .get_mut(username)
            .ok_or_else(|| NovaError::NotFound(format!("user {username:?}")))?
            .enabled = enabled;
        if !enabled {
            revoke_user_sessions(&mut state, username);
        }
        Ok(())
    }

    /// Replaces a password and revokes every existing session for the user.
    ///
    /// # Errors
    /// Returns a typed validation, unknown-user, entropy, or hashing error.
    pub fn change_password(&self, username: &str, password: &str) -> Result<()> {
        validate_password(password)?;
        let password_hash = hash_password(password)?;
        let mut state = self.lock()?;
        state
            .users
            .get_mut(username)
            .ok_or_else(|| NovaError::NotFound(format!("user {username:?}")))?
            .password_hash = password_hash;
        revoke_user_sessions(&mut state, username);
        Ok(())
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>> {
        self.state
            .lock()
            .map_err(|_| NovaError::Internal("authentication mutex poisoned".to_owned()))
    }
}

fn revoke_user_sessions(state: &mut State, username: &str) {
    state.sessions.retain(|_, owner| owner != username);
}

fn validate_username(username: &str) -> Result<()> {
    if username.is_empty()
        || username.len() > 64
        || !username
            .chars()
            .all(|character| character == '_' || character.is_alphanumeric())
    {
        return Err(NovaError::InvalidArgument(format!(
            "invalid username {username:?}"
        )));
    }
    Ok(())
}

fn validate_password(password: &str) -> Result<()> {
    if !(MIN_PASSWORD_BYTES..=MAX_PASSWORD_BYTES).contains(&password.len()) {
        return Err(NovaError::InvalidArgument(format!(
            "password must be {MIN_PASSWORD_BYTES} through {MAX_PASSWORD_BYTES} UTF-8 bytes"
        )));
    }
    Ok(())
}

fn hash_password(password: &str) -> Result<String> {
    let mut salt_bytes = [0_u8; SALT_BYTES];
    getrandom::fill(&mut salt_bytes)
        .map_err(|error| NovaError::Internal(format!("auth entropy failure: {error}")))?;
    let salt = SaltString::encode_b64(&salt_bytes)
        .map_err(|error| NovaError::Internal(format!("salt encoding failure: {error}")))?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| NovaError::Internal(format!("password hashing failure: {error}")))
}

fn random_token() -> Result<SessionToken> {
    let mut bytes = [0_u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes)
        .map_err(|error| NovaError::Internal(format!("auth entropy failure: {error}")))?;
    let mut token = String::with_capacity(TOKEN_BYTES * 2);
    for byte in bytes {
        token.push(char::from(HEX[usize::from(byte >> 4)]));
        token.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(SessionToken(token))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_sessions_and_revocation_are_enforced() {
        let auth = AuthService::new().unwrap();
        auth.create_user("reader", "correct horse", Role::ReadOnly)
            .unwrap();
        assert!(matches!(
            auth.authenticate("reader", "wrong password"),
            Err(NovaError::Auth(_))
        ));
        assert!(matches!(
            auth.authenticate("missing", "wrong password"),
            Err(NovaError::Auth(_))
        ));
        let token = auth.authenticate("reader", "correct horse").unwrap();
        assert_eq!(
            auth.authorize(token.as_str(), Permission::Read)
                .unwrap()
                .username,
            "reader"
        );
        assert!(matches!(
            auth.authorize(token.as_str(), Permission::Write),
            Err(NovaError::PermissionDenied(_))
        ));
        auth.revoke(token.as_str()).unwrap();
        assert!(matches!(
            auth.authorize(token.as_str(), Permission::Read),
            Err(NovaError::Auth(_))
        ));
    }

    #[test]
    fn roles_password_changes_and_disable_revoke_sessions() {
        let auth = AuthService::new().unwrap();
        auth.create_user("operator", "initial-password", Role::ReadWrite)
            .unwrap();
        let first = auth.authenticate("operator", "initial-password").unwrap();
        assert!(auth.authorize(first.as_str(), Permission::Write).is_ok());
        auth.set_role("operator", Role::Admin).unwrap();
        assert!(auth.authorize(first.as_str(), Permission::Read).is_err());
        let second = auth.authenticate("operator", "initial-password").unwrap();
        assert!(auth.authorize(second.as_str(), Permission::Manage).is_ok());
        auth.change_password("operator", "replacement-password")
            .unwrap();
        assert!(auth.authorize(second.as_str(), Permission::Read).is_err());
        assert!(auth.authenticate("operator", "initial-password").is_err());
        let third = auth
            .authenticate("operator", "replacement-password")
            .unwrap();
        auth.set_enabled("operator", false).unwrap();
        assert!(auth.authorize(third.as_str(), Permission::Read).is_err());
        assert!(auth
            .authenticate("operator", "replacement-password")
            .is_err());
    }

    #[test]
    fn validation_duplicates_and_token_redaction_are_explicit() {
        let auth = AuthService::new().unwrap();
        assert!(auth
            .create_user("bad/name", "long-enough", Role::Admin)
            .is_err());
        assert!(auth.create_user("admin", "short", Role::Admin).is_err());
        auth.create_user("admin", "long-enough", Role::Admin)
            .unwrap();
        assert!(matches!(
            auth.create_user("admin", "another-long", Role::Admin),
            Err(NovaError::AlreadyExists(_))
        ));
        let token = auth.authenticate("admin", "long-enough").unwrap();
        assert_eq!(token.as_str().len(), TOKEN_BYTES * 2);
        assert_eq!(format!("{token:?}"), "SessionToken([REDACTED])");
    }
}
