// i[wire.actor]
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Actor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    // i[verify wire.actor]
    #[test]
    fn serialisation_omits_absent_fields() {
        assert_eq!(serde_json::to_value(Actor::default()).unwrap(), json!({}));

        let partial = Actor {
            kind: Some("ctl".into()),
            id: Some("fp123".into()),
            display: None,
            session: None,
        };
        assert_eq!(
            serde_json::to_value(&partial).unwrap(),
            json!({"kind": "ctl", "id": "fp123"})
        );
    }

    // i[verify wire.actor]
    #[test]
    fn all_fields_round_trip() {
        let full = Actor {
            kind: Some("password".into()),
            id: Some("user@example.com".into()),
            display: Some("User".into()),
            session: Some("sess-1".into()),
        };
        let wire = serde_json::to_value(&full).unwrap();
        assert_eq!(
            wire,
            json!({
                "kind": "password",
                "id": "user@example.com",
                "display": "User",
                "session": "sess-1",
            })
        );
        let back: Actor = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&back).unwrap(), wire);
    }
}
