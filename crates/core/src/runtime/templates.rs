use std::sync::Arc;

use rusqlite::{OptionalExtension, params};
use seedling_protocol::names::TemplateName;

use crate::runtime::{
    db::Db,
    definition::{Bundle, Source},
    generations,
};

// i[impl template.definition]
#[derive(Debug, Clone)]
pub struct Template {
    pub name: TemplateName,
    pub bundle: Arc<Bundle>,
    pub source: Source,
    pub description: Option<String>,
    pub created_at: String,
}

/// Why a template could not be read.
#[derive(Debug)]
pub enum Error {
    Db(rusqlite::Error),
    Bundle(generations::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(e) => write!(f, "database error: {e}"),
            Self::Bundle(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
    }
}

impl From<generations::Error> for Error {
    fn from(e: generations::Error) -> Self {
        Self::Bundle(e)
    }
}

struct Row {
    name: TemplateName,
    bundle_hash: String,
    provenance: Option<String>,
    description: Option<String>,
    created_at: String,
}

const COLUMNS: &str = "name, bundle_hash, provenance, description, created_at";

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok(Row {
        name: row.get(0)?,
        bundle_hash: row.get(1)?,
        provenance: row.get(2)?,
        description: row.get(3)?,
        created_at: row.get(4)?,
    })
}

fn load(db: &Db, row: Row) -> Result<Template, Error> {
    let source = match row.provenance.as_deref() {
        Some(text) => Source::from_db(text).map_err(|e| {
            Error::Bundle(generations::Error::CorruptBundle(format!(
                "unreadable provenance: {e}"
            )))
        })?,
        None => Source::unknown_push(),
    };
    Ok(Template {
        name: row.name,
        bundle: generations::load_bundle(db, &row.bundle_hash)?,
        source,
        description: row.description,
        created_at: row.created_at,
    })
}

// i[impl template.create]
pub fn create(db: &Db, t: &Template) -> rusqlite::Result<()> {
    let tx = db.conn.unchecked_transaction()?;
    generations::store_bundle(db, &t.bundle)?;
    db.conn.execute(
        "INSERT INTO templates (name, bundle_hash, provenance, description, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            t.name,
            t.bundle.hash(),
            t.source.to_db(),
            t.description,
            t.created_at
        ],
    )?;
    tx.commit()
}

// i[impl template.list]
pub fn list(db: &Db) -> Result<Vec<Template>, Error> {
    let rows: Vec<Row> = {
        let mut stmt = db
            .conn
            .prepare(&format!("SELECT {COLUMNS} FROM templates ORDER BY name"))?;
        stmt.query_map([], read_row)?
            .collect::<rusqlite::Result<_>>()?
    };
    rows.into_iter().map(|r| load(db, r)).collect()
}

// i[impl template.show]
pub fn get(db: &Db, name: &TemplateName) -> Result<Option<Template>, Error> {
    let row = db
        .conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM templates WHERE name = ?1"),
            params![name],
            read_row,
        )
        .optional()?;
    row.map(|r| load(db, r)).transpose()
}

// i[impl template.update]
pub struct UpdateFields<'a> {
    pub definition: Option<(&'a Bundle, &'a Source)>,
    pub description: Option<Option<&'a str>>,
}

// i[impl template.update]
pub fn update(db: &Db, name: &TemplateName, fields: UpdateFields<'_>) -> rusqlite::Result<bool> {
    let mut sets: Vec<&'static str> = Vec::new();
    let mut values: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    let tx = db.conn.unchecked_transaction()?;
    if let Some((bundle, source)) = fields.definition {
        generations::store_bundle(db, bundle)?;
        sets.push("bundle_hash = ?");
        values.push(Box::new(bundle.hash().to_owned()));
        sets.push("provenance = ?");
        values.push(Box::new(source.to_db()));
    }
    if let Some(description) = fields.description {
        sets.push("description = ?");
        values.push(Box::new(description.map(str::to_owned)));
    }

    if sets.is_empty() {
        return exists(db, name);
    }

    let sql = format!("UPDATE templates SET {} WHERE name = ?", sets.join(", "));
    values.push(Box::new(name.clone()));
    let params: Vec<&dyn rusqlite::ToSql> = values.iter().map(|b| b.as_ref()).collect();
    let n = db.conn.execute(&sql, params.as_slice())?;
    generations::gc_bundles(db)?;
    tx.commit()?;
    Ok(n > 0)
}

// i[impl template.remove]
pub fn delete(db: &Db, name: &TemplateName) -> rusqlite::Result<bool> {
    let tx = db.conn.unchecked_transaction()?;
    let n = db
        .conn
        .execute("DELETE FROM templates WHERE name = ?1", params![name])?;
    generations::gc_bundles(db)?;
    tx.commit()?;
    Ok(n > 0)
}

