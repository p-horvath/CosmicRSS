use std::path::PathBuf;

use crate::feed::{Channel, Entry};
use anyhow::Context;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

/// Default feed set, seeded on first run so the sync loop has feeds to fetch.
const DEFAULT_FEEDS: &[&str] = &[
    "https://github.com/pop-os/cosmic-epoch/releases.atom",
    "https://blog.system76.com/rss.xml",
    "https://www.phoronix.com/rss.php",
    "https://this-week-in-rust.org/rss.xml"
];

/// Per-user data directory: `$XDG_DATA_HOME/com.example.cosmic-rss` (matching
/// `APP_ID`), defaulting to `~/.local/share/com.example.cosmic-rss`.
fn data_dir() -> PathBuf {
    let base = match std::env::var("XDG_DATA_HOME") {
        Ok(xdg) if !xdg.is_empty() => PathBuf::from(xdg),
        _ => {
            let home = std::env::var("HOME").unwrap_or_default();
            PathBuf::from(home).join(".local").join("share")
        }
    };
    base.join("com.example.cosmic-rss")
}

pub fn init() -> Connection {
    let dir = data_dir();
    std::fs::create_dir_all(&dir).expect("failed to create data directory");
    let conn = Connection::open(dir.join("rss.db")).unwrap();
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
            published TEXT,
            read      INTEGER NOT NULL DEFAULT 0
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
            // `read` is intentionally not in the DO UPDATE SET, so a
            // user's read state survives sync rounds.
            tx.execute(
                "INSERT INTO entry (id, channel, title, link, summary, published, read)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(id) DO UPDATE SET
                     channel = excluded.channel,
                     title = excluded.title,
                     link = excluded.link,
                     summary = excluded.summary,
                     published = excluded.published",
                params![
                    e.id,
                    c.id,
                    e.title,
                    e.link,
                    e.summary,
                    e.published.to_rfc3339(),
                    e.read as i64
                ],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn load_channels(conn: &Connection) -> anyhow::Result<Vec<Channel>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, title, url FROM channel ORDER BY title COLLATE NOCASE ASC",
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
    let published_str: String = r.get(5)?;
    let published = DateTime::parse_from_rfc3339(&published_str)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
            5, rusqlite::types::Type::Text, Box::new(e),
        ))?;
    Ok(Entry {
        id: r.get(0)?,
        channel: r.get(1)?,
        title: r.get(2)?,
        link: r.get(3)?,
        summary: r.get(4)?,
        published,
        read: r.get(6)?,
    })
}

pub fn load_page(conn: &Connection, offset: usize, limit: usize) -> anyhow::Result<Vec<Entry>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, channel, title, link, summary, published, read FROM entry
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
        "SELECT id, channel, title, link, summary, published, read FROM entry
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

/// Sets or clears the read flag for a single entry by id.
pub fn set_entry_read(conn: &Connection, id: &str, read: bool) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE entry SET read = ?1 WHERE id = ?2",
        params![read as i64, id],
    )?;
    Ok(())
}

/// Marks all of a channel's entries as read.
pub fn mark_channel_read(conn: &Connection, channel_id: &str) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE entry SET read = 1 WHERE channel = ?1",
        params![channel_id],
    )?;
    Ok(())
}

/// Counts unread entries, optionally restricted to one channel.
pub fn count_unread(conn: &Connection, channel_id: Option<&str>) -> anyhow::Result<usize> {
    let n: i64 = match channel_id {
        Some(cid) => conn.query_row(
            "SELECT COUNT(*) FROM entry WHERE channel = ?1 AND read = 0",
            params![cid],
            |r| r.get(0),
        )?,
        None => conn.query_row("SELECT COUNT(*) FROM entry WHERE read = 0", [], |r| r.get(0))?,
    };
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

#[cfg(test)]
mod tests {
    use super::{count_unread, init, load_page, set_entry_read, store};
    use crate::feed::{Channel, Entry};
    use chrono::{DateTime, Utc};

    const FEED_URL: &str = "https://example.com/feed.xml";

    fn entry(id: &str, read: bool) -> Entry {
        Entry {
            id: id.to_string(),
            channel: String::new(),
            title: format!("Title {id}"),
            link: format!("https://example.com/{id}"),
            summary: None,
            published: DateTime::parse_from_rfc3339("2024-01-15T10:30:00Z")
                .unwrap()
                .with_timezone(&Utc),
            read,
        }
    }

    fn channel(entries: Vec<Entry>) -> Channel {
        Channel {
            id: FEED_URL.to_string(),
            title: "Test Feed".to_string(),
            url: FEED_URL.to_string(),
            entries,
        }
    }

    // Single test fn: `XDG_DATA_HOME` is process-global and tests run in
    // parallel, so the whole lifecycle lives in one sequential test.
    #[test]
    fn read_state_survives_sync_and_round_trips() {
        // Isolate from the user's real database (`data_dir()` honors XDG_DATA_HOME).
        let dir = std::env::temp_dir().join(format!("cosmic-rss-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("XDG_DATA_HOME", &dir);

        let mut conn = init();

        // Fresh entries are stored unread.
        store(
            &mut conn,
            std::slice::from_ref(&channel(vec![entry("a", false), entry("b", false)])),
        )
        .unwrap();
        assert_eq!(count_unread(&conn, None).unwrap(), 2);

        // Mark "a" read; load_page and count_unread reflect it.
        set_entry_read(&conn, "a", true).unwrap();
        let page = load_page(&conn, 0, 10).unwrap();
        assert!(page.iter().find(|e| e.id == "a").unwrap().read);
        assert!(!page.iter().find(|e| e.id == "b").unwrap().read);
        // Entries carry their channel id so the UI can show the source feed.
        assert_eq!(
            page.iter().find(|e| e.id == "a").unwrap().channel,
            FEED_URL,
        );
        assert_eq!(count_unread(&conn, Some(FEED_URL)).unwrap(), 1);

        // Simulate the next sync round: the fetch re-delivers everything as
        // unread, and store() must not clobber the user's read flag.
        store(
            &mut conn,
            std::slice::from_ref(&channel(vec![entry("a", false), entry("b", false)])),
        )
        .unwrap();
        let page = load_page(&conn, 0, 10).unwrap();
        assert!(page.iter().find(|e| e.id == "a").unwrap().read);
        assert!(!page.iter().find(|e| e.id == "b").unwrap().read);
        assert_eq!(count_unread(&conn, None).unwrap(), 1);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::env::remove_var("XDG_DATA_HOME");
    }
}
