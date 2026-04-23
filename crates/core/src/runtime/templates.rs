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

// i[impl template.update]
pub struct UpdateFields<'a> {
    pub body: Option<&'a str>,
    pub description: Option<Option<&'a str>>,
}

// i[impl template.update]
pub fn update(db: &Db, name: &TemplateName, fields: UpdateFields<'_>) -> rusqlite::Result<bool> {
    let mut sets: Vec<&'static str> = Vec::new();
    let mut values: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(body) = fields.body {
        sets.push("body = ?");
        values.push(Box::new(body.to_owned()));
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
    Ok(n > 0)
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

    // i[verify template.update]
    #[test]
    fn update_replaces_body_and_description() {
        let db = Db::open_in_memory().unwrap();
        create(
            &db,
            &Template {
                name: TemplateName::new_unchecked("nginx-stack"),
                body: "old body".to_owned(),
                description: Some("old desc".to_owned()),
                created_at: "2026-04-23T00:00:00Z".to_owned(),
            },
        )
        .unwrap();
        let ok = update(
            &db,
            &TemplateName::new_unchecked("nginx-stack"),
            UpdateFields {
                body: Some("new body"),
                description: Some(Some("new desc")),
            },
        )
        .unwrap();
        assert!(ok);
        let got = get(&db, &TemplateName::new_unchecked("nginx-stack"))
            .unwrap()
            .unwrap();
        assert_eq!(got.body, "new body");
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
                body: "b1".to_owned(),
                description: Some("keep me".to_owned()),
                created_at: "2026-04-23T00:00:00Z".to_owned(),
            },
        )
        .unwrap();
        update(
            &db,
            &TemplateName::new_unchecked("t"),
            UpdateFields {
                body: Some("b2"),
                description: None,
            },
        )
        .unwrap();
        let got = get(&db, &TemplateName::new_unchecked("t"))
            .unwrap()
            .unwrap();
        assert_eq!(got.body, "b2");
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
                body: "b".to_owned(),
                description: Some("initial".to_owned()),
                created_at: "2026-04-23T00:00:00Z".to_owned(),
            },
        )
        .unwrap();
        update(
            &db,
            &TemplateName::new_unchecked("t"),
            UpdateFields {
                body: None,
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
                body: Some("x"),
                description: None,
            },
        )
        .unwrap();
        assert!(!ok);
    }
}
