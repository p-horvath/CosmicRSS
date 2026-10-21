mod db;
mod feed;
mod sync;

use cosmic::app::{Action, Core, Settings, Task};
use cosmic::iced::{Subscription, Length};
use cosmic::iced::Alignment;
use cosmic::iced::widget::scrollable::Viewport;
use cosmic::widget::{container, scrollable, text, text_input, Column, Row, nav_bar, Space};
use cosmic::widget::button;
use cosmic::widget::menu::{self};
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
}

#[derive(Debug, Clone)]
enum Message {
    Tick(Vec<feed::Channel>),
    Scrolled(Viewport),
    LoadMore,
    RefreshNow,
    SyncReady(mpsc::Sender<()>),
    ToggleSidebar,
    ForceRefreshSent,
    AddFeedInput(String),
    AddFeed,
    CloseFeed(nav_bar::Id),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MenuAction {
    Refresh,
    ToggleSidebar,
}

impl menu::Action for MenuAction {
    type Message = Message;
    fn message(&self) -> Message {
        match self {
            MenuAction::Refresh => Message::RefreshNow,
            MenuAction::ToggleSidebar => Message::ToggleSidebar,
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

        (
            Self {
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
            },
            Task::none(),
        )
    }

    fn nav_bar(&self) -> Option<Element<'_, cosmic::Action<Message>>> {
        if !self.core().nav_bar_active() {
            return None;
        }

        let nav_model = self.nav_model()?;

        let mut nav = cosmic::widget::nav_bar(
            nav_model,
            |id| cosmic::Action::Cosmic(Action::NavBar(id)),
        )
        .on_context(|id| cosmic::Action::Cosmic(Action::NavBarContext(id)))
        .on_close(|id| cosmic::Action::App(Message::CloseFeed(id)))
        .context_menu(self.nav_context_menu());
        #[cfg(wayland_platform)]
        {
            nav = nav
                .window_id_maybe(self.core().main_window_id())
                .on_surface_action(|action| cosmic::Action::Surface(action.flatten()))
        }
        let mut nav = nav
            .into_container()
            .width(Length::Shrink)
            .height(Length::Fill);

        if !self.core().is_condensed() {
            nav = nav.max_width(280);
        }

        Some(Element::from(nav))
    }

    fn nav_model(&self) -> Option<&nav_bar::Model> {
        Some(&self.nav)
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

    fn header_end(&self) -> Vec<Element<'_, Message>> {
        let label = if self.syncing { "Syncing…" } else { "Refresh" };
        let btn = button::text(label);
        let btn = if self.syncing { btn } else { btn.on_press(Message::RefreshNow) };
        vec![btn.into()]
    }

    fn view_window(&self, _id: cosmic::iced::window::Id) -> Element<'_, Message> {
        self.view()
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SyncReady(tx) => {
                self.force_tx = Some(tx);
            }

            Message::ToggleSidebar => {
                self.core.nav_bar_toggle();
            }

            Message::Tick(channels) => {
                self.syncing = false;
                for c in &channels {
                    let _ = db::store(&mut self.conn, std::slice::from_ref(c));
                }

                let db_channels = db::load_channels(&self.conn).unwrap_or_default();
                for ch in &db_channels {
                    let nav_id = self
                        .nav
                        .iter()
                        .find(|id| self.nav.data::<String>(*id) == Some(&ch.id));
                    match nav_id {
                        // Refresh the title if it was corrected (e.g. a newly added or
                        // first-run seeded feed whose title was still the URL).
                        Some(nav_id) => {
                            if let Some(current) = self.nav.text(nav_id) {
                                if current != ch.title.as_str() {
                                    self.nav.text_set(nav_id, ch.title.clone());
                                }
                            }
                        }
                        None => {
                            self.nav.insert()
                                .text(ch.title.clone())
                                .icon(cosmic::widget::icon::from_name(
                                    "application-rss+xml-symbolic",
                                ))
                                .data(ch.id.clone());
                        }
                    }
                }
                self.channels = db_channels;

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

            Message::ForceRefreshSent => {}

            Message::AddFeedInput(url) => {
                self.new_feed_url = url;
            }

            Message::AddFeed => {
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
                        .data(url.clone())
                        .closable();
                }
                self.new_feed_url.clear();
                // Trigger an immediate sync so the new feed's articles appear now.
                self.syncing = true;
                if let Some(tx) = self.force_tx.clone() {
                    tokio::spawn(async move { let _ = tx.send(()).await; });
                }
            }

            Message::CloseFeed(id) => {
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
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let mut col = Column::new().spacing(4);

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
                .push(button::text("Add feed").on_press(Message::AddFeed))
                .height(Length::Shrink),
        );

        for e in &self.selected_entries {
            col = col.push(
                container(
                    Column::new()
                        .push(text(&e.title).size(16))
                        .push(
                            text(e.published.format("%d %b %Y  %H:%M").to_string())
                                .size(11),
                        )
                        .spacing(2),
                )
                .padding([8, 12])
                .width(Length::Fill),
            );
        }

        if !self.all_loaded {
            col = col.push(Space::new().height(8));
            col = col.push(
                Row::new()
                    .spacing(12)
                    .push(button::text("Load more").on_press(Message::LoadMore))
                    .push(text(format!("{} loaded", self.selected_entries.len())).size(12)),
            );
        } else if !self.selected_entries.is_empty() {
            col = col.push(Space::new().height(8));
            col = col.push(
                text(format!("All {} articles loaded", self.selected_entries.len())).size(12),
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    cosmic::app::run::<App>(Settings::default(), ())?;
    Ok(())
}
