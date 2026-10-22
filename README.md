# cosmic-rss

A simple RSS reader built with [libcosmic](https://github.com/pop-os/libcosmic) and Rust, designed for the COSMIC desktop environment on Linux.

> **About this fork:** this repository is a fork of [Arunhegde1231/CosmicRSS](https://github.com/Arunhegde1231/CosmicRSS). I am not a developer - all of the modifications in this fork were made by an AI acting on my requests. My goal was simply to have a working RSS reader for my COSMIC desktop that covers my basic requirements - no more.
>
> I also wanted to familiarise myself with LLMs, so instead of using frontier AIs, I am running smaller models locally on my machine.

---

## What it does

- Fetches articles from RSS **and Atom** feeds
- Stores them locally in a SQLite database (`~/.local/share/com.example.cosmic-rss/rss.db`) so they persist between launches
- Add feeds at runtime: **Feeds → Add feed…** in the header - paste a feed URL, no rebuild needed
- Delete feeds: right-click a feed in the sidebar → **Delete feed**
- Rename feeds: right-click → **Rename feed** - the title is preserved across syncs and restarts
- Displays feeds in a collapsible sidebar - click a feed to filter articles, "All Articles" shows everything
- Lazy-loads articles as you scroll, 50 at a time
- Auto-syncs every 10 minutes in the background
- "Refresh" button in the header to trigger an immediate sync

## What it doesn't do (yet)

- No article reading view - titles only, no content or summary pane
- No read/unread tracking
- No search
- No notifications
- RSS has no historical pagination, so older articles only accumulate over time as you leave the app running

---

## Requirements

- Podman or Docker - for running the dev container

## Building

This project ships a [dev container](.devcontainer/devcontainer.json) (Fedora 45) with all system dependencies and the Rust toolchain preinstalled, so there is nothing to set up on the host.

### 1. Open the project

Clone the repository, then open it in an IDE. For example Zed detects the dev container automatically and creates it for you - no extensions or manual setup needed.

### 2. Build inside the dev container

```bash
# Check for errors without building
cargo check

# Release build (optimised, recommended for actual use)
cargo build --release
```

### 3. Run the binary from your host

The built binary is at `target/release/cosmic-rss`.

On first run, `rss.db` will be created in `~/.local/share/com.example.cosmic-rss/`. Delete it to start fresh:

```bash
rm ~/.local/share/com.example.cosmic-rss/rss.db
```

---

## Project structure

```
src/
├── main.rs    # App struct, UI layout, nav sidebar, message handling
├── feed.rs    # Entry and Channel data types
├── sync.rs    # RSS/Atom fetching, background sync loop
└── db.rs      # SQLite setup, read/write helpers
```

## Managing feeds

All feed management happens in the UI - no code changes or rebuilds needed.

- **Add:** **Feeds → Add feed…** in the header, paste the feed's URL (RSS or Atom), press **Add feed**.
- **Rename:** right-click a feed in the sidebar → **Rename feed**, type the new title, press **Save**.
- **Delete:** right-click a feed → **Delete feed** (removes the feed and its articles).

To reset to the default feed set, delete the database (see above).

---

## Dependencies

| Crate | Purpose |
|---|---|
| `libcosmic` | Core UI framework (COSMIC desktop) |
| `tokio` | For Async runtime |
| `reqwest` | HTTP client for fetching feeds |
| `rss` | For parsing RSS feeds |
| `atom_syndication` | For parsing Atom feeds |
| `rusqlite` | SQLite database (bundled) |
| `chrono` | For date/time parsing and formatting |
| `serde` | Serialisation |
| `anyhow` | Error handling |
| `futures` | Streaming utilities |
