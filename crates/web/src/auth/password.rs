use std::sync::Arc;
use std::time::{Duration, Instant};

use argon2::{Argon2, PasswordHash, PasswordVerifier};
use seedling_protocol::actor::Actor;

// w[auth.password]
pub fn verify_password(hash_str: &str, password: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash_str) else {
        tracing::warn!("configured password_hash is not a valid PHC string");
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

pub struct SessionEntry {
    pub actor: Arc<Actor>,
    pub expires: Instant,
}

pub type Sessions = parking_lot::Mutex<std::collections::HashMap<String, SessionEntry>>;

// w[auth.password]
pub fn issue_session_token(sessions: &Sessions, actor: Arc<Actor>, lifetime: Duration) -> String {
    let token = uuid::Uuid::new_v4().to_string();
    let expires = Instant::now() + lifetime;

    let mut map = sessions.lock();
    // Prune expired entries opportunistically.
    map.retain(|_, v| v.expires > Instant::now());
    map.insert(token.clone(), SessionEntry { actor, expires });
    token
}

pub fn verify_session_token(sessions: &Sessions, token: &str) -> Option<Arc<Actor>> {
    let map = sessions.lock();
    let entry = map.get(token)?;
    if entry.expires <= Instant::now() {
        return None;
    }
    Some(Arc::clone(&entry.actor))
}
