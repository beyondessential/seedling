use rusqlite::{OptionalExtension, params};

use crate::runtime::db::Db;

// i[impl template.definition]
#[derive(Debug, Clone)]
pub struct Template {
    pub name: String,
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
pub fn get(db: &Db, name: &str) -> rusqlite::Result<Option<Template>> {
    db.conn
        .query_row(
            "SELECT name, body, description, created_at FROM templates WHERE name = ?1",
            [name],
            row_to_template,
        )
        .optional()
}

// i[impl template.remove]
pub fn delete(db: &Db, name: &str) -> rusqlite::Result<bool> {
    let n = db
        .conn
        .execute("DELETE FROM templates WHERE name = ?1", params![name])?;
    Ok(n > 0)
}

pub fn exists(db: &Db, name: &str) -> rusqlite::Result<bool> {
    db.conn
        .query_row(
            "SELECT 1 FROM templates WHERE name = ?1",
            [name],
            |_| Ok(()),
        )
        .optional()
        .map(|o| o.is_some())
}
