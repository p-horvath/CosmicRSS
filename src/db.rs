use crate::feed::{Channel, Entry};
use anyhow::Context;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

/// Default feed set, seeded on first run so the sync loop has feeds to fetch.
const DEFAULT_FEEDS: &[&str] = &[
    "https://www.phoronix.com/rss.php",
    "https://blog.system76.com/rss.xml",
    "https://this-week-in-rust.org/rss.xml",
];

pub fn init() -> Connection {
    let conn = Connection::open("rss.db").unwrap();
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
        .unwrap();
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS channel (
            id    TEXT PRIMARY KEY,
            title TEXT,
            url   TEXT
        );
        CREATE TABLE IF NOT EXISTS entry (
            id        TEXT PRIMARY KEY,
            channel   TEXT,
            title     TEXT,
            link      TEXT,
            summary   TEXT,
            published TEXT
        );",
    )
    .unwrap();
    // First-run seed: insert the default feed set when the channel table is empty.
    // `title` starts as the URL and is corrected on the first successful fetch.
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM channel", [], |r| r.get(0))
        .unwrap();
    if count == 0 {
        for url in DEFAULT_FEEDS {
            conn.execute(
                "INSERT OR IGNORE INTO channel (id, title, url) VALUES (?1, ?2, ?3)",
                params![url, url, url],
            )
            .unwrap();
        }
    }
    conn
}

/// Adds a feed. `id` is the URL (matching the PK scheme); `title` starts as the
/// URL and is corrected on the first successful fetch by `store()`.
pub fn add_channel(conn: &Connection, url: &str) -> anyhow::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO channel (id, title, url) VALUES (?1, ?2, ?3)",
        params![url, url, url],
    )?;
    Ok(())
}

/// Sets a user-chosen title for a feed (rename). `store()` preserves any title
/// that differs from the URL placeholder, so the rename survives syncs.
pub fn set_channel_title(conn: &Connection, id: &str, title: &str) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE channel SET title = ?1 WHERE id = ?2",
        params![title, id],
    )?;
    Ok(())
}

/// Removes a feed and all of its entries.
pub fn remove_channel(conn: &mut Connection, id: &str) -> anyhow::Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM entry WHERE channel = ?1", params![id])?;
    tx.execute("DELETE FROM channel WHERE id = ?1", params![id])?;
    tx.commit()?;
    Ok(())
}

pub fn store(conn: &mut Connection, channels: &[Channel]) -> anyhow::Result<()> {
    let tx = conn.transaction()?;
    for c in channels {
        // Upsert: new channels get the feed's title; for existing rows, correct
        // the title only while it's still the URL placeholder, so user renames
        // (see `set_channel_title`) survive syncs.
        tx.execute(
            "INSERT INTO channel (id, title, url) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET
                 title = CASE WHEN channel.title = channel.url THEN excluded.title
                              ELSE channel.title END,
                 url = excluded.url",
            params![c.id, c.title, c.url],
        )?;
        for e in &c.entries {
            tx.execute(
                "INSERT OR REPLACE INTO entry (id, channel, title, link, summary, published)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![e.id, c.id, e.title, e.link, e.summary, e.published.to_rfc3339()],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn load_channels(conn: &Connection) -> anyhow::Result<Vec<Channel>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, title, url FROM channel ORDER BY title ASC",
    )?;
    let result = stmt
        .query_map([], |r| {
            Ok(Channel { id: r.get(0)?, title: r.get(1)?, url: r.get(2)?, entries: vec![] })
        })?
        .collect::<Result<Vec<_>, _>>()
        .context("load_channels")?;
    Ok(result)
}

fn parse_entry(r: &rusqlite::Row) -> rusqlite::Result<Entry> {
    let published_str: String = r.get(4)?;
    let published = DateTime::parse_from_rfc3339(&published_str)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
            4, rusqlite::types::Type::Text, Box::new(e),
        ))?;
    Ok(Entry { id: r.get(0)?, title: r.get(1)?, link: r.get(2)?, summary: r.get(3)?, published })
}

pub fn load_page(conn: &Connection, offset: usize, limit: usize) -> anyhow::Result<Vec<Entry>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, title, link, summary, published FROM entry
         ORDER BY published DESC LIMIT ?1 OFFSET ?2",
    )?;
    let result = stmt
        .query_map(params![limit as i64, offset as i64], parse_entry)?
        .collect::<Result<Vec<_>, _>>()
        .context("load_page")?;
    Ok(result)
}

pub fn load_page_for_channel(
    conn: &Connection,
    channel_id: &str,
    offset: usize,
    limit: usize,
) -> anyhow::Result<Vec<Entry>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, title, link, summary, published FROM entry
         WHERE channel = ?1
         ORDER BY published DESC LIMIT ?2 OFFSET ?3",
    )?;
    let result = stmt
        .query_map(params![channel_id, limit as i64, offset as i64], parse_entry)?
        .collect::<Result<Vec<_>, _>>()
        .context("load_page_for_channel")?;
    Ok(result)
}

pub fn count(conn: &Connection) -> anyhow::Result<usize> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM entry", [], |r| r.get(0))?;
    Ok(n as usize)
}

pub fn count_for_channel(conn: &Connection, channel_id: &str) -> anyhow::Result<usize> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM entry WHERE channel = ?1",
        params![channel_id],
        |r| r.get(0),
    )?;
    Ok(n as usize)
}
