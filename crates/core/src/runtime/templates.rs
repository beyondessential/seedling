use rusqlite::{OptionalExtension, params};
use seedling_protocol::names::TemplateName;

use crate::runtime::db::Db;

// i[impl template.definition]
#[derive(Debug, Clone)]
pub struct Template {
    pub name: TemplateName,
    pub body: String,
    pub description: Option<String>,
    pub created_at: String,
}

fn row_to_template(row: &rusqlite::Row<'_>) -> rusqlite::Result<Template> {
    Ok(Template {
        name: row.get(0)?,
        body: row.get(1)?,
        description: row.get(2)?,
        created_at: row.get(3)?,
    })
}

// i[impl template.create]
pub fn create(db: &Db, t: &Template) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT INTO templates (name, body, description, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![t.name, t.body, t.description, t.created_at],
    )?;
    Ok(())
}

// i[impl template.list]
pub fn list(db: &Db) -> rusqlite::Result<Vec<Template>> {
    let mut stmt = db
        .conn
        .prepare("SELECT name, body, description, created_at FROM templates ORDER BY name")?;
    let rows = stmt.query_map([], row_to_template)?;
    rows.collect()
}

// i[impl template.show]
pub fn get(db: &Db, name: &TemplateName) -> rusqlite::Result<Option<Template>> {
    db.conn
        .query_row(
            "SELECT name, body, description, created_at FROM templates WHERE name = ?1",
            params![name],
            row_to_template,
        )
        .optional()
}

// i[impl template.remove]
pub fn delete(db: &Db, name: &TemplateName) -> rusqlite::Result<bool> {
    let n = db
        .conn
        .execute("DELETE FROM templates WHERE name = ?1", params![name])?;
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
            body: body.to_owned(),
            description: None,
            created_at: "2026-04-23T00:00:00Z".to_owned(),
        }
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
                body: "app.deployment(\"web\");".to_owned(),
                description: Some("basic nginx".to_owned()),
                created_at: "2026-04-23T00:00:00Z".to_owned(),
            },
        )
        .unwrap();
        let got = get(&db, &TemplateName::new_unchecked("nginx-stack"))
            .unwrap()
            .unwrap();
        assert_eq!(got.body, "app.deployment(\"web\");");
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
}
