use crate::db;
use crate::feed::{Channel, Entry};
use anyhow::Result;
use chrono::{DateTime, Utc};
use rss::Channel as RssChannel;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time;

pub async fn fetch_url(url: &str) -> Result<Channel> {
    let xml = reqwest::get(url).await?.bytes().await?;
    let rss = RssChannel::read_from(&xml[..])?;
    Ok(Channel {
        id: url.to_string(),
        title: rss.title().to_string(),
        url: url.to_string(),
        entries: rss
            .items()
            .iter()
            .filter_map(|i| {
                Some(Entry {
                    id: i.guid()?.value().to_string(),
                    title: i.title()?.to_string(),
                    link: i.link()?.to_string(),
                    summary: i.description().map(|s| s.to_string()),
                    published: i
                        .pub_date()
                        .and_then(|d| DateTime::parse_from_rfc2822(d).ok())?
                        .with_timezone(&Utc),
                })
            })
            .collect(),
    })
}

async fn fetch_all(urls: &[String]) -> Vec<Channel> {
    futures::future::join_all(urls.iter().map(|u| fetch_url(u.as_str())))
        .await
        .into_iter()
        .flatten()
        .collect()
}


pub async fn sync_loop(
    tx: mpsc::Sender<Vec<Channel>>,
    mut force_rx: mpsc::Receiver<()>,
) {
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Own connection to the DB so the feed list can be read on every round.
    // WAL mode allows this to coexist with the main app's connection.
    let conn = db::init();

    let mut interval = time::interval(Duration::from_secs(600));
    interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = interval.tick() => {},
            _ = force_rx.recv() => {
                while force_rx.try_recv().is_ok() {}
            },
        }

        let feeds: Vec<String> = db::load_channels(&conn)
            .unwrap_or_default()
            .into_iter()
            .map(|c| c.url)
            .collect();
        let channels = fetch_all(&feeds).await;
        if !channels.is_empty() {
            let _ = tx.send(channels).await;
        }
    }
}
