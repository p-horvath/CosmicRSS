use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    /// Owning channel id (the feed URL). Populated from the `channel`
    /// column when loading entries from the database, and from the feed
    /// URL when the sync loop fetches a feed.
    pub channel: String,
    pub title: String,
    pub link: String,
    pub summary: Option<String>,
    pub published: DateTime<Utc>,
    pub read: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub title: String,
    pub url: String,
    pub entries: Vec<Entry>,
}
