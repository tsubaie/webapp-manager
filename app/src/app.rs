// SPDX-License-Identifier: MIT

//! The Web Apps window: a COSMIC settings-style list of web apps, an editor in
//! the context drawer, and a confirmation dialog for deleting.

use crate::fl;
use cosmic::app::context_drawer;
use cosmic::dialog::file_chooser::{self, FileFilter};
use cosmic::iced::alignment::Horizontal;
use cosmic::iced::{Alignment, Length};
use cosmic::prelude::*;
use cosmic::widget::{self, about::About, icon, menu};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use webapp_core::{Browser, Error as StoreError, Store, WebApp, WebAppInput, browsers, host_of};

const FALLBACK_ICON: &str = "web-browser-symbolic";

pub struct AppModel {
    core: cosmic::Core,
    about: About,
    key_binds: HashMap<menu::KeyBind, MenuAction>,
    context_page: ContextPage,

    store: Store,
    apps: Vec<WebApp>,
    browsers: Vec<&'static Browser>,
    browser_names: Vec<String>,
    search: String,

    editor: Editor,
    /// App awaiting delete confirmation.
    confirm_delete: Option<WebApp>,
    /// Last error outside the editor (e.g. a failed delete).
    status: Option<String>,
}

/// State of the add/edit form shown in the context drawer.
#[derive(Default)]
struct Editor {
    /// `None` when creating a new web app.
    id: Option<String>,
    name: String,
    url: String,
    browser: usize,
    current_icon: String,
    chosen_icon: Option<PathBuf>,
    name_error: Option<String>,
    url_error: Option<String>,
    error: Option<String>,
    saving: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Search(String),
    New,
    Edit(String),
    AskDelete(String),
    ConfirmDelete,
    CancelDelete,
    Launch(String),
    EditorName(String),
    EditorUrl(String),
    EditorBrowser(usize),
    ChooseIcon,
    IconChosen(PathBuf),
    IconPickFailed(Arc<file_chooser::Error>),
    UseSiteIcon,
    Save,
    Saved(Result<WebApp, Arc<StoreError>>),
    Deleted(Result<(), Arc<StoreError>>),
    Loaded(Vec<WebApp>),
    /// A background task panicked; shown as a status line.
    TaskFailed(String),
    CloseDrawer,
    ToggleAbout,
    LaunchUrl(String),
    Nothing,
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum ContextPage {
    #[default]
    Editor,
    About,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuAction {
    New,
    About,
}

impl menu::action::MenuAction for MenuAction {
    type Message = Message;
    fn message(&self) -> Message {
        match self {
            MenuAction::New => Message::New,
            MenuAction::About => Message::ToggleAbout,
        }
    }
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "dev.tsubaie.WebApps";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(core: cosmic::Core, _flags: ()) -> (Self, Task<cosmic::Action<Message>>) {
        let about = About::default()
            .name(fl!("app-title"))
            .icon(icon::from_name(FALLBACK_ICON).handle())
            .version(env!("CARGO_PKG_VERSION"))
            .license("MIT");

        let mut browsers = browsers::available();
        if browsers.is_empty() {
            browsers.push(&browsers::KNOWN[0]);
        }
        let browser_names = browsers.iter().map(|b| b.name.to_string()).collect();

        let store = Store::user_default();

        let mut app = AppModel {
            core,
            about,
            key_binds: HashMap::new(),
            context_page: ContextPage::default(),
            store,
            apps: Vec::new(),
            browsers,
            browser_names,
            search: String::new(),
            editor: Editor::default(),
            confirm_delete: None,
            status: None,
        };
        let title = match app.core.main_window_id() {
            Some(id) => app.set_window_title(fl!("app-title"), id),
            None => Task::none(),
        };
        let load = app.load_task();
        (app, Task::batch([title, load]))
    }

    fn header_start(&self) -> Vec<Element<'_, Message>> {
        let menu_bar = menu::bar(vec![
            menu::Tree::with_children(
                menu::root(fl!("file")).apply(Element::from),
                menu::items(
                    &self.key_binds,
                    vec![menu::Item::Button(
                        fl!("new-web-app"),
                        None,
                        MenuAction::New,
                    )],
                ),
            ),
            menu::Tree::with_children(
                menu::root(fl!("view")).apply(Element::from),
                menu::items(
                    &self.key_binds,
                    vec![menu::Item::Button(fl!("about"), None, MenuAction::About)],
                ),
            ),
        ]);
        vec![menu_bar.into()]
    }

    fn header_end(&self) -> Vec<Element<'_, Message>> {
        vec![
            widget::tooltip(
                widget::button::icon(icon::from_name("list-add-symbolic")).on_press(Message::New),
                widget::text(fl!("new-web-app")),
                widget::tooltip::Position::Bottom,
            )
            .into(),
        ]
    }

