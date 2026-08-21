use crate::error::{AppError, AppResult};
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Connection;

pub type Pool = r2d2::Pool<SqliteConnectionManager>;

/// Opens (creating if needed) the SQLite database and applies migrations.
pub fn open(path: &std::path::Path) -> Result<Pool, String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {parent:?}: {e}"))?;
        }
    }
    let manager = SqliteConnectionManager::file(path).with_init(|c| {
        c.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             PRAGMA synchronous = NORMAL;",
        )
    });
    let pool = r2d2::Pool::builder()
        .max_size(8)
        .build(manager)
        .map_err(|e| format!("cannot open database {path:?}: {e}"))?;

    let mut conn = pool.get().map_err(|e| e.to_string())?;
    migrate(&mut conn).map_err(|e| format!("migration failed: {e}"))?;
    Ok(pool)
}

/// Schema migrations, tracked with SQLite's `user_version`.
pub(crate) fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;

    if version < 1 {
        let tx = conn.transaction()?;
        tx.execute_batch(
            r#"
            CREATE TABLE photos (
                id          INTEGER PRIMARY KEY,
                mime        TEXT NOT NULL,
                bytes       BLOB NOT NULL,
                created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );

            -- A container is anything that holds things: an area, a shelf unit,
            -- a cabinet, a drawer, or a cardboard box. They nest via parent_id,
            -- so "Garage / North wall shelves / Bin 3" is just three rows.
            CREATE TABLE containers (
                id          INTEGER PRIMARY KEY,
                code        TEXT NOT NULL UNIQUE COLLATE NOCASE,
                name        TEXT NOT NULL,
                kind        TEXT NOT NULL DEFAULT 'box',
                parent_id   INTEGER REFERENCES containers(id) ON DELETE SET NULL,
                notes       TEXT NOT NULL DEFAULT '',
                photo_id    INTEGER REFERENCES photos(id) ON DELETE SET NULL,
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX idx_containers_parent ON containers(parent_id);

            CREATE TABLE items (
                id           INTEGER PRIMARY KEY,
                name         TEXT NOT NULL,
                description  TEXT NOT NULL DEFAULT '',
                quantity     INTEGER NOT NULL DEFAULT 1,
                container_id INTEGER REFERENCES containers(id) ON DELETE SET NULL,
                photo_id     INTEGER REFERENCES photos(id) ON DELETE SET NULL,
                created_at   TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX idx_items_container ON items(container_id);
            CREATE INDEX idx_items_name ON items(name COLLATE NOCASE);

            CREATE TABLE tags (
                id    INTEGER PRIMARY KEY,
                name  TEXT NOT NULL UNIQUE COLLATE NOCASE
            );
            CREATE TABLE item_tags (
                item_id  INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
                tag_id   INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
                PRIMARY KEY (item_id, tag_id)
            );
            CREATE INDEX idx_item_tags_tag ON item_tags(tag_id);

            CREATE TABLE settings (
                key    TEXT PRIMARY KEY,
                value  TEXT NOT NULL
            );
            "#,
        )?;
        tx.pragma_update(None, "user_version", 1)?;
        tx.commit()?;
    }

    if version < 2 {
        // When the contents of a container were last verified by eye. NULL
        // means "never checked since it was created".
        let tx = conn.transaction()?;
        tx.execute_batch("ALTER TABLE containers ADD COLUMN checked_at TEXT")?;
        tx.pragma_update(None, "user_version", 2)?;
        tx.commit()?;
    }
    Ok(())
}

/// Runs a blocking database closure on the blocking pool so async workers are
/// never parked on SQLite I/O.
pub async fn run<T, F>(pool: &Pool, f: F) -> AppResult<T>
where
    F: FnOnce(&mut Connection) -> AppResult<T> + Send + 'static,
    T: Send + 'static,
{
    let pool = pool.clone();
    tokio::task::spawn_blocking(move || {
        let mut conn = pool.get()?;
        f(&mut conn)
    })
    .await
    .map_err(|e| AppError::internal(format!("task panicked: {e}")))?
}

pub fn get_setting(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [key, value],
    )?;
    Ok(())
}
