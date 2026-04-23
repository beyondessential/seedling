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

#[cfg(test)]
mod tests {
    use super::*;
    use argon2::PasswordHasher;
    use argon2::password_hash::SaltString;
    use std::collections::HashMap;

    fn hash(password: &str) -> String {
        let salt = SaltString::encode_b64(b"testsalt0123456").unwrap();
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .unwrap()
            .to_string()
    }

    fn empty_sessions() -> Sessions {
        parking_lot::Mutex::new(HashMap::new())
    }

    fn test_actor() -> Arc<Actor> {
        Arc::new(Actor {
            kind: Some("password".to_owned()),
            id: Some("admin".to_owned()),
            display: Some("admin".to_owned()),
            session: Some("session-id".to_owned()),
        })
    }

    // w[verify auth.password]
    #[test]
    fn verify_password_accepts_correct_password() {
        let h = hash("correct horse battery staple");
        assert!(verify_password(&h, "correct horse battery staple"));
    }

    // w[verify auth.password]
    #[test]
    fn verify_password_rejects_wrong_password() {
        let h = hash("correct horse battery staple");
        assert!(!verify_password(&h, "wrong password"));
    }

    // w[verify auth.password]
    #[test]
    fn verify_password_rejects_malformed_hash() {
        // Not a PHC string — must return false, not panic.
        assert!(!verify_password("not a phc hash", "anything"));
        assert!(!verify_password("", "anything"));
    }

    // w[verify auth.password]
    #[test]
    fn issue_then_verify_round_trips_actor() {
        let sessions = empty_sessions();
        let actor = test_actor();
        let token = issue_session_token(&sessions, Arc::clone(&actor), Duration::from_secs(60));

        let recovered = verify_session_token(&sessions, &token).expect("token should resolve");
        assert_eq!(recovered.id, actor.id);
    }

    // w[verify auth.password]
    #[test]
    fn verify_unknown_token_returns_none() {
        let sessions = empty_sessions();
        assert!(verify_session_token(&sessions, "no-such-token").is_none());
    }

    // w[verify auth.password]
    #[test]
    fn expired_token_does_not_verify() {
        let sessions = empty_sessions();
        let actor = test_actor();
        // Issue with a zero-second lifetime so the entry is already expired.
        let token = issue_session_token(&sessions, actor, Duration::from_secs(0));
        // Wait a beat so Instant::now() has definitely advanced past expires.
        std::thread::sleep(Duration::from_millis(1));
        assert!(verify_session_token(&sessions, &token).is_none());
    }

    // w[verify auth.password]
    #[test]
    fn issuing_new_token_prunes_expired_entries() {
        let sessions = empty_sessions();
        // Stale entry with a lifetime of zero — will be pruned on next issue.
        let stale_token = issue_session_token(&sessions, test_actor(), Duration::from_secs(0));
        std::thread::sleep(Duration::from_millis(1));
        assert_eq!(sessions.lock().len(), 1);

        // Issuing a fresh long-lived token prunes the stale one.
        let _fresh = issue_session_token(&sessions, test_actor(), Duration::from_secs(60));
        let map = sessions.lock();
        assert_eq!(map.len(), 1, "stale entry should have been pruned");
        assert!(!map.contains_key(&stale_token));
    }

    // w[verify auth.password]
    #[test]
    fn issued_tokens_are_unique() {
        let sessions = empty_sessions();
        let a = issue_session_token(&sessions, test_actor(), Duration::from_secs(60));
        let b = issue_session_token(&sessions, test_actor(), Duration::from_secs(60));
        assert_ne!(a, b);
    }
}