    fn context_drawer(&self) -> Option<context_drawer::ContextDrawer<'_, Message>> {
        if !self.core.window.show_context {
            return None;
        }
        Some(match self.context_page {
            ContextPage::About => context_drawer::about(
                &self.about,
                |url| Message::LaunchUrl(url.to_string()),
                Message::CloseDrawer,
            ),
            ContextPage::Editor => {
                context_drawer::context_drawer(self.editor_view(), Message::CloseDrawer)
                    .title(if self.editor.id.is_some() {
                        fl!("edit-web-app")
                    } else {
                        fl!("new-web-app")
                    })
                    .footer(self.editor_footer())
            }
        })
    }

    fn dialog(&self) -> Option<Element<'_, Message>> {
        let app = self.confirm_delete.as_ref()?;
        Some(
            widget::dialog()
                .title(fl!("delete-title", name = app.name.clone()))
                .icon(app_icon(app, 64))
                .body(fl!("delete-body"))
                .primary_action(
                    widget::button::destructive(fl!("delete")).on_press(Message::ConfirmDelete),
                )
                .secondary_action(
                    widget::button::standard(fl!("cancel")).on_press(Message::CancelDelete),
                )
                .into(),
        )
    }

    fn view(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let content = if self.apps.is_empty() {
            empty_view()
        } else {
            self.list_view()
        };
        widget::container(widget::container(content).max_width(760))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding([spacing.space_xs, spacing.space_l])
            .align_x(Horizontal::Center)
            .into()
    }

    fn update(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::Search(text) => self.search = text,

            // Opening another form mid-save would orphan the running save's result.
            Message::New | Message::Edit(_) if self.editor.saving => {}

            Message::New => {
                self.editor = Editor::default();
                self.open_drawer(ContextPage::Editor);
            }

            Message::Edit(id) => {
                if let Some(app) = self.apps.iter().find(|a| a.id == id) {
                    // Keep a browser that isn't installed (any more) selectable,
                    // so saving doesn't silently switch the app to another one.
                    let browser = match self.browsers.iter().position(|b| b.id == app.browser) {
                        Some(i) => i,
                        None => browsers::by_id(&app.browser).map_or(0, |b| {
                            self.browsers.push(b);
                            self.browser_names.push(b.name.to_string());
                            self.browsers.len() - 1
                        }),
                    };
                    self.editor = Editor {
                        id: Some(app.id.clone()),
                        name: app.name.clone(),
                        url: app.url.clone(),
                        browser,
                        current_icon: app.icon.clone(),
                        ..Editor::default()
                    };
                    self.open_drawer(ContextPage::Editor);
                }
            }

            Message::AskDelete(id) => {
                self.confirm_delete = self.apps.iter().find(|a| a.id == id).cloned();
            }
            Message::CancelDelete => self.confirm_delete = None,
            Message::ConfirmDelete => {
                if let Some(app) = self.confirm_delete.take() {
                    let store = self.store.clone();
                    return cosmic::task::future(async move {
                        match tokio::task::spawn_blocking(move || store.remove(&app.id)).await {
                            Ok(result) => Message::Deleted(result.map_err(Arc::new)),
                            Err(e) => Message::TaskFailed(e.to_string()),
                        }
                    });
                }
            }
            Message::Deleted(result) => {
                self.status = result.err().map(|e| e.to_string());
                return self.load_task();
            }
            Message::Loaded(apps) => self.apps = apps,
            Message::TaskFailed(why) => {
                self.editor.saving = false;
                self.status = Some(why);
            }

            Message::Launch(id) => {
                let path = self.store.launcher_path(&id);
                // `gio launch` runs the launcher exactly as the app launcher would.
                match std::process::Command::new("gio")
                    .arg("launch")
                    .arg(&path)
                    .spawn()
                {
                    Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
                    Err(e) => self.status = Some(e.to_string()),
                }
            }

            message @ (Message::EditorName(_)
            | Message::EditorUrl(_)
            | Message::EditorBrowser(_)
            | Message::UseSiteIcon
            | Message::ChooseIcon
            | Message::IconChosen(_)
            | Message::IconPickFailed(_)
            | Message::Save
            | Message::Saved(_)) => return self.update_editor(message),

            Message::CloseDrawer => {
                if !self.editor.saving {
                    self.core.window.show_context = false;
                }
            }
            Message::ToggleAbout => {
                if self.core.window.show_context && self.context_page == ContextPage::About {
                    self.core.window.show_context = false;
                } else {
                    self.open_drawer(ContextPage::About);
                }
            }
            Message::LaunchUrl(url) => {
                if let Err(err) = open::that_detached(&url) {
                    eprintln!("failed to open {url:?}: {err}");
                }
            }
            Message::Nothing => {}
        }
        Task::none()
    }
}

