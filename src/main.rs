mod db;
mod feed;
mod sync;

use std::collections::HashMap;

use cosmic::app::{Core, Settings, Task};
use cosmic::cosmic_theme::palette::WithAlpha;
use cosmic::iced::{Subscription, Length};
use cosmic::iced::Alignment;
use cosmic::iced::widget::scrollable::Viewport;
use cosmic::widget::{container, scrollable, text, text_input, Column, Row, nav_bar, Space};
use cosmic::widget::button;
use cosmic::widget::menu::{self, ItemHeight, ItemWidth};
use chrono::Local;
use cosmic::{Application, Element};
use feed::Entry;
use tokio::sync::mpsc;

const PAGE_SIZE: usize = 50;

struct App {
    core: Core,
    nav: nav_bar::Model,
    channels: Vec<feed::Channel>,
    selected_entries: Vec<Entry>,
    selected_offset: usize,
    all_loaded: bool,
    conn: rusqlite::Connection,
    syncing: bool,
    force_tx: Option<mpsc::Sender<()>>,
    new_feed_url: String,
    adding_feed: bool,
    renaming_feed: Option<nav_bar::Id>,
    rename_text: String,
}

#[derive(Debug, Clone)]
enum Message {
    Tick(Vec<feed::Channel>),
    Scrolled(Viewport),
    LoadMore,
    RefreshNow,
    SyncReady(mpsc::Sender<()>),
    AddFeedInput(String),
    /// "Feeds → Add feed…" pressed: toggles the add-feed input row.
    AddFeed,
    AddFeedSubmit,
    CancelAddFeed,
    DeleteFeed(nav_bar::Id),
    /// Right-click a feed → "Mark all as read": mark all of its entries read.
    MarkAllRead(nav_bar::Id),
    /// Right-click → "Rename feed": enter rename mode for the nav item.
    RenameFeed(nav_bar::Id),
    /// Rename-row text input changes.
    RenameInput(String),
    /// "Save" button in the rename row.
    RenameSubmit,
    /// "Cancel" button in the rename row.
    CancelRename,
    /// Open an article entry's link in the default browser (marks it read).
    OpenEntry { id: String, link: String },
    /// Click an article row: toggle its read state.
    ToggleEntryRead(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MenuAction {
    AddFeed,
}

impl menu::Action for MenuAction {
    type Message = Message;
    fn message(&self) -> Message {
        match self {
            MenuAction::AddFeed => Message::AddFeed,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FeedMenuAction {
    MarkAllRead(nav_bar::Id),
    Rename(nav_bar::Id),
    Delete(nav_bar::Id),
}

impl menu::Action for FeedMenuAction {
    type Message = cosmic::Action<Message>;
    fn message(&self) -> Self::Message {
        match self {
            FeedMenuAction::MarkAllRead(id) => cosmic::Action::App(Message::MarkAllRead(*id)),
            FeedMenuAction::Rename(id) => cosmic::Action::App(Message::RenameFeed(*id)),
            FeedMenuAction::Delete(id) => cosmic::Action::App(Message::DeleteFeed(*id)),
        }
    }
}

impl Application for App {
    type Executor = cosmic::executor::Default;
    type Message = Message;
    type Flags = ();

    const APP_ID: &'static str = "com.example.cosmic-rss";

    fn core(&self) -> &Core { &self.core }
    fn core_mut(&mut self) -> &mut Core { &mut self.core }

    fn init(core: Core, _flags: ()) -> (Self, Task<Message>) {
        let conn = db::init();
        let channels = db::load_channels(&conn).unwrap_or_default();

        let mut nav = nav_bar::Model::default();
        nav.insert()
            .text("All Articles")
            .icon(cosmic::widget::icon::from_name("feed-subscribe-symbolic"))
            .activate();
        for ch in &channels {
            nav.insert()
                .text(ch.title.clone())
                .icon(cosmic::widget::icon::from_name("application-rss+xml-symbolic"))
                .data(ch.id.clone());
        }

        let selected_entries = db::load_page(&conn, 0, PAGE_SIZE).unwrap_or_default();
        let all_loaded = selected_entries.len() < PAGE_SIZE;

        let mut app = Self {
            core,
            nav,
            channels,
            selected_entries,
            selected_offset: 0,
            all_loaded,
            conn,
            syncing: false,
            force_tx: None,
            new_feed_url: String::new(),
            adding_feed: false,
            renaming_feed: None,
            rename_text: String::new(),
        };
        app.refresh_nav_counts();
        (app, Task::none())
    }

    fn nav_model(&self) -> Option<&nav_bar::Model> {
        Some(&self.nav)
    }

    fn nav_context_menu(&self) -> Option<Vec<menu::Tree<cosmic::Action<Message>>>> {
        Some(menu::nav_context(
            &HashMap::<menu::KeyBind, FeedMenuAction>::new(),
            self.nav
                .iter()
                .map(|id| match self.nav.data::<String>(id) {
                    None => Vec::new(), // "All Articles"
                    Some(_) => vec![
                        menu::Item::Button(
                            "Mark all as read",
                            None,
                            FeedMenuAction::MarkAllRead(id),
                        ),
                        menu::Item::Button(
                            "Rename feed",
                            None,
                            FeedMenuAction::Rename(id),
                        ),
                        menu::Item::Button(
                            "Delete feed",
                            None,
                            FeedMenuAction::Delete(id),
                        ),
                    ],
                })
                .collect(),
        ))
    }

    fn header_start(&self) -> Vec<Element<'_, Message>> {
        let label = if self.syncing { "Syncing…" } else { "Refresh" };
        let btn = button::text(label);
        let btn = if self.syncing { btn } else { btn.on_press(Message::RefreshNow) };
        vec![menu_bar(), btn.into()]
    }

    fn on_nav_select(&mut self, id: nav_bar::Id) -> Task<Message> {
        self.nav.activate(id);
        self.selected_offset = 0;
        let channel_id: Option<String> = self.nav.data(id).cloned();
        self.selected_entries = match &channel_id {
            None => db::load_page(&self.conn, 0, PAGE_SIZE).unwrap_or_default(),
            Some(cid) => db::load_page_for_channel(&self.conn, cid, 0, PAGE_SIZE).unwrap_or_default(),
        };
        self.all_loaded = self.selected_entries.len() < PAGE_SIZE;
        Task::none()
    }

    fn view_window(&self, _id: cosmic::iced::window::Id) -> Element<'_, Message> {
        self.view()
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SyncReady(tx) => {
                self.force_tx = Some(tx);
            }

            Message::Tick(channels) => {
                self.syncing = false;
                for c in &channels {
                    let _ = db::store(&mut self.conn, std::slice::from_ref(c));
                }

                let db_channels = db::load_channels(&self.conn).unwrap_or_default();
                for ch in &db_channels {
                    let exists = self
                        .nav
                        .iter()
                        .any(|id| self.nav.data::<String>(id) == Some(&ch.id));
                    if !exists {
                        self.nav.insert()
                            .text(ch.title.clone())
                            .icon(cosmic::widget::icon::from_name(
                                "application-rss+xml-symbolic",
                            ))
                            .data(ch.id.clone());
                    }
                }
                self.channels = db_channels;
                // Labels (title + unread count) always sync from `self.channels`,
                // so corrected titles (e.g. a newly added or first-run seeded
                // feed whose title was still the URL) propagate here.
                self.refresh_nav_counts();

                let active = self.nav.active();
                let channel_id: Option<String> = self.nav.data(active).cloned();
                let loaded_count = (self.selected_offset + PAGE_SIZE).max(PAGE_SIZE);
                self.selected_entries = match &channel_id {
                    None => db::load_page(&self.conn, 0, loaded_count).unwrap_or_default(),
                    Some(cid) => db::load_page_for_channel(&self.conn, cid, 0, loaded_count).unwrap_or_default(),
                };
                let total = match &channel_id {
                    None => db::count(&self.conn).unwrap_or(0),
                    Some(cid) => db::count_for_channel(&self.conn, cid).unwrap_or(0),
                };
                self.all_loaded = self.selected_entries.len() >= total;
            }

            Message::Scrolled(viewport) => {
                if viewport.relative_offset().y > 0.8 && !self.all_loaded {
                    return self.load_more();
                }
            }

            Message::LoadMore => return self.load_more(),

            Message::RefreshNow => {
                self.syncing = true;
                if let Some(tx) = self.force_tx.clone() {
                    tokio::spawn(async move { let _ = tx.send(()).await; });
                }
            }

            Message::AddFeedInput(url) => {
                self.new_feed_url = url;
            }

            Message::AddFeed => {
                self.adding_feed = !self.adding_feed;
                if self.adding_feed {
                    // Opening the add row closes the rename row.
                    self.renaming_feed = None;
                    self.rename_text.clear();
                } else {
                    self.new_feed_url.clear();
                }
            }

            Message::AddFeedSubmit => {
                let url = self.new_feed_url.trim().to_string();
                if url.is_empty() {
                    return Task::none();
                }
                let _ = db::add_channel(&self.conn, &url);
                let exists = self
                    .nav
                    .iter()
                    .any(|id| self.nav.data::<String>(id) == Some(&url));
                if !exists {
                    self.nav.insert()
                        .text(url.clone())
                        .icon(cosmic::widget::icon::from_name(
                            "application-rss+xml-symbolic",
                        ))
                        .data(url.clone());
                }
                self.new_feed_url.clear();
                self.adding_feed = false;
                // Trigger an immediate sync so the new feed's articles appear now.
                self.syncing = true;
                if let Some(tx) = self.force_tx.clone() {
                    tokio::spawn(async move { let _ = tx.send(()).await; });
                }
            }

            Message::CancelAddFeed => {
                self.adding_feed = false;
                self.new_feed_url.clear();
            }

            Message::DeleteFeed(id) => {
                let was_active = self.nav.active() == id;
                let url: Option<String> = self.nav.data::<String>(id).cloned();
                if let Some(url) = url {
                    let _ = db::remove_channel(&mut self.conn, &url);
                }
                self.nav.remove(id);

                if was_active {
                    // Fall back to "All Articles" (the nav item with no data).
                    let all_id = self
                        .nav
                        .iter()
                        .find(|nav_id| self.nav.data::<String>(*nav_id).is_none());
                    if let Some(all_id) = all_id {
                        self.nav.activate(all_id);
                    }
                    self.selected_offset = 0;
                    self.selected_entries =
                        db::load_page(&self.conn, 0, PAGE_SIZE).unwrap_or_default();
                    self.all_loaded = self.selected_entries.len() < PAGE_SIZE;
                }
                self.refresh_nav_counts();
            }

            Message::MarkAllRead(id) => {
                if let Some(cid) = self.nav.data::<String>(id).cloned() {
                    let _ = db::mark_channel_read(&self.conn, &cid);
                    // If this feed is the active view, dim the loaded rows now
                    // instead of waiting for a reload.
                    if self.nav.data::<String>(self.nav.active()) == Some(&cid) {
                        for e in &mut self.selected_entries {
                            e.read = true;
                        }
                    }
                }
                self.refresh_nav_counts();
            }

            Message::RenameFeed(id) => {
                // Opening the rename row closes the add row.
                self.adding_feed = false;
                self.new_feed_url.clear();
                // Seed from the stored title, not the nav label (which may
                // carry an unread-count suffix).
                self.rename_text = match self.nav.data::<String>(id) {
                    Some(cid) => self
                        .channels
                        .iter()
                        .find(|ch| ch.id == *cid)
                        .map(|ch| ch.title.clone())
                        .unwrap_or_default(),
                    None => String::new(),
                };
                self.renaming_feed = Some(id);
            }

            Message::RenameInput(text) => {
                self.rename_text = text;
            }

            Message::RenameSubmit => {
                if let Some(id) = self.renaming_feed.take() {
                    let title = self.rename_text.trim().to_string();
                    self.rename_text.clear();
                    if !title.is_empty() {
                        if let Some(url) = self.nav.data::<String>(id).cloned() {
                            let _ = db::set_channel_title(&self.conn, &url, &title);
                            if let Some(ch) = self.channels.iter_mut().find(|ch| ch.id == url) {
                                ch.title = title.clone();
                            }
                        }
                    }
                }
                self.refresh_nav_counts();
            }

            Message::CancelRename => {
                self.renaming_feed = None;
                self.rename_text.clear();
            }

            Message::OpenEntry { id, link } => {
                if let Some(e) = self.selected_entries.iter_mut().find(|e| e.id == id) {
                    e.read = true;
                }
                let _ = db::set_entry_read(&self.conn, &id, true);
                open_in_browser(link);
                self.refresh_nav_counts();
            }

            Message::ToggleEntryRead(id) => {
                if let Some(e) = self.selected_entries.iter_mut().find(|e| e.id == id) {
                    e.read = !e.read;
                    let _ = db::set_entry_read(&self.conn, &id, e.read);
                }
                self.refresh_nav_counts();
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let mut col = Column::new().spacing(4);

        if self.adding_feed {
            col = col.push(
                Row::new()
                    .spacing(8)
                    .push(
                        text_input(
                            "https://example.com/feed.xml",
                            &self.new_feed_url,
                        )
                        .on_input(Message::AddFeedInput)
                        .width(Length::Fill),
                    )
                    .push(button::text("Add feed").on_press(Message::AddFeedSubmit))
                    .push(button::text("Cancel").on_press(Message::CancelAddFeed))
                    .height(Length::Shrink),
            );
        }

        if self.renaming_feed.is_some() {
            col = col.push(
                Row::new()
                    .spacing(8)
                    .push(
                        text_input("Feed title", &self.rename_text)
                            .on_input(Message::RenameInput)
                            .width(Length::Fill),
                    )
                    .push(button::text("Save").on_press(Message::RenameSubmit))
                    .push(button::text("Cancel").on_press(Message::CancelRename))
                    .height(Length::Shrink),
            );
        }

        for e in &self.selected_entries {
            let read_class = if e.read {
                MUTED_TEXT
            } else {
                NORMAL_TEXT
            };
            col = col.push(
                button::custom(
                    container(
                        Row::new()
                            .spacing(8)
                            .align_y(Alignment::Center)
                            .push(
                                Column::new()
                                    .push(
                                        text(&e.title)
                                            .size(16)
                                            .class(read_class),
                                    )
                                    .push(
                                        text(
                                            e.published
                                                .with_timezone(&Local)
                                                .format("%d %b %Y  %H:%M")
                                                .to_string(),
                                        )
                                        .size(11)
                                        .class(read_class),
                                    )
                                    .spacing(2)
                                    .width(Length::Fill),
                            )
                            .push(
                                button::icon(cosmic::widget::icon::from_name(
                                    "web-browser-symbolic",
                                ))
                                .class(cosmic::theme::Button::Transparent)
                                .on_press_maybe(
                                    (!e.link.is_empty()).then(|| Message::OpenEntry {
                                        id: e.id.clone(),
                                        link: e.link.clone(),
                                    }),
                                ),
                            ),
                    )
                    .padding([8, 12])
                    .width(Length::Fill),
                )
                // Fill width is required: a Shrink-width button enables width
                // compression, which makes Length::Fill children resolve to
                // content width instead of stretching (misaligning the icon).
                .width(Length::Fill)
                .class(entry_row_class())
                .on_press(Message::ToggleEntryRead(e.id.clone())),
            );
        }

        let active = self.nav.active();
        let channel_id: Option<String> = self.nav.data(active).cloned();
        let unread = db::count_unread(&self.conn, channel_id.as_deref()).unwrap_or(0);

        if !self.all_loaded {
            col = col.push(Space::new().height(8));
            col = col.push(
                Row::new()
                    .spacing(12)
                    .push(button::text("Load more").on_press(Message::LoadMore))
                    .push(
                        text(
                            format!(
                                "{} loaded ({} unread)",
                                self.selected_entries.len(),
                                unread
                            ),
                        )
                        .size(12),
                    ),
            );
        } else if !self.selected_entries.is_empty() {
            col = col.push(Space::new().height(8));
            col = col.push(
                text(
                    format!(
                        "All {} articles loaded ({} unread)",
                        self.selected_entries.len(),
                        unread
                    ),
                )
                .size(12),
            );
        } else {
            col = col.push(text("No articles yet — press Refresh").size(14));
        }

        let scroller = scrollable(col.align_x(Alignment::Start))
            .on_scroll(Message::Scrolled)
            .width(Length::Fill)
            .height(Length::Fill);

        container(scroller)
            .padding(16)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::run(|| futures::stream::unfold(SyncState::Init, |state| async move {
            match state {
                    SyncState::Init => {
                        let (force_tx, force_rx) = mpsc::channel::<()>(1);
                        let (chan_tx, chan_rx) = mpsc::channel(10);
                        tokio::spawn(sync::sync_loop(chan_tx, force_rx));
                        Some((
                            Message::SyncReady(force_tx.clone()),
                            SyncState::Running { chan_rx, force_tx },
                        ))
                    }
                    SyncState::Running { mut chan_rx, force_tx } => {
                        let channels = chan_rx.recv().await?;
                        Some((
                            Message::Tick(channels),
                            SyncState::Running { chan_rx, force_tx },
                        ))
                    }
            }
        }))
    }
}

enum SyncState {
    Init,
    Running {
        chan_rx: mpsc::Receiver<Vec<feed::Channel>>,
        force_tx: mpsc::Sender<()>,
    },
}

impl App {
    /// Updates the nav labels with unread counts: a feed with unread entries
    /// shows `"{title} - {count}"` (the count is right-aligned in the label, so
    /// it's separated from the title — nav items only take a plain text label
    /// and always render it left-aligned), a feed with none shows just its
    /// title, and "All Articles" (the nav item with no data) shows the total.
    /// `text_set`
    /// is only called when the label actually changed. Titles come from
    /// `self.channels`, so title corrections/renames propagate through here.
    fn refresh_nav_counts(&mut self) {
        let ids: Vec<nav_bar::Id> = self.nav.iter().collect();
        for id in ids {
            let label = match self.nav.data::<String>(id) {
                None => {
                    let total = db::count_unread(&self.conn, None).unwrap_or(0);
                    if total == 0 {
                        "All Articles".to_string()
                    } else {
                        format!("All Articles - {total}")
                    }
                }
                Some(cid) => {
                    let count = db::count_unread(&self.conn, Some(cid.as_str())).unwrap_or(0);
                    match self.channels.iter().find(|ch| ch.id == *cid) {
                        Some(ch) if count > 0 => format!("{} - {}", ch.title, count),
                        Some(ch) => ch.title.clone(),
                        // Feed not in `self.channels` yet (e.g. just added via the
                        // Feeds menu): keep its current label until the next Tick.
                        None => continue,
                    }
                }
            };
            if self.nav.text(id) != Some(label.as_str()) {
                self.nav.text_set(id, label);
            }
        }
    }

    fn load_more(&mut self) -> Task<Message> {
        let next_offset = self.selected_offset + PAGE_SIZE;
        let active = self.nav.active();
        let channel_id: Option<String> = self.nav.data(active).cloned();
        let new_page = match &channel_id {
            None => db::load_page(&self.conn, next_offset, PAGE_SIZE).unwrap_or_default(),
            Some(cid) => db::load_page_for_channel(&self.conn, cid, next_offset, PAGE_SIZE).unwrap_or_default(),
        };
        if new_page.is_empty() {
            self.all_loaded = true;
        } else {
            self.all_loaded = new_page.len() < PAGE_SIZE;
            self.selected_offset = next_offset;
            self.selected_entries.extend(new_page);
        }
        Task::none()
    }
}

fn menu_bar<'a>() -> Element<'a, Message> {
    menu::bar(vec![menu::Tree::with_children(
        cosmic::widget::RcElementWrapper::new(Element::from(menu::root("Feeds"))),
        menu::items(
            &HashMap::<menu::KeyBind, MenuAction>::new(),
            vec![menu::Item::Button(
                "Add feed…",
                Some(cosmic::widget::icon::from_name("list-add-symbolic").into()),
                MenuAction::AddFeed,
            )],
        ),
    )])
    .item_height(ItemHeight::Dynamic(40))
    .item_width(ItemWidth::Uniform(240))
    .spacing(4.0)
    .into()
}

// Entry rows sit inside a transparent button, whose content text color is
// transparent itself (it's meant for icon-only buttons) — so the text needs
// an explicit color in both states; an inherited color would render nothing.
fn normal_text_style(theme: &cosmic::theme::Theme) -> cosmic::iced::widget::text::Style {
    cosmic::iced::widget::text::Style {
        color: Some(theme.cosmic().on_bg_color().into()),
        ..Default::default()
    }
}

/// Muted text for read entries: the theme's default on-background text color
/// at reduced alpha, so it adapts to light/dark themes.
fn muted_text_style(theme: &cosmic::theme::Theme) -> cosmic::iced::widget::text::Style {
    let base: cosmic::iced::Color = theme.cosmic().on_bg_color().into();
    cosmic::iced::widget::text::Style {
        color: Some(cosmic::iced::Color { a: base.a * 0.55, ..base }),
        ..Default::default()
    }
}

const NORMAL_TEXT: cosmic::theme::Text = cosmic::theme::Text::Custom(normal_text_style);

const MUTED_TEXT: cosmic::theme::Text = cosmic::theme::Text::Custom(muted_text_style);

/// Article-row button class: transparent at rest, washed with the same
/// `neutral_5` the nav sidebar uses on hover/press (alphas copied from
/// `segmented_button`'s vertical tab-bar).
fn entry_row_class() -> cosmic::theme::Button {
    cosmic::theme::Button::Custom {
        active: Box::new(|_, theme| entry_row_style(theme, None)),
        hovered: Box::new(|_, theme| entry_row_style(theme, Some(0.3))),
        pressed: Box::new(|_, theme| entry_row_style(theme, Some(0.25))),
        disabled: Box::new(|theme| entry_row_style(theme, None)),
    }
}

fn entry_row_style(
    theme: &cosmic::Theme,
    hover_alpha: Option<f32>,
) -> cosmic::widget::button::Style {
    let mut style = cosmic::widget::button::Style::new();
    // Explicit color: inherited text falls back to whatever the renderer
    // default is; the row's text uses explicit classes anyway, but this
    // keeps any inherited content visible (see the Button::Transparent
    // text bug in docs/read-unread-plan.md).
    style.text_color = Some(theme.cosmic().on_bg_color().into());
    style.border_radius = theme.cosmic().corner_radii.radius_m.into();
    if let Some(alpha) = hover_alpha {
        let color: cosmic::iced::Color =
            theme.cosmic().palette.neutral_5.with_alpha(alpha).into();
        style.background = Some(cosmic::iced::Background::Color(color));
    }
    style
}

/// Open `url` in the system's default browser. Fire-and-forget on a dedicated
/// thread so the UI thread never blocks; stdio is nulled so the child process
/// can't hang on inherited handles.
fn open_in_browser(url: String) {
    std::thread::spawn(move || {
        let _ = std::process::Command::new("xdg-open")
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    });
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    cosmic::app::run::<App>(Settings::default(), ())?;
    Ok(())
}
