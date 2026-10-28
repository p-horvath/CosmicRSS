use crate::db;
use crate::feed::{Channel, Entry};
use anyhow::Result;
use atom_syndication::{Entry as AtomEntry, Feed as AtomFeed};
use chrono::{DateTime, Utc};
use rss::Channel as RssChannel;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time;

pub async fn fetch_url(url: &str) -> Result<Channel> {
    let xml = reqwest::get(url).await?.bytes().await?;
    parse_feed(url, &xml)
}

fn parse_feed(url: &str, xml: &[u8]) -> Result<Channel> {
    // RSS first: also handles RSS feeds carrying `atom:` extensions.
    // On parse error, assume Atom and retry.
    match RssChannel::read_from(xml) {
        Ok(rss) => Ok(Channel {
            id: url.to_string(),
            title: rss.title().to_string(),
            url: url.to_string(),
            entries: rss
                .items()
                .iter()
                .filter_map(|i| {
                    Some(Entry {
                        id: i.guid()?.value().to_string(),
                        channel: url.to_string(),
                        title: i.title()?.to_string(),
                        link: i.link()?.to_string(),
                        summary: i.description().map(|s| s.to_string()),
                        published: i
                            .pub_date()
                            .and_then(|d| DateTime::parse_from_rfc2822(d).ok())?
                            .with_timezone(&Utc),
                        read: false,
                    })
                })
                .collect(),
        }),
        Err(_) => {
            let atom = AtomFeed::read_from(xml)?; // `&[u8]` implements `BufRead`
            Ok(Channel {
                id: url.to_string(),
                title: atom.title.as_str().to_string(),
                url: url.to_string(),
                entries: atom
                    .entries
                    .iter()
                    .map(|e: &AtomEntry| Entry {
                        id: e.id.clone(),
                        channel: url.to_string(),
                        title: e.title.as_str().to_string(),
                        link: e.links
                            .iter()
                            .find(|l| l.rel == "alternate")
                            .map(|l| l.href.clone())
                            .unwrap_or_default(),
                        summary: e.summary.as_ref().map(|s| s.as_str().to_string()),
                        published: e
                            .published
                            .map(|d| d.with_timezone(&Utc))
                            .unwrap_or_else(|| e.updated.with_timezone(&Utc)),
                        read: false,
                    })
                    .collect(),
            })
        }
    }
}

async fn fetch_all(urls: &[String]) -> Vec<Channel> {
    futures::future::join_all(urls.iter().map(|u| fetch_url(u.as_str())))
        .await
        .into_iter()
        .flatten()
        .collect()
}


/// Events the sync loop reports to the UI: a cycle starting, and a cycle's
/// fetched channels (possibly empty if no feed was fetched).
pub enum SyncEvent {
    Start,
    Finished(Vec<Channel>),
}

pub async fn sync_loop(
    tx: mpsc::Sender<SyncEvent>,
    mut force_rx: mpsc::Receiver<()>,
) {
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Own connection to the DB so the feed list can be read on every round.
    // WAL mode allows this to coexist with the main app's connection.
    let conn = db::init();

    let mut interval = time::interval(Duration::from_secs(900));
    interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = interval.tick() => {},
            _ = force_rx.recv() => {
                while force_rx.try_recv().is_ok() {}
            },
        }

        // Always report start and finish (even with no channels fetched), so
        // the header spinner tracks each cycle and never gets stuck on.
        let _ = tx.send(SyncEvent::Start).await;

        let feeds: Vec<String> = db::load_channels(&conn)
            .unwrap_or_default()
            .into_iter()
            .map(|c| c.url)
            .collect();
        let channels = fetch_all(&feeds).await;
        let _ = tx.send(SyncEvent::Finished(channels)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::parse_feed;
    use chrono::{DateTime, Utc};

    const ATOM_SAMPLE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Test Atom Feed</title>
  <link rel="self" href="https://example.com/feed.atom"/>
  <id>https://example.com/feed.atom</id>
  <updated>2024-01-15T12:00:00Z</updated>
  <entry>
    <id>tag:example.com,2024:1</id>
    <title>Hello Atom</title>
    <link rel="alternate" href="https://example.com/post/1"/>
    <published>2024-01-15T10:30:00Z</published>
    <updated>2024-01-15T10:30:00Z</updated>
    <summary>An atom summary</summary>
  </entry>
</feed>"#;

    const RSS_SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Test RSS Feed</title>
    <link>https://example.com</link>
    <description>An RSS test feed</description>
    <item>
      <title>Hello RSS</title>
      <link>https://example.com/rss-post/1</link>
      <guid isPermaLink="false">https://example.com/rss-post/1</guid>
      <description>An RSS summary</description>
      <pubDate>Mon, 15 Jan 2024 10:30:00 +0000</pubDate>
    </item>
  </channel>
</rss>"#;

    #[test]
    fn parse_feed_atom_sample() {
        let channel = parse_feed("https://example.com/feed.atom", ATOM_SAMPLE.as_bytes())
            .expect("Atom sample should parse");
        assert_eq!(channel.id, "https://example.com/feed.atom");
        assert_eq!(channel.title, "Test Atom Feed");
        assert_eq!(channel.url, "https://example.com/feed.atom");
        assert_eq!(channel.entries.len(), 1);

        let e = &channel.entries[0];
        assert_eq!(e.id, "tag:example.com,2024:1");
        assert_eq!(e.title, "Hello Atom");
        assert_eq!(e.link, "https://example.com/post/1");
        assert_eq!(e.summary.as_deref(), Some("An atom summary"));
        assert_eq!(
            e.published,
            DateTime::parse_from_rfc3339("2024-01-15T10:30:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
    }

    #[test]
    fn parse_feed_rss_still_works() {
        let channel = parse_feed("https://example.com/feed.xml", RSS_SAMPLE.as_bytes())
            .expect("RSS sample should parse");
        assert_eq!(channel.id, "https://example.com/feed.xml");
        assert_eq!(channel.title, "Test RSS Feed");
        assert_eq!(channel.entries.len(), 1);

        let e = &channel.entries[0];
        assert_eq!(e.id, "https://example.com/rss-post/1");
        assert_eq!(e.title, "Hello RSS");
        assert_eq!(e.link, "https://example.com/rss-post/1");
        assert_eq!(e.summary.as_deref(), Some("An RSS summary"));
        assert_eq!(
            e.published,
            DateTime::parse_from_rfc3339("2024-01-15T10:30:00+00:00")
                .unwrap()
                .with_timezone(&Utc),
        );
    }

    #[test]
    fn parse_feed_garbage_errors() {
        assert!(parse_feed("https://example.com/bad", b"not a feed at all").is_err());
    }
}