impl AppModel {
    /// Validates the form and saves it on a blocking thread (it may download
    /// the site's icon); the result arrives as `Message::Saved`.
    fn start_save(&mut self) -> Task<cosmic::Action<Message>> {
        if self.editor.saving {
            return Task::none();
        }
        self.editor.name_error = self
            .editor
            .name
            .trim()
            .is_empty()
            .then(|| fl!("name-required"));
        self.editor.url_error = self
            .editor
            .url
            .trim()
            .is_empty()
            .then(|| fl!("address-required"));
        if self.editor.name_error.is_some() || self.editor.url_error.is_some() {
            return Task::none();
        }
        self.editor.saving = true;
        self.editor.error = None;
        let store = self.store.clone();
        let id = self.editor.id.clone();
        let input = WebAppInput {
            name: self.editor.name.clone(),
            url: self.editor.url.clone(),
            browser: self.browsers[self.editor.browser.min(self.browsers.len() - 1)]
                .id
                .to_string(),
            icon_file: self
                .editor
                .chosen_icon
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        // Saving can download the site's icon, so it runs off the UI thread.
        cosmic::task::future(async move {
            match tokio::task::spawn_blocking(move || store.save(id.as_deref(), &input)).await {
                Ok(result) => Message::Saved(result.map_err(Arc::new)),
                Err(e) => Message::TaskFailed(e.to_string()),
            }
        })
    }

    /// Handles the add/edit form's messages (split out of `update`).
    fn update_editor(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::EditorName(v) => {
                self.editor.name = v;
                self.editor.name_error = None;
            }
            Message::EditorUrl(v) => {
                self.editor.url = v;
                self.editor.url_error = None;
            }
            Message::EditorBrowser(i) => self.editor.browser = i,
            Message::UseSiteIcon => self.editor.chosen_icon = None,

            Message::ChooseIcon => {
                let (title, images) = (fl!("choose-icon-title"), fl!("images"));
                return cosmic::task::future(async move {
                    let dialog = file_chooser::open::Dialog::new()
                        .title(title)
                        .filter(
                            FileFilter::new(&images)
                                .glob("*.png")
                                .glob("*.svg")
                                .glob("*.jpg")
                                .glob("*.jpeg")
                                .glob("*.ico")
                                .glob("*.webp"),
                        );
                    match dialog.open_file().await {
                        Ok(response) => match response.url().to_file_path() {
                            Ok(path) => Message::IconChosen(path),
                            Err(()) => Message::Nothing,
                        },
                        Err(file_chooser::Error::Cancelled) => Message::Nothing,
                        Err(why) => Message::IconPickFailed(Arc::new(why)),
                    }
                });
            }
            Message::IconChosen(path) => self.editor.chosen_icon = Some(path),
            Message::IconPickFailed(why) => self.editor.error = Some(why.to_string()),

            Message::Save => return self.start_save(),
            Message::Saved(result) => {
                self.editor.saving = false;
                match result {
                    Ok(_) => {
                        self.core.window.show_context = false;
                        return self.load_task();
                    }
                    // Put each error next to the field it concerns.
                    Err(e) => match &*e {
                        StoreError::EmptyName => {
                            self.editor.name_error = Some(fl!("name-required"));
                        }
                        StoreError::InvalidUrl(why) => {
                            self.editor.url_error = Some((*why).to_string());
                        }
                        _ => self.editor.error = Some(e.to_string()),
                    },
                }
            }

            _ => {}
        }
        Task::none()
    }

