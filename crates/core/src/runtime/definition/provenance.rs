use seedling_protocol::actor::Actor;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::bundle::Bundle;

/// Where the client says it obtained a pushed bundle. Recorded as reported,
/// never verified.
// i[impl definition.source]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    pub url: String,
    pub revision: String,
}

/// How a definition reached the runtime: the part of its provenance that is
/// not derived from the bundle itself.
// i[impl definition.provenance]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    Fetched {
        reference: String,
        digest: String,
    },
    Pushed {
        pushed_by: Option<Actor>,
        reported_origin: Option<Origin>,
    },
}

impl Source {
    /// Provenance for a definition stored before provenance was recorded:
    /// it was pushed, by nobody known.
    pub fn unknown_push() -> Self {
        Self::Pushed {
            pushed_by: None,
            reported_origin: None,
        }
    }

    pub fn to_db(&self) -> String {
        serde_json::to_string(self).expect("provenance serialises")
    }

    pub fn from_db(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// The tag-bearing reference a re-check should resolve, if this
    /// definition has one.
    // r[impl definition.recheck]
    pub fn recheckable_reference(&self) -> Option<&str> {
        match self {
            Self::Fetched { reference, .. } if !reference.contains('@') => Some(reference),
            _ => None,
        }
    }

    /// The full provenance object for a definition with this source.
    // i[impl definition.provenance]
    pub fn to_json(&self, bundle: &Bundle) -> Value {
        self.to_json_with(
            bundle.hash(),
            bundle.seedling_versions().map(|r| r.as_str()),
        )
    }

    /// The same, for a caller holding the two bundle-derived fields without
    /// the bundle: a history page already has the content hash in the row it
    /// is listing, and reads the requirement on its own rather than
    /// materialising every definition it names.
    // i[impl definition.provenance]
    pub fn to_json_with(&self, content_hash: &str, seedling_versions: Option<&str>) -> Value {
        let mut obj = match serde_json::to_value(self).expect("provenance serialises") {
            Value::Object(map) => map,
            _ => unreachable!("an internally tagged enum serialises as an object"),
        };
        obj.insert("content_hash".into(), json!(content_hash));
        obj.insert("seedling_versions".into(), json!(seedling_versions));
        Value::Object(obj)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // i[verify definition.provenance]
    #[test]
    fn pushed_provenance_carries_actor_origin_and_hash() {
        let bundle = Bundle::from_files([
            ("app.seed.rhai".to_owned(), b"app;".to_vec()),
            (
                "seedling.toml".to_owned(),
                b"seedling = \">=0.12\"".to_vec(),
            ),
        ])
        .unwrap();
        let source = Source::Pushed {
            pushed_by: Some(Actor {
                kind: Some("ctl".into()),
                id: Some("fp".into()),
                display: None,
                session: None,
            }),
            reported_origin: Some(Origin {
                url: "https://github.com/o/r/tree/main/app".into(),
                revision: "abc123".into(),
            }),
        };
        let v = source.to_json(&bundle);
        assert_eq!(v["kind"], "pushed");
        assert_eq!(v["pushed_by"]["kind"], "ctl");
        assert_eq!(v["reported_origin"]["revision"], "abc123");
        assert_eq!(v["content_hash"], bundle.hash());
        assert_eq!(v["seedling_versions"], ">=0.12");
        assert_eq!(Source::from_db(&source.to_db()).unwrap(), source);
    }

    // i[verify definition.provenance]
    #[test]
    fn fetched_provenance_carries_reference_and_digest() {
        let bundle = Bundle::from_script("app;").unwrap();
        let v = Source::Fetched {
            reference: "ghcr.io/o/app-def:1.2".into(),
            digest: "sha256:abc".into(),
        }
        .to_json(&bundle);
        assert_eq!(v["kind"], "fetched");
        assert_eq!(v["reference"], "ghcr.io/o/app-def:1.2");
        assert_eq!(v["digest"], "sha256:abc");
        assert_eq!(v["seedling_versions"], Value::Null);
    }

    // r[verify definition.recheck]
    #[test]
    fn only_tagged_fetches_are_rechecked() {
        let tagged = Source::Fetched {
            reference: "ghcr.io/o/d:1".into(),
            digest: "sha256:a".into(),
        };
        let pinned = Source::Fetched {
            reference: "ghcr.io/o/d@sha256:a".into(),
            digest: "sha256:a".into(),
        };
        assert_eq!(tagged.recheckable_reference(), Some("ghcr.io/o/d:1"));
        assert_eq!(pinned.recheckable_reference(), None);
        assert_eq!(Source::unknown_push().recheckable_reference(), None);
    }
}