pub fn exists(db: &Db, name: &TemplateName) -> rusqlite::Result<bool> {
    db.conn
        .query_row(
            "SELECT 1 FROM templates WHERE name = ?1",
            params![name],
            |_| Ok(()),
        )
        .optional()
        .map(|o| o.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template(name: &str, body: &str) -> Template {
        Template {
            name: TemplateName::new_unchecked(name),
            bundle: script(body),
            source: Source::unknown_push(),
            description: None,
            created_at: "2026-04-23T00:00:00Z".to_owned(),
        }
    }

    fn script(body: &str) -> Arc<Bundle> {
        Arc::new(Bundle::from_stored_script(body))
    }

    fn body_of(t: &Template) -> String {
        t.bundle.script().unwrap().text().trim_end().to_owned()
    }

    // i[verify template.definition]
    // i[verify template.create]
    #[test]
    fn create_stores_template_body() {
        let db = Db::open_in_memory().unwrap();
        create(
            &db,
            &Template {
                name: TemplateName::new_unchecked("nginx-stack"),
                bundle: script("app.deployment(\"web\");"),
                source: Source::unknown_push(),
                description: Some("basic nginx".to_owned()),
                created_at: "2026-04-23T00:00:00Z".to_owned(),
            },
        )
        .unwrap();
        let got = get(&db, &TemplateName::new_unchecked("nginx-stack"))
            .unwrap()
            .unwrap();
        assert_eq!(body_of(&got), "app.deployment(\"web\");");
        assert_eq!(got.description.as_deref(), Some("basic nginx"));
    }

    // i[verify template.create]
    #[test]
    fn create_duplicate_name_errors() {
        let db = Db::open_in_memory().unwrap();
        create(&db, &template("dup", "body1")).unwrap();
        assert!(create(&db, &template("dup", "body2")).is_err());
    }

    // i[verify template.list]
    #[test]
    fn list_empty_returns_empty() {
        let db = Db::open_in_memory().unwrap();
        assert!(list(&db).unwrap().is_empty());
    }

    // i[verify template.list]
    #[test]
    fn list_returns_templates_ordered_by_name() {
        let db = Db::open_in_memory().unwrap();
        create(&db, &template("zeta", "b1")).unwrap();
        create(&db, &template("alpha", "b2")).unwrap();
        create(&db, &template("mu", "b3")).unwrap();
        let got: Vec<_> = list(&db).unwrap().into_iter().map(|t| t.name).collect();
        assert_eq!(got, vec!["alpha", "mu", "zeta"]);
    }

    // i[verify template.show]
    #[test]
    fn show_returns_none_for_unknown() {
        let db = Db::open_in_memory().unwrap();
        assert!(
            get(&db, &TemplateName::new_unchecked("ghost"))
                .unwrap()
                .is_none()
        );
    }

    // i[verify template.remove]
    #[test]
    fn remove_existing_returns_true_and_deletes() {
        let db = Db::open_in_memory().unwrap();
        create(&db, &template("gone", "b")).unwrap();
        assert!(delete(&db, &TemplateName::new_unchecked("gone")).unwrap());
        assert!(
            get(&db, &TemplateName::new_unchecked("gone"))
                .unwrap()
                .is_none()
        );
    }

    // i[verify template.remove]
    #[test]
    fn remove_absent_returns_false() {
        let db = Db::open_in_memory().unwrap();
        assert!(!delete(&db, &TemplateName::new_unchecked("ghost")).unwrap());
    }

    // i[verify template.update]
    #[test]
    fn update_replaces_body_and_description() {
        let db = Db::open_in_memory().unwrap();
        create(
            &db,
            &Template {
                name: TemplateName::new_unchecked("nginx-stack"),
                bundle: script("old body"),
                source: Source::unknown_push(),
                description: Some("old desc".to_owned()),
                created_at: "2026-04-23T00:00:00Z".to_owned(),
            },
        )
        .unwrap();
        let ok = update(
            &db,
            &TemplateName::new_unchecked("nginx-stack"),
            UpdateFields {
                definition: Some((&script("new body"), &Source::unknown_push())),
                description: Some(Some("new desc")),
            },
        )
        .unwrap();
        assert!(ok);
        let got = get(&db, &TemplateName::new_unchecked("nginx-stack"))
            .unwrap()
            .unwrap();
        assert_eq!(body_of(&got), "new body");
        assert_eq!(got.description.as_deref(), Some("new desc"));
        assert_eq!(got.created_at, "2026-04-23T00:00:00Z");
    }

    // i[verify template.update]
    #[test]
    fn update_body_only_leaves_description_untouched() {
        let db = Db::open_in_memory().unwrap();
        create(
            &db,
            &Template {
                name: TemplateName::new_unchecked("t"),
                bundle: script("b1"),
                source: Source::unknown_push(),
                description: Some("keep me".to_owned()),
                created_at: "2026-04-23T00:00:00Z".to_owned(),
            },
        )
        .unwrap();
        update(
            &db,
            &TemplateName::new_unchecked("t"),
            UpdateFields {
                definition: Some((&script("b2"), &Source::unknown_push())),
                description: None,
            },
        )
        .unwrap();
        let got = get(&db, &TemplateName::new_unchecked("t"))
            .unwrap()
            .unwrap();
        assert_eq!(body_of(&got), "b2");
        assert_eq!(got.description.as_deref(), Some("keep me"));
    }

    // i[verify template.update]
    #[test]
    fn update_description_to_null_clears_it() {
        let db = Db::open_in_memory().unwrap();
        create(
            &db,
            &Template {
                name: TemplateName::new_unchecked("t"),
                bundle: script("b"),
                source: Source::unknown_push(),
                description: Some("initial".to_owned()),
                created_at: "2026-04-23T00:00:00Z".to_owned(),
            },
        )
        .unwrap();
        update(
            &db,
            &TemplateName::new_unchecked("t"),
            UpdateFields {
                definition: None,
                description: Some(None),
            },
        )
        .unwrap();
        let got = get(&db, &TemplateName::new_unchecked("t"))
            .unwrap()
            .unwrap();
        assert!(got.description.is_none());
    }

    // i[verify template.update]
    #[test]
    fn update_absent_returns_false() {
        let db = Db::open_in_memory().unwrap();
        let ok = update(
            &db,
            &TemplateName::new_unchecked("ghost"),
            UpdateFields {
                definition: Some((&script("x"), &Source::unknown_push())),
                description: None,
            },
        )
        .unwrap();
        assert!(!ok);
    }
}