    /// Reads the launchers on a blocking thread and delivers them as `Loaded`.
    fn load_task(&self) -> Task<cosmic::Action<Message>> {
        let store = self.store.clone();
        cosmic::task::future(async move {
            match tokio::task::spawn_blocking(move || store.list()).await {
                Ok(apps) => Message::Loaded(apps),
                Err(e) => Message::TaskFailed(e.to_string()),
            }
        })
    }

    /// Header, search field and the (filtered) list of web apps.
    fn list_view(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let query = self.search.trim().to_lowercase();
        let visible: Vec<&WebApp> = self
            .apps
            .iter()
            .filter(|a| {
                query.is_empty()
                    || a.name.to_lowercase().contains(&query)
                    || a.url.to_lowercase().contains(&query)
            })
            .collect();

        let list: Element<_> = if visible.is_empty() {
            widget::container(widget::text::body(fl!(
                "no-results",
                query = self.search.trim().to_string()
            )))
            .padding(spacing.space_l)
            .width(Length::Fill)
            .align_x(Horizontal::Center)
            .into()
        } else {
            visible
                .into_iter()
                .fold(widget::settings::section(), |section, app| {
                    section.add(Self::row(app))
                })
                .into()
        };

        let mut column = widget::column::with_capacity(4)
            .spacing(spacing.space_s)
            .push(
                widget::row::with_capacity(3)
                    .push(widget::text::title3(fl!("app-title")))
                    .push(widget::space::horizontal())
                    .push(widget::text::caption(fl!(
                        "apps-count",
                        count = self.apps.len()
                    )))
                    .align_y(Alignment::Center),
            )
            .push(
                widget::search_input(fl!("search"), &self.search)
                    .on_input(Message::Search)
                    .on_clear(Message::Search(String::new())),
            )
            .push(list);
        if let Some(status) = &self.status {
            column = column
                .push(widget::text::caption(status.clone()).class(cosmic::theme::Text::Accent));
        }
        widget::scrollable(column.padding([0, 0, spacing.space_l, 0])).into()
    }

    fn open_drawer(&mut self, page: ContextPage) {
        self.context_page = page;
        self.core.window.show_context = true;
    }

    /// One web app as a settings item: icon, name, host · browser, actions.
    fn row(app: &WebApp) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let actions = widget::row::with_capacity(3)
            .spacing(spacing.space_xxs)
            .align_y(Alignment::Center)
            .push(widget::button::standard(fl!("open")).on_press(Message::Launch(app.id.clone())))
            .push(widget::tooltip(
                widget::button::icon(icon::from_name("edit-symbolic"))
                    .on_press(Message::Edit(app.id.clone())),
                widget::text(fl!("edit")),
                widget::tooltip::Position::Bottom,
            ))
            .push(widget::tooltip(
                widget::button::icon(icon::from_name("edit-delete-symbolic"))
                    .on_press(Message::AskDelete(app.id.clone())),
                widget::text(fl!("delete")),
                widget::tooltip::Position::Bottom,
            ));

        widget::settings::item::builder(app.name.as_str())
            .description(format!(
                "{}  ·  {}",
                host_of(&app.url),
                browser_name(&app.browser)
            ))
            .icon(app_icon(app, 32))
            .control(actions)
            .into()
    }

    fn editor_view(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let e = &self.editor;

        let preview: Element<_> = match (&e.chosen_icon, e.current_icon.as_str()) {
            (Some(path), _) => icon::from_path(path.clone()).icon().size(48).into(),
            (None, current) if !current.is_empty() => icon::from_path(PathBuf::from(current))
                .icon()
                .size(48)
                .into(),
            _ => icon::from_name(FALLBACK_ICON).size(48).icon().into(),
        };
        let icon_note = if e.chosen_icon.is_some() {
            fl!("icon-chosen")
        } else if e.id.is_some() && !e.current_icon.is_empty() {
            fl!("icon-current")
        } else {
            fl!("icon-auto")
        };
        let mut icon_buttons = widget::row::with_capacity(2)
            .spacing(spacing.space_xxs)
            .push(widget::button::standard(fl!("choose-image")).on_press(Message::ChooseIcon));
        if e.chosen_icon.is_some() {
            icon_buttons = icon_buttons
                .push(widget::button::text(fl!("use-site-icon")).on_press(Message::UseSiteIcon));
        }

        let mut name = widget::text_input(fl!("name-placeholder"), &e.name)
            .label(fl!("name"))
            .on_input(Message::EditorName)
            .on_submit(|_| Message::Save);
        if let Some(err) = &e.name_error {
            name = name.error(err.clone());
        }
        let mut url = widget::text_input(fl!("address-placeholder"), &e.url)
            .label(fl!("address"))
            .helper_text(fl!("address-helper"))
            .on_input(Message::EditorUrl)
            .on_submit(|_| Message::Save);
        if let Some(err) = &e.url_error {
            url = url.error(err.clone());
        }

        let mut column = widget::column::with_capacity(6)
            .spacing(spacing.space_m)
            .push(name)
            .push(url)
            .push(
                widget::column::with_capacity(2)
                    .spacing(spacing.space_xxs)
                    .push(widget::text::body(fl!("open-with")))
                    .push(widget::dropdown(
                        &self.browser_names,
                        Some(e.browser),
                        Message::EditorBrowser,
                    )),
            )
            .push(
                widget::column::with_capacity(2)
                    .spacing(spacing.space_xxs)
                    .push(widget::text::body(fl!("icon")))
                    .push(
                        widget::row::with_capacity(2)
                            .spacing(spacing.space_s)
                            .align_y(Alignment::Center)
                            .push(preview)
                            .push(
                                widget::column::with_capacity(2)
                                    .spacing(spacing.space_xxs)
                                    .push(widget::text::caption(icon_note))
                                    .push(icon_buttons),
                            ),
                    ),
            );
        if let Some(err) = &e.error {
            column =
                column.push(widget::text::body(err.clone()).class(cosmic::theme::Text::Accent));
        }
        column.into()
    }

    fn editor_footer(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let e = &self.editor;
        let label = if e.saving {
            fl!("saving")
        } else if e.id.is_some() {
            fl!("save")
        } else {
            fl!("create")
        };
        widget::row::with_capacity(3)
            .spacing(spacing.space_xs)
            .push(widget::space::horizontal())
            .push(
                widget::button::standard(fl!("cancel"))
                    .on_press_maybe((!e.saving).then_some(Message::CloseDrawer)),
            )
            .push(
                widget::button::suggested(label)
                    .on_press_maybe((!e.saving).then_some(Message::Save)),
            )
            .into()
    }
}

/// The site's stored icon, or the generic web icon when there isn't one.
fn app_icon(app: &WebApp, size: u16) -> Element<'static, Message> {
    let path = PathBuf::from(&app.icon);
    if !app.icon.is_empty() && path.exists() {
        icon::from_path(path).icon().size(size).into()
    } else {
        icon::from_name(FALLBACK_ICON).size(size).icon().into()
    }
}

fn browser_name(id: &str) -> String {
    browsers::by_id(id).map_or_else(|| id.to_string(), |b| b.name.to_string())
}

/// Shown before the first web app exists.
fn empty_view<'a>() -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    widget::container(
        widget::column::with_capacity(4)
            .spacing(spacing.space_s)
            .align_x(Alignment::Center)
            .push(icon::from_name(FALLBACK_ICON).size(64).icon())
            .push(widget::text::title3(fl!("empty-title")))
            .push(widget::text::body(fl!("empty-body")).align_x(Horizontal::Center))
            .push(widget::button::suggested(fl!("new-web-app")).on_press(Message::New)),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_x(Horizontal::Center)
    .align_y(cosmic::iced::alignment::Vertical::Center)
    .into()
}
