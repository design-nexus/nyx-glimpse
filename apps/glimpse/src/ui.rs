use crate::cb;
use crate::player::Player;
use crate::pw;
use gtk::{gdk, gio, glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use nyx_glimpse::{
    config::{self, Paths},
    clipboard, index, learning, metrics, omarchy, passwords, plugins, providers, query, semantic, utility,
};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
/// The launcher keeps one width; it only grows downwards once there are results.
const WIDTH: i32 = 760;
const COLLAPSED: i32 = 56;
const EXPANDED: i32 = 460;
const LIST_WIDTH: i32 = 300;
static LEARNING_PENDING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
thread_local! {static PANEL:RefCell<Option<gtk::ApplicationWindow>>=const{RefCell::new(None)};}
#[derive(Clone)]
enum Action {
    App(String),
    File(PathBuf, Value),
    Copy(String),
    System(&'static str),
    Command(providers::Target),
    Plugin(String, String, plugins::Action),
    /// A Nyx Passwords row (`pw …`): an item's id and metadata, never a secret.
    Password(pw::Pw),
    /// A Nyx Clipboard row (`cb …`): an entry's id and kind; the service pastes it.
    Clipboard(cb::Cb),
}
#[derive(Clone)]
struct ResultRow {
    title: String,
    /// The second line: a file's folder, a plugin's subtitle, a conversion's source.
    folder: String,
    /// Why it matched, shown as a small tag: "Related", "p. 4", "Line 12".
    reason: String,
    action: Action,
}
impl ResultRow {
    fn category(&self) -> (u8, &'static str) {
        match self.action {
            Action::App(_) => (0, "Applications"),
            Action::File(_, ref hit) if hit["related"] == true => (RELATED, "Related"),
            Action::File(_, _) => (1, "Files"),
            Action::Copy(_) => (2, "Utilities"),
            Action::System(_) | Action::Command(_) => (3, "System"),
            Action::Plugin(_, _, _) => (4, "Plugins"),
            Action::Password(_) => (0, "Passwords"),
            Action::Clipboard(_) => (0, "Clipboard"),
        }
    }
    fn identity(&self) -> String {
        match &self.action {
            Action::App(id) => format!("app:{id}"),
            Action::File(path, _) => format!("file:{}", path.display()),
            Action::Copy(value) => format!("copy:{value}"),
            Action::System(cmd) => format!("system:{cmd}"),
            Action::Command(providers::Target::Omarchy(id)) => omarchy::result_id(id),
            Action::Command(providers::Target::Service(id)) => format!("service:{id}"),
            Action::Command(_) => String::new(),
            Action::Plugin(id, hit, _) => format!("plugin:{id}:{hit}"),
            Action::Password(p) => p.key(),
            Action::Clipboard(c) => c.key(),
        }
    }
}
fn provider_row(candidate: providers::Candidate) -> ResultRow {
    let action = match candidate.target {
        providers::Target::App(id) => Action::App(id),
        providers::Target::File(path) => Action::File(path, Value::Null),
        providers::Target::System(cmd) => Action::Command(providers::Target::System(cmd)),
        providers::Target::Plugin(id, hit, action) => Action::Plugin(id, hit, action),
        target => Action::Command(target),
    };
    ResultRow { title: candidate.title, folder: candidate.subtitle, reason: String::new(), action }
}
fn record_selection(result: &ResultRow) {
    let query = SEARCH.with(|s| s.borrow().as_ref().map(|s| s.text().to_string())).unwrap_or_default();
    let id = result.identity();
    let app = SEARCH.with(|s| {
        s.borrow().as_ref().and_then(|s| s.root()).and_then(|r| r.downcast::<gtk::Window>().ok()).and_then(|w| w.application())
    });
    if learning::eligible_id(&id) && passwords::parse(&query).is_none() && clipboard::parse(&query).is_none() {
        LEARNING_PENDING.fetch_add(1, Ordering::Relaxed);
        let hold = app.map(|app| app.hold());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = learning::record(&Paths::discover(), &id, &query);
            LEARNING_PENDING.fetch_sub(1, Ordering::Relaxed);
            let _ = tx.send(());
        });
        let mut hold = hold;
        glib::timeout_add_local(Duration::from_millis(10), move || {
            if rx.try_recv().is_ok() {
                hold.take();
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }
}
/// A file's folder for display, with the home folder as "~".
fn folder_of(path: &Path) -> String {
    let parent = path.parent().unwrap_or(path);
    match std::env::var_os("HOME").map(PathBuf::from).and_then(|home| parent.strip_prefix(home).ok().map(Path::to_owned)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => parent.display().to_string(),
    }
}
fn clock(seconds: f64) -> String {
    let s = seconds.max(0.).round() as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}
/// Where a content hit landed, in words: a page, a line or a moment.
fn match_place(hit: &Value) -> Option<String> {
    let loc = hit["loc"].as_f64();
    if let Some(page) = hit["info"]["page"].as_u64().or_else(|| loc.filter(|_| hit["kind"] == "pdf").map(|n| n as u64)) {
        return Some(format!("Page {page}"));
    }
    if let Some(line) = hit["info"]["line"].as_u64().or_else(|| loc.filter(|_| hit["segment"] == "lines").map(|n| n as u64)) {
        return Some(format!("Line {line}"));
    }
    hit["info"]["seconds"]
        .as_f64()
        .or_else(|| loc.filter(|_| hit["kind"] == "audio" || hit["kind"] == "video"))
        .map(|s| format!("At {}", clock(s)))
}
/// The tag beside a file found by its contents rather than its name.
fn match_reason(hit: &Value) -> String {
    if hit.is_null() {
        return String::new();
    }
    if hit["related"] == true {
        return "Related".into();
    }
    match match_place(hit) {
        Some(place) => place.replace("Page ", "p. "),
        None => "Content".into(),
    }
}
fn focus_search(window: &gtk::ApplicationWindow, entry: &gtk::SearchEntry) {
    gtk::prelude::GtkWindowExt::set_focus(window, Some(entry));
    entry.grab_focus();
    let entry = entry.clone();
    glib::idle_add_local_once(move || {
        entry.grab_focus();
    });
}
struct Reply {
    generation: u64,
    rows: Vec<ResultRow>,
    append: bool,
    status: String,
}
pub fn theme() {
    nyx_theme::use_design_2();
    nyx_theme::install(nyx_theme::Style {
        window_class: "glimpse-window",
        base_skip: &[],
        css: include_str!("style.css"),
        no_glow_css: "",
        extra_colors: None,
        prefs: || {
            let c = Paths::discover().load().unwrap_or_default();
            nyx_theme::ThemePrefs {
                mode: if c.mode == "theme" { nyx_theme::ThemeMode::Theme } else { nyx_theme::ThemeMode::Omarchy },
                theme: c.theme,
                glow: false,
                reduce_motion: true,
            }
        },
        set_theme: Some(|mode, theme| {
            let p = Paths::discover();
            if let Ok(mut c) = p.load() {
                c.mode = if mode == nyx_theme::ThemeMode::Theme { "theme" } else { "omarchy" }.into();
                c.theme = theme;
                let _ = p.save(&c);
            }
        }),
    });
}
/// The output named by `GLIMPSE_MONITOR`, for layout checks on a virtual monitor.
fn test_monitor() -> Option<gtk::gdk::Monitor> {
    let name = std::env::var("GLIMPSE_MONITOR").ok()?;
    let monitors = gtk::gdk::Display::default()?.monitors();
    (0..monitors.n_items())
        .filter_map(|i| monitors.item(i).and_downcast::<gtk::gdk::Monitor>())
        .find(|m| m.connector().is_some_and(|c| c == name))
}
fn status_error(label: &gtk::Label, e: impl std::fmt::Display) {
    label.set_text(&e.to_string());
}
thread_local! {static SEARCH:RefCell<Option<gtk::SearchEntry>>=const{RefCell::new(None)};}

/// Runs the search in the box again, as typing would (the vault just opened under a `pw`
/// query).
pub fn rerun_search() {
    let entry = SEARCH.with(|s| s.borrow().clone());
    if let Some(entry) = entry {
        entry.emit_by_name::<()>("search-changed", &[]);
    }
}
pub fn present(app: &gtk::Application, toggle: bool, query: Option<&str>) {
    let _ = nyx_glimpse::ai_setup::resume(&Paths::discover());
    if let Some(win) = PANEL.with(|p| p.borrow().clone()) {
        if toggle && win.is_visible() && win.is_mapped() {
            win.set_visible(false);
            return;
        }
        // Recreate the native window instead of reusing a retired Wayland surface.
        win.destroy();
        PANEL.with(|p| *p.borrow_mut() = None);
        SEARCH.with(|s| *s.borrow_mut() = None);
    }
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Glimpse")
        .default_width(WIDTH)
        .default_height(COLLAPSED)
        .decorated(false)
        .resizable(false)
        .build();
    window.add_css_class("glimpse-window");
    window.add_css_class("glimpse-launcher");
    // A transparent overlay over the whole monitor: the pointer never leaves Glimpse, so
    // focus-follows-mouse can't take the keyboard, and a click beside the panel dismisses it.
    if gtk4_layer_shell::is_supported() {
        window.init_layer_shell();
        // A non-resizable window would size the surface to its content instead of the monitor.
        window.set_resizable(true);
        window.set_layer(Layer::Overlay);
        for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            window.set_anchor(edge, true);
        }
        window.set_namespace(Some("glimpse"));
        match test_monitor() {
            // A test copy (GLIMPSE_MONITOR=<connector>) stays on that output and never
            // takes the keyboard from the real session.
            Some(monitor) => {
                window.set_monitor(Some(&monitor));
                window.set_keyboard_mode(KeyboardMode::None);
            }
            None => window.set_keyboard_mode(KeyboardMode::Exclusive),
        }
    }
    let main = gtk::Box::new(gtk::Orientation::Vertical, 0);
    main.add_css_class("glimpse-frame");
    main.set_size_request(WIDTH, COLLAPSED);
    let search_bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    search_bar.add_css_class("search-bar");
    let entry =
        gtk::SearchEntry::builder().placeholder_text("Search apps, files and content").search_delay(0).hexpand(true).build();
    entry.add_css_class("glimpse-search");
    entry.set_search_delay(0);
    search_bar.append(&entry);
    main.append(&search_bar);
    // Results on the left, the selected one previewed on the right; or an empty state.
    let content = gtk::Stack::new();
    content.set_vexpand(true);
    let results = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.add_css_class("results");
    let scroll = gtk::ScrolledWindow::builder()
        .vscrollbar_policy(gtk::PolicyType::External)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&list)
        .width_request(LIST_WIDTH)
        .build();
    results.append(&scroll);
    let pane = PreviewPane::new(&entry);
    results.append(&pane.root);
    content.add_named(&results, Some("results"));
    let empty = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.add_named(&empty, Some("empty"));
    content.set_visible(false);
    main.append(&content);
    let playback: Rc<RefCell<Option<Player>>> = Rc::new(RefCell::new(None));
    let media_hide = pane.media.clone();
    let playback_hide = playback.clone();
    window.connect_visible_notify(move |w| {
        if !w.is_visible() {
            stop_playback(&media_hide, &playback_hide);
            let weak = w.downgrade();
            glib::idle_add_local_once(move || {
                if let Some(w) = weak.upgrade()
                    && !w.is_visible()
                    && let Some(app) = w.application()
                    && !app.windows().iter().any(|other| other.is_visible())
                {
                    // A dismissed launcher should release its native surface and D-Bus name.
                    if LEARNING_PENDING.load(Ordering::Relaxed) == 0 {
                        app.quit();
                    } else {
                        glib::timeout_add_local(Duration::from_millis(10), move || {
                            if app.windows().iter().any(|w| w.is_visible()) {
                                return glib::ControlFlow::Break;
                            }
                            if LEARNING_PENDING.load(Ordering::Relaxed) == 0 {
                                app.quit();
                                glib::ControlFlow::Break
                            } else {
                                glib::ControlFlow::Continue
                            }
                        });
                    }
                }
            });
        }
    });
    // Status bar: key hints for the selected result (or a message), the count, then actions.
    let status_bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    status_bar.add_css_class("status-bar");
    let hints = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    hints.add_css_class("status-hints");
    status_bar.append(&hints);
    let footer = gtk::Label::new(None);
    footer.add_css_class("status-message");
    footer.set_ellipsize(gtk::pango::EllipsizeMode::End);
    footer.set_xalign(0.);
    footer.set_visible(false);
    let hints_toggle = hints.clone();
    footer.connect_label_notify(move |label| {
        // A message replaces the hints until it is cleared.
        let message = !label.label().is_empty();
        label.set_visible(message);
        hints_toggle.set_visible(!message);
    });
    status_bar.append(&footer);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    status_bar.append(&spacer);
    let count = gtk::Label::new(None);
    count.add_css_class("status-count");
    status_bar.append(&count);
    let app_clone = app.clone();
    status_bar
        .append(&nyx_ui::chrome::status_action("folder-new-symbolic", "Add a folder to search", move || add_folder(&app_clone)));
    let app_clone = app.clone();
    status_bar
        .append(&nyx_ui::chrome::status_action("application-x-addon-symbolic", "Plugins", move || plugin_manager(&app_clone)));
    status_bar.append(&nyx_ui::chrome::gear(|| {
        dismiss();
        let _ = Command::new("nyx-settings").args(["--section", "glimpse"]).spawn();
    }));
    status_bar.set_visible(false);
    main.append(&status_bar);
    // A fixed container gives the panel its requested size rather than its content's natural one.
    let backdrop = gtk::Fixed::new();
    backdrop.put(&main, 0., 0.);
    window.set_child(Some(&backdrop));
    let overlay_size = Rc::new(Cell::new((0, 0)));
    let panel_layout = main.clone();
    let size_layout = overlay_size.clone();
    window.connect_realize(move |w| {
        if let Some(surface) = w.surface() {
            let panel = panel_layout.clone();
            let size = size_layout.clone();
            // Moving the panel lays the surface out again; only a new size needs a new spot.
            surface.connect_layout(move |_, width, height| {
                if size.replace((width, height)) != (width, height) {
                    place_panel(&panel, (width, height));
                }
            });
        }
    });
    let armed = Rc::new(RefCell::new(None::<String>));
    let rows: Rc<RefCell<Vec<ResultRow>>> = Rc::new(RefCell::new(Vec::new()));
    let header_rows = rows.clone();
    list.set_header_func(move |row, before| {
        let results = header_rows.borrow();
        let Some(result) = results.get(row.index() as usize) else {
            return;
        };
        let same = before.and_then(|r| results.get(r.index() as usize)).is_some_and(|r| r.category().0 == result.category().0);
        if same {
            row.set_header(None::<&gtk::Widget>);
        } else {
            // GTK CSS can't uppercase, so the heading is written that way.
            let label = gtk::Label::new(Some(&result.category().1.to_uppercase()));
            label.set_xalign(0.);
            label.add_css_class("category-heading");
            if before.is_none() {
                label.add_css_class("first");
            }
            row.set_header(Some(&label));
        }
    });
    let search_started = Rc::new(Cell::new(Instant::now()));
    let search_render = search_started.clone();
    let search_input = search_started.clone();
    window.connect_map(move |w| trace_frame(w, "process-to-first-frame", metrics::started()));
    let generation = Arc::new(AtomicU64::new(0));
    let (tx, rx) = mpsc::channel::<Reply>();
    let (jobs, job_rx) = mpsc::channel::<(u64, String)>();
    let apps: Vec<_> = gio::AppInfo::all()
        .into_iter()
        .filter(|a| a.should_show())
        .filter_map(|a| {
            a.id().map(|id| {
                // Kept on this thread for the rows' icons and the app preview.
                APPS.with(|apps| apps.borrow_mut().insert(id.to_string(), a.clone()));
                let keywords = app_keywords(&id);
                (
                    id.to_string(),
                    a.display_name().to_string(),
                    format!("{} {} {keywords}", a.name(), a.description().unwrap_or_default()).to_lowercase(),
                )
            })
        })
        .collect();
    let (enrich_jobs, enrich_rx) = mpsc::channel::<(u64, String)>();
    let gen_enrich = generation.clone();
    let tx_enrich = tx.clone();
    std::thread::spawn(move || {
        let p = Paths::discover();
        let mut extensions = providers::Extensions;
        let mut extra = providers::Registry::default();
        let _ = extra.register(omarchy::Omarchy::default());
        let _ = extra.register(providers::DesktopServices);
        let mut session: Option<semantic::Session> = None;
        let mut session_device = String::new();
        loop {
            let (mut request_generation, mut query) = match enrich_rx.recv_timeout(Duration::from_secs(1)) {
                Ok(job) => job,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(session) = session.as_mut() {
                        session.release_idle();
                    }
                    continue;
                }
                Err(_) => break,
            };
            std::thread::sleep(Duration::from_millis(180));
            while let Ok((g, q)) = enrich_rx.try_recv() {
                request_generation = g;
                query = q;
            }
            let q = query.trim();
            let cancelled = || gen_enrich.load(Ordering::Relaxed) != request_generation;
            if q.is_empty() || cancelled() || utility::intent(q) != utility::Intent::Search {
                continue;
            }
            let Ok(c) = p.load() else {
                continue;
            };
            if session_device != c.device || !c.semantic {
                session = None;
                session_device = c.device.clone();
            }
            let profile = learning::Profile::load(&p, &c).unwrap_or_default();
            let (candidates, errors) =
                extra.search(&providers::SearchContext { paths: &p, config: &c, query: q, cancelled: &cancelled }, &profile);
            let mut enrich: Vec<_> = candidates.into_iter().take(30).map(|(_, candidate)| provider_row(candidate)).collect();
            if !enrich.is_empty() {
                let _ = tx_enrich.send(Reply {
                    generation: request_generation,
                    rows: std::mem::take(&mut enrich),
                    append: true,
                    status: String::new(),
                });
            }
            if !errors.is_empty() && metrics::enabled() {
                eprintln!("Glimpse provider diagnostics: {}", errors.join("; "));
            }

            if let Ok(hits) = providers::Provider::discover(
                &mut extensions,
                &providers::SearchContext { paths: &p, config: &c, query: q, cancelled: &cancelled },
            ) {
                enrich.extend(hits.into_iter().map(provider_row));
            }
            let mut semantic_status = String::new();
            if c.semantic && query::semantic_allowed(q) && !cancelled() {
                let result = (|| {
                    if session.is_none() {
                        session = Some(semantic::Session::start(&p)?);
                    }
                    session.as_mut().unwrap().search(&p, q, cancelled)
                })();
                match result {
                    Ok(hits) => {
                        for hit in hits {
                            if let Some(path) = hit["path"].as_str() {
                                let path = PathBuf::from(path);
                                enrich.push(ResultRow {
                                    title: path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                                    folder: folder_of(&path),
                                    reason: match_reason(&hit),
                                    action: Action::File(path, hit),
                                });
                            }
                        }
                    }
                    Err(e) => {
                        semantic_status = e.to_string();
                        session = None;
                    }
                }
            }
            let _ = tx_enrich.send(Reply { generation: request_generation, rows: enrich, append: true, status: semantic_status });
        }
    });
    // `pw` queries are answered on this thread (the daemon's client runs on the GTK loop).
    let tx_pw = tx.clone();
    let gen_worker = generation.clone();
    std::thread::spawn(move || {
        let mut registry = providers::Registry::default();
        let _ = registry.register(providers::Applications(apps));
        let mut files = providers::Files::default();
        // One index connection for the life of the window, not one per keystroke.
        let mut db: Option<rusqlite::Connection> = None;
        while let Ok((mut request_generation, mut query)) = job_rx.recv() {
            std::thread::sleep(Duration::from_millis(10));
            while let Ok((g, q)) = job_rx.try_recv() {
                request_generation = g;
                query = q;
            }
            if gen_worker.load(Ordering::Relaxed) != request_generation {
                continue;
            }
            let p = Paths::discover();
            let Ok(c) = p.load() else {
                continue;
            };
            let q = query.trim();
            if q.is_empty() {
                continue;
            }
            let mut found = Vec::new();
            let mut error = String::new();
            if utility::intent(q) != utility::Intent::Search {
                if utility::currency_query(q).is_some() {
                    let _ = tx.send(Reply {
                        generation: request_generation,
                        rows: vec![],
                        append: false,
                        status: "Getting daily currency rates…".into(),
                    });
                }
                match utility::answers_local(&p, q, &c, || gen_worker.load(Ordering::Relaxed) != request_generation) {
                    Ok(answers) => {
                        found.extend(answers.into_iter().map(|answer| ResultRow {
                            title: answer.title,
                            // Key hints live in the status bar, not in every row.
                            folder: answer.detail.replace(" · Enter to copy", "").replace("Enter to copy answer", ""),
                            reason: String::new(),
                            action: Action::Copy(answer.copy),
                        }));
                    }
                    Err(e) => error = e.to_string(),
                }
                let _ = tx.send(Reply { generation: request_generation, rows: found, append: false, status: error });
                continue;
            }
            let profile = learning::Profile::load(&p, &c).unwrap_or_default();
            let cancelled = || gen_worker.load(Ordering::Relaxed) != request_generation;
            let (candidates, _) =
                registry.search(&providers::SearchContext { paths: &p, config: &c, query: q, cancelled: &cancelled }, &profile);
            let mut app_count = 0;
            for (_, candidate) in candidates {
                if matches!(candidate.target, providers::Target::App(_)) {
                    app_count += 1;
                    if app_count > 12 {
                        continue;
                    }
                }
                found.push(provider_row(candidate));
            }
            // Applications paint before a large filename catalogue or content lookup finishes.
            let _ = tx.send(Reply { generation: request_generation, rows: found.clone(), append: false, status: String::new() });
            if let Ok(hits) = providers::Provider::discover(
                &mut files,
                &providers::SearchContext { paths: &p, config: &c, query: q, cancelled: &cancelled },
            ) {
                found.extend(hits.into_iter().map(provider_row));
            }
            if db.is_none() {
                db = index::open(&p).ok();
            }
            if let Some(conn) = &db
                && let Ok(hits) = index::content_search_in(conn, &p, &c, q)
            {
                for (file, info) in hits {
                    found.push(ResultRow {
                        title: file.name,
                        folder: folder_of(&file.path),
                        reason: match_reason(&info),
                        action: Action::File(file.path, info),
                    });
                }
            }
            // Matched like app names, so a stray letter doesn't offer Reboot or Shut down.
            for (name, cmd) in [("Lock screen", "lock"), ("Log out", "logout"), ("Reboot", "reboot"), ("Shut down", "shutdown")] {
                if query::name_rank(name, q).is_some() {
                    found.push(ResultRow {
                        title: name.into(),
                        folder: String::new(),
                        reason: String::new(),
                        action: Action::System(cmd),
                    });
                }
            }
            let _ = tx.send(Reply { generation: request_generation, rows: found, append: false, status: error });
            let _ = enrich_jobs.send((request_generation, query));
        }
    });
    let invalidated = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let signal = invalidated.clone();
    std::thread::spawn(move || {
        use notify::Watcher;
        let p = Paths::discover();
        let _ = p.ensure();
        let (tx, rx) = mpsc::channel();
        let Ok(mut watcher) = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if let Ok(event) = event
                && matches!(
                    event.kind,
                    notify::EventKind::Create(_) | notify::EventKind::Modify(_) | notify::EventKind::Remove(_)
                )
                && event
                    .paths
                    .iter()
                    .any(|path| path.file_name().is_some_and(|name| ["settings.toml", "plugins.json"].iter().any(|s| name == *s)))
            {
                signal.store(true, Ordering::Relaxed);
            }
            let _ = tx.send(());
        }) else {
            return;
        };
        let _ = watcher.watch(&p.config, notify::RecursiveMode::NonRecursive);
        let _ = watcher.watch(&p.state, notify::RecursiveMode::NonRecursive);
        while rx.recv().is_ok() {}
    });
    let browsing = Rc::new(std::cell::Cell::new(false));
    let browsing_poll = browsing.clone();
    let rebuilding = Rc::new(std::cell::Cell::new(false));
    let rebuild_poll = rebuilding.clone();
    let scroll_poll = scroll.clone();
    let refresh_jobs = jobs.clone();
    let refresh_entry = entry.clone();
    let rows_poll = rows.clone();
    let list_poll = list.clone();
    let footer_poll = footer.clone();
    let count_poll = count.clone();
    let content_poll = content.clone();
    let empty_poll = empty.clone();
    let gen_poll = generation.clone();
    let pane_poll = pane.root.clone();
    let window_weak = window.downgrade();
    glib::timeout_add_local(Duration::from_millis(25), move || {
        if window_weak.upgrade().is_none() {
            return glib::ControlFlow::Break;
        }
        if invalidated.swap(false, Ordering::Relaxed) {
            pane_poll.set_visible(Paths::discover().load().is_ok_and(|c| c.preview));
            // A `pw` query never goes to the other providers.
            if passwords::parse(&refresh_entry.text()).is_none() && clipboard::parse(&refresh_entry.text()).is_none() {
                let g = gen_poll.fetch_add(1, Ordering::Relaxed) + 1;
                let _ = refresh_jobs.send((g, refresh_entry.text().to_string()));
            }
        }
        while let Ok(reply) = rx.try_recv() {
            if reply.generation != gen_poll.load(Ordering::Relaxed) {
                continue;
            }
            // Late enrichment must never move the list under keyboard or mouse navigation:
            // new results are added at the end, leaving the selection and scroll position alone.
            if browsing_poll.get() {
                if !reply.status.is_empty() {
                    footer_poll.set_text(&reply.status);
                }
                for result in reply.rows {
                    if rows_poll.borrow().iter().any(|r| r.identity() == result.identity()) {
                        continue;
                    }
                    rows_poll.borrow_mut().push(result.clone());
                    list_poll.append(&result_row(&result));
                }
                count_poll.set_text(&count_text(rows_poll.borrow().len()));
                continue;
            }
            if !reply.status.is_empty() {
                footer_poll.set_text(&reply.status);
            }
            let mut merged = if reply.append { rows_poll.borrow().clone() } else { Vec::new() };
            for result in reply.rows {
                if let Some(existing) = merged.iter_mut().find(|r| r.identity() == result.identity()) {
                    if matches!(&result.action, Action::File(_, hit) if !hit.is_null()) {
                        let mut result = result;
                        // A file already listed by name keeps its place rather than moving to Related.
                        if existing.category().0 != RELATED
                            && let Action::File(_, ref mut hit) = result.action
                        {
                            hit["related"] = false.into();
                        }
                        result.reason = match &result.action {
                            Action::File(_, hit) => match_reason(hit),
                            _ => result.reason,
                        };
                        *existing = result;
                    }
                } else {
                    merged.push(result);
                }
            }
            merged.sort_by_key(|r| r.category().0);
            if merged.is_empty() && reply.status.is_empty() {
                show_empty(&empty_poll, refresh_entry.text().trim());
                content_poll.set_visible_child_name("empty");
            } else {
                content_poll.set_visible_child_name("results");
            }
            count_poll.set_text(&count_text(merged.len()));
            if let Some(w) = window_weak.upgrade() {
                trace_frame(&w, "search-to-frame", search_render.get());
            }
            let unchanged = {
                let old = rows_poll.borrow();
                old.len() == merged.len()
                    && old.iter().zip(&merged).all(|(a, b)| {
                        a.identity() == b.identity() && a.title == b.title && a.folder == b.folder && a.reason == b.reason
                    })
            };
            if unchanged {
                *rows_poll.borrow_mut() = merged;
                continue;
            }
            rebuild_poll.set(true);
            while let Some(child) = list_poll.first_child() {
                list_poll.remove(&child);
            }
            *rows_poll.borrow_mut() = merged.clone();
            for result in &merged {
                list_poll.append(&result_row(result));
            }
            // Nobody has navigated yet (that path returns above), so the top result is the
            // one selected; otherwise Enter could act on a row sorted out of view.
            list_poll.select_row(list_poll.row_at_index(0).as_ref());
            rebuild_poll.set(false);
            // Notify once, after the list is complete.
            list_poll.emit_by_name::<()>("row-selected", &[&list_poll.selected_row()]);
            let adjustment = scroll_poll.vadjustment();
            glib::idle_add_local_once(move || adjustment.set_value(0.));
        }
        glib::ControlFlow::Continue
    });
    let jobs_entry = jobs.clone();
    let gen_entry = generation.clone();
    let footer_entry = footer.clone();
    let count_entry = count.clone();
    let hints_entry = hints.clone();
    let armed_entry = armed.clone();
    let content_entry = content.clone();
    let status_entry = status_bar.clone();
    let pane_entry = pane.clone();
    let playback_entry = playback.clone();
    let panel_entry = main.clone();
    let size_entry = overlay_size.clone();
    let list_entry = list.clone();
    let rows_entry = rows.clone();
    let browsing_entry = browsing.clone();
    entry.connect_changed(move |_| search_input.set(Instant::now()));
    entry.connect_search_changed(move |e| {
        browsing_entry.set(false);
        let expanded = !e.text().trim().is_empty();
        let is_math = utility::intent(&e.text()) == utility::Intent::Calculation
            && passwords::parse(&e.text()).is_none()
            && clipboard::parse(&e.text()).is_none();
        pane_entry.show_calculator(is_math);
        stop_playback(&pane_entry.media, &playback_entry);
        if is_math {
            let c = Paths::discover().load().unwrap_or_default();
            pane_entry.display.set_text(
                &utility::answers(&e.text(), &c)
                    .ok()
                    .and_then(|a| a.into_iter().next())
                    .map(|a| a.title)
                    .unwrap_or_else(|| utility::pretty_math(&e.text())),
            );
        }
        content_entry.set_visible(expanded);
        content_entry.set_visible_child_name("results");
        // The bar's hairline only shows once there is something under it.
        if let Some(frame) = content_entry.parent() {
            if expanded { frame.add_css_class("expanded") } else { frame.remove_css_class("expanded") }
        }
        status_entry.set_visible(expanded);
        pane_entry.root.set_visible(Paths::discover().load().is_ok_and(|c| c.preview));
        panel_entry.set_size_request(WIDTH, if expanded { EXPANDED } else { COLLAPSED });
        place_panel(&panel_entry, size_entry.get());
        while let Some(child) = list_entry.first_child() {
            list_entry.remove(&child);
        }
        rows_entry.borrow_mut().clear();
        *armed_entry.borrow_mut() = None;
        footer_entry.set_text("");
        count_entry.set_text("");
        set_hints(&hints_entry, None);
        let g = gen_entry.fetch_add(1, Ordering::Relaxed) + 1;
        // `pw …` belongs to the vault alone: no apps, files, plugins or semantic search.
        if let Some(q) = passwords::parse(&e.text()) {
            let tx = tx_pw.clone();
            glib::spawn_future_local(async move {
                let (found, status) = pw::lookup(&q).await;
                let rows = found
                    .into_iter()
                    .map(|(title, folder, p)| ResultRow { title, folder, reason: String::new(), action: Action::Password(p) })
                    .collect();
                let _ = tx.send(Reply { generation: g, rows, append: false, status });
            });
            return;
        }
        // `cb …` belongs to the clipboard history alone, like `pw`.
        if let Some(q) = clipboard::parse(&e.text()) {
            let tx = tx_pw.clone();
            glib::spawn_future_local(async move {
                let (found, status) = cb::lookup(&q).await;
                let rows = found
                    .into_iter()
                    .map(|(title, folder, c)| ResultRow { title, folder, reason: String::new(), action: Action::Clipboard(c) })
                    .collect();
                let _ = tx.send(Reply { generation: g, rows, append: false, status });
            });
            return;
        }
        let _ = jobs_entry.send((g, e.text().to_string()));
    });
    let preview_gen = Arc::new(AtomicU64::new(0));
    let (preview_tx, preview_rx) = mpsc::channel::<(u64, Preview)>();
    let rows_preview = rows.clone();
    let gen_preview = preview_gen.clone();
    let query_entry = entry.clone();
    let armed_preview = armed.clone();
    let footer_preview = footer.clone();
    let hints_preview = hints.clone();
    let selected_preview = Rc::new(RefCell::new(None::<String>));
    let pane_select = pane.clone();
    let playback_select = playback.clone();
    let window_select = window.clone();
    let entry_select = entry.clone();
    list.connect_row_selected(move |_, row| {
        if rebuilding.get() {
            return;
        }
        // Moving away from a system action disarms it.
        if armed_preview.borrow_mut().take().is_some() {
            footer_preview.set_text("");
        }
        if row.is_some() {
            let window = window_select.clone();
            let entry = entry_select.clone();
            glib::idle_add_local_once(move || {
                if window.is_visible() {
                    focus_search(&window, &entry);
                }
            });
        }
        let pane = &pane_select;
        let Some(row) = row else {
            *selected_preview.borrow_mut() = None;
            gen_preview.fetch_add(1, Ordering::Relaxed);
            stop_playback(&pane.media, &playback_select);
            pane.card(&gio::ThemedIcon::new("system-search-symbolic").upcast(), "", "", true);
            pane.clear_details();
            set_hints(&hints_preview, None);
            return;
        };
        let Some(result) = rows_preview.borrow().get(row.index() as usize).cloned() else {
            return;
        };
        let identity = result.identity();
        if selected_preview.borrow().as_ref() == Some(&identity) {
            return;
        }
        *selected_preview.borrow_mut() = Some(identity);
        set_hints(&hints_preview, Some(&result.action));
        let g = gen_preview.fetch_add(1, Ordering::Relaxed) + 1;
        stop_playback(&pane.media, &playback_select);
        pane.clear_details();
        let icon = action_icon(&result.action);
        match &result.action {
            Action::App(id) => {
                let app = APPS.with(|apps| apps.borrow().get(id).cloned());
                let description = app.as_ref().and_then(|a| a.description()).map(|d| d.to_string()).unwrap_or_default();
                // Drop the desktop-entry field codes (%U, %f, …) from the command shown.
                let command = app
                    .and_then(|a| a.commandline())
                    .map(|c| c.to_string_lossy().split_whitespace().filter(|w| !w.starts_with('%')).collect::<Vec<_>>().join(" "))
                    .unwrap_or_default();
                pane.app_card(&icon, &result.title, &description, &command, &app_facts(id));
                return;
            }
            Action::Copy(value) => {
                pane.card(&icon, &result.title, &result.folder, false);
                pane.add_detail("Copies", value);
                return;
            }
            Action::Command(_) => {
                pane.card(&icon, &result.title, &result.folder, false);
                return;
            }
            Action::System(cmd) => {
                pane.card(&icon, "", system_description(cmd), false);
                return;
            }
            Action::Password(p) => {
                pane.card(&icon, &result.title, &result.folder, false);
                for (key, value) in p.details() {
                    pane.add_detail(&key, &value);
                }
                return;
            }
            Action::Clipboard(c) => {
                pane.card(&icon, &result.title, &result.folder, false);
                for (key, value) in c.details() {
                    pane.add_detail(&key, &value);
                }
                return;
            }
            Action::File(path, _) => {
                // Until the preview arrives, a dim icon holds the frame so it doesn't flash.
                pane.card(&icon, "", "", true);
                let c = Paths::discover().load().unwrap_or_default();
                if c.preview
                    && config::allowed(&c, path)
                    && let Some(is_video) = media_kind(path)
                    && let Some(player) = Player::new(path, is_video)
                {
                    if !is_video {
                        let art = gtk::Image::from_gicon(&icon);
                        art.set_pixel_size(64);
                        art.set_vexpand(true);
                        art.add_css_class("preview-card-icon");
                        pane.media.append(&art);
                    }
                    pane.media.append(player.widget());
                    pane.frame.set_visible_child_name("media");
                    *playback_select.borrow_mut() = Some(player);
                }
            }
            Action::Plugin(..) => pane.card(&icon, "", "", true),
        }
        let query = query_entry.text().to_string();
        if utility::intent(&query) == utility::Intent::Calculation {
            return;
        }
        let tx = preview_tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send((g, make_preview(&result, &query)));
        });
    });
    let pane_poll = pane.clone();
    let win_weak = window.downgrade();
    glib::timeout_add_local(Duration::from_millis(35), move || {
        if win_weak.upgrade().is_none() {
            return glib::ControlFlow::Break;
        }
        while let Ok((g, preview)) = preview_rx.try_recv() {
            if g != preview_gen.load(Ordering::Relaxed) {
                continue;
            }
            pane_poll.set_details(&preview.details);
            match preview.body {
                Body::Icon => pane_poll.card_icon.remove_css_class("dim"),
                Body::Text(markup) => pane_poll.text(&markup),
                Body::Image(bytes) => match gdk::Texture::from_bytes(&glib::Bytes::from(&bytes)) {
                    Ok(texture) => pane_poll.image(&texture),
                    Err(_) => pane_poll.card_icon.remove_css_class("dim"),
                },
                Body::Media => {}
            }
        }
        glib::ControlFlow::Continue
    });
    let rows_activate = rows.clone();
    let win_activate = window.clone();
    let footer_activate = footer.clone();
    let armed_activate = armed.clone();
    list.connect_row_activated(move |_, row| {
        if let Some(result) = rows_activate.borrow().get(row.index() as usize) {
            if let Action::Password(p) = &result.action {
                pw::activate(p, None, &win_activate, &footer_activate);
                return;
            }
            if let Action::Clipboard(c) = &result.action {
                cb::activate(c, cb::How::Paste, &win_activate, &footer_activate);
                return;
            }
            activate(result, &win_activate, &footer_activate, &armed_activate, false);
        }
    });
    let actions = ActionPanel::new(&entry, &window, &footer, &entry, &armed);
    let actions_button = actions.clone();
    let actions_rows = rows.clone();
    let actions_list = list.clone();
    let binding = Paths::discover().load().unwrap_or_default().action_key;
    let caption =
        gtk::accelerator_parse(&binding).map(|(key, mods)| gtk::accelerator_get_label(key, mods).to_string()).unwrap_or(binding);
    status_bar.append(&nyx_ui::chrome::status_action("view-more-symbolic", &format!("Actions ({caption})"), move || {
        if let Some(result) = actions_list.selected_row().and_then(|r| actions_rows.borrow().get(r.index() as usize).cloned()) {
            actions_button.open(result);
        }
    }));
    let native_action = gio::SimpleAction::new("context-actions", None);
    let native_panel = actions.clone();
    let native_rows = rows.clone();
    let native_list = list.clone();
    native_action.connect_activate(move |_, _| {
        if let Some(result) = native_list.selected_row().and_then(|r| native_rows.borrow().get(r.index() as usize).cloned()) {
            native_panel.open(result);
        }
    });
    window.add_action(&native_action);
    let actions_keys = actions.clone();
    let keys = gtk::EventControllerKey::new();
    let list_keys = list.clone();
    let win_keys = window.clone();
    let rows_keys = rows.clone();
    let footer_keys = footer.clone();
    let entry_keys = entry.clone();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let scroll_keys = scroll.clone();
    let browsing_keys = browsing.clone();
    let playback_keys = playback.clone();
    let click = gtk::GestureClick::new();
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.connect_pressed(move |_, _, _, _| browsing.set(true));
    list.add_controller(click);
    keys.connect_key_pressed(move |_, key, _, mods| {
        if actions_keys.popover.is_visible() {
            return glib::Propagation::Proceed;
        }
        let c = Paths::discover().load().unwrap_or_default();
        let normalized_mods = mods & gtk::accelerator_get_default_mod_mask();
        if gtk::accelerator_parse(&c.action_key).map(|(k, m)| (k.to_lower(), m)) == Some((key.to_lower(), normalized_mods)) {
            if let Some(row) = list_keys.selected_row().and_then(|r| rows_keys.borrow().get(r.index() as usize).cloned()) {
                actions_keys.open(row);
            }
            return glib::Propagation::Stop;
        }
        if gtk::accelerator_parse(&c.favorite_key).map(|(k, m)| (k.to_lower(), m)) == Some((key.to_lower(), normalized_mods)) {
            if let Some(row) = list_keys.selected_row().and_then(|r| rows_keys.borrow().get(r.index() as usize).cloned()) {
                if let Err(e) = learning::favorite(&Paths::discover(), &row.identity()) {
                    footer_keys.set_text(&e.to_string());
                } else {
                    footer_keys.set_text("Favorite updated");
                }
            }
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::Up && entry_keys.text().is_empty() {
            actions_keys.history();
            return glib::Propagation::Stop;
        }
        match key {
            gdk::Key::Escape => {
                win_keys.set_visible(false);
            }
            gdk::Key::Down | gdk::Key::Up | gdk::Key::Page_Down | gdk::Key::Page_Up => {
                let last = rows_keys.borrow().len() as i32 - 1;
                let step = match key {
                    gdk::Key::Down => 1,
                    gdk::Key::Up => -1,
                    gdk::Key::Page_Down => 8,
                    _ => -8,
                };
                let next = list_keys.selected_row().map(|r| (r.index() + step).clamp(0, last.max(0))).unwrap_or(0);
                select_and_reveal(&list_keys, &scroll_keys, &browsing_keys, next);
            }
            // Plain Home/End stay with the search text; with Ctrl they jump through the results.
            gdk::Key::Home | gdk::Key::End if mods.contains(gdk::ModifierType::CONTROL_MASK) => {
                let last = rows_keys.borrow().len() as i32 - 1;
                select_and_reveal(&list_keys, &scroll_keys, &browsing_keys, if key == gdk::Key::Home { 0 } else { last.max(0) });
            }
            gdk::Key::p if mods.contains(gdk::ModifierType::CONTROL_MASK) => match playback_keys.borrow().as_ref() {
                Some(player) => player.toggle(),
                None => return glib::Propagation::Proceed,
            },
            gdk::Key::Return | gdk::Key::KP_Enter => {
                if let Some(row) = list_keys.selected_row()
                    && let Some(result) = rows_keys.borrow().get(row.index() as usize)
                {
                    if let Action::Password(p) = &result.action {
                        // Ctrl+Enter copies the password, Alt+Enter the one-time code.
                        let verb = if mods.contains(gdk::ModifierType::CONTROL_MASK) {
                            Some(passwords::Verb::Password)
                        } else if mods.contains(gdk::ModifierType::ALT_MASK) {
                            Some(passwords::Verb::Totp)
                        } else {
                            None
                        };
                        pw::activate(p, verb, &win_keys, &footer_keys);
                        return glib::Propagation::Stop;
                    }
                    if let Action::Clipboard(c) = &result.action {
                        // Ctrl+Enter pastes plain text, Alt+Enter only copies.
                        let how = if mods.contains(gdk::ModifierType::CONTROL_MASK) {
                            cb::How::PastePlain
                        } else if mods.contains(gdk::ModifierType::ALT_MASK) {
                            cb::How::Copy
                        } else {
                            cb::How::Paste
                        };
                        cb::activate(c, how, &win_keys, &footer_keys);
                        return glib::Propagation::Stop;
                    }
                    activate(result, &win_keys, &footer_keys, &armed, mods.contains(gdk::ModifierType::CONTROL_MASK));
                }
            }
            gdk::Key::o | gdk::Key::O if mods.contains(gdk::ModifierType::CONTROL_MASK) => {
                let selected = list_keys.selected_row().and_then(|row| rows_keys.borrow().get(row.index() as usize).cloned());
                match selected.map(|r| r.action) {
                    Some(Action::Password(p)) => pw::activate(&p, Some(passwords::Verb::Open), &win_keys, &footer_keys),
                    _ => return glib::Propagation::Proceed,
                }
            }
            // With text selected in the search box Ctrl+C copies that; otherwise the selected result.
            gdk::Key::c if mods.contains(gdk::ModifierType::CONTROL_MASK) && entry_keys.selection_bounds().is_none() => {
                let Some(row) = list_keys.selected_row() else { return glib::Propagation::Proceed };
                let Some(result) = rows_keys.borrow().get(row.index() as usize).cloned() else {
                    return glib::Propagation::Proceed;
                };
                // A vault row copies its username through the daemon, never its title.
                if let Action::Password(p) = &result.action {
                    pw::activate(p, Some(passwords::Verb::Username), &win_keys, &footer_keys);
                    return glib::Propagation::Stop;
                }
                if let Action::Clipboard(c) = &result.action {
                    cb::activate(c, cb::How::Copy, &win_keys, &footer_keys);
                    return glib::Propagation::Stop;
                }
                let text = match &result.action {
                    Action::File(path, _) => path.display().to_string(),
                    Action::Copy(value) => value.clone(),
                    _ => result.title.clone(),
                };
                win_keys.clipboard().set_text(&text);
                footer_keys.set_text("Copied");
                let footer = footer_keys.clone();
                glib::timeout_add_local_once(Duration::from_secs(2), move || {
                    if footer.text() == "Copied" {
                        footer.set_text("");
                    }
                });
            }
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    window.add_controller(keys);
    let popover = actions.popover.clone();
    window.connect_destroy(move |_| popover.unparent());
    let next_list = list.clone();
    entry.connect_next_match(move |_| navigate_result(&next_list, 1));
    let previous_list = list.clone();
    entry.connect_previous_match(move |_| navigate_result(&previous_list, -1));
    window.connect_close_request(|w| {
        w.set_visible(false);
        glib::Propagation::Stop
    });
    // A click beside the panel puts the launcher away; moving the pointer off it does not.
    let outside = gtk::GestureClick::new();
    outside.set_button(0);
    let window_outside = window.clone();
    let panel_outside = main.clone();
    outside.connect_pressed(move |_, _, x, y| {
        let inside = panel_outside
            .compute_bounds(&window_outside)
            .is_some_and(|b| b.contains_point(&gtk::graphene::Point::new(x as f32, y as f32)));
        if !inside {
            window_outside.set_visible(false);
        }
    });
    window.add_controller(outside);
    PANEL.with(|p| *p.borrow_mut() = Some(window.clone()));
    window.present();
    focus_search(&window, &entry);
    SEARCH.with(|s| *s.borrow_mut() = Some(entry.clone()));
    if let Some(query) = query {
        entry.set_text(query);
    }
}
/// Places the panel in the launcher's overlay at its requested size: centred across, with
/// the search bar fixed where the expanded panel sits centred on screen, so results grow
/// downwards from it into the middle of the screen.
fn place_panel(panel: &gtk::Box, (width, height): (i32, i32)) {
    let Some(backdrop) = panel.parent().and_downcast::<gtk::Fixed>() else { return };
    let w = panel.measure(gtk::Orientation::Horizontal, -1).0;
    let h = panel.measure(gtk::Orientation::Vertical, w).0;
    let x = ((width - w) / 2).max(0);
    let y = ((height - EXPANDED) / 2).clamp(0, (height - h).max(0));
    backdrop.move_(panel, x as f64, y as f64);
}
/// The right-hand pane: a framed content area, then a list of details. Every kind of
/// result uses the same layout, so moving through results never reshuffles the pane.
#[derive(Clone)]
struct PreviewPane {
    root: gtk::Box,
    frame: gtk::Stack,
    text: gtk::Label,
    text_scroll: gtk::ScrolledWindow,
    picture: gtk::Picture,
    media: gtk::Box,
    card_icon: gtk::Image,
    card_title: gtk::Label,
    card_desc: gtk::Label,
    app: gtk::Box,
    details: gtk::Grid,
    detail_rows: Rc<std::cell::Cell<i32>>,
    calculator: gtk::Box,
    display: gtk::Label,
}
impl PreviewPane {
    fn new(entry: &gtk::SearchEntry) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 14);
        root.add_css_class("preview");
        root.set_hexpand(true);
        let frame = gtk::Stack::new();
        frame.add_css_class("preview-frame");
        frame.set_vexpand(true);
        frame.set_overflow(gtk::Overflow::Hidden);
        let text = gtk::Label::new(None);
        text.set_wrap(true);
        text.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        text.set_xalign(0.);
        text.set_yalign(0.);
        text.add_css_class("preview-text");
        let text_scroll = gtk::ScrolledWindow::builder()
            .vscrollbar_policy(gtk::PolicyType::External)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&text)
            .build();
        frame.add_named(&text_scroll, Some("text"));
        let picture = gtk::Picture::new();
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.add_css_class("preview-picture");
        frame.add_named(&picture, Some("image"));
        let media = gtk::Box::new(gtk::Orientation::Vertical, 0);
        media.add_css_class("preview-media");
        frame.add_named(&media, Some("media"));
        let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
        card.add_css_class("preview-card");
        card.set_valign(gtk::Align::Center);
        card.set_halign(gtk::Align::Center);
        let card_icon = gtk::Image::new();
        card_icon.set_pixel_size(64);
        card.append(&card_icon);
        let card_title = gtk::Label::new(None);
        card_title.set_wrap(true);
        card_title.set_justify(gtk::Justification::Center);
        card_title.set_selectable(false);
        card_title.add_css_class("preview-card-title");
        card.append(&card_title);
        let card_desc = gtk::Label::new(None);
        card_desc.set_wrap(true);
        card_desc.set_justify(gtk::Justification::Center);
        card_desc.set_max_width_chars(42);
        card_desc.add_css_class("preview-card-desc");
        card.append(&card_desc);
        frame.add_named(&card, Some("card"));
        let app = gtk::Box::new(gtk::Orientation::Vertical, 0);
        app.add_css_class("app-card");
        frame.add_named(&app, Some("app"));
        let details = gtk::Grid::builder().row_spacing(6).column_spacing(18).build();
        details.add_css_class("preview-details");
        let (calculator, display) = calculator_preview(entry);
        calculator.set_visible(false);
        calculator.set_vexpand(true);
        root.append(&frame);
        root.append(&details);
        root.append(&calculator);
        Self {
            root,
            frame,
            text,
            text_scroll,
            picture,
            media,
            card_icon,
            card_title,
            card_desc,
            app,
            details,
            detail_rows: Rc::new(std::cell::Cell::new(0)),
            calculator,
            display,
        }
    }
    fn show_calculator(&self, on: bool) {
        self.frame.set_visible(!on);
        self.details.set_visible(!on);
        self.calculator.set_visible(on);
    }
    /// A large icon with an optional title and line of text, for results with nothing
    /// else to show. A dim icon is the placeholder while a preview loads.
    fn card(&self, icon: &gio::Icon, title: &str, desc: &str, dim: bool) {
        self.card_icon.set_from_gicon(icon);
        self.card_icon.set_css_classes(if dim { &["preview-card-icon", "dim"][..] } else { &["preview-card-icon"][..] });
        self.card_title.set_text(title);
        self.card_title.set_visible(!title.is_empty());
        self.card_desc.set_text(desc);
        self.card_desc.set_visible(!desc.is_empty());
        self.frame.set_visible_child_name("card");
    }
    /// An app: its icon, name and kind, what it does, its categories, then the facts.
    fn app_card(&self, icon: &gio::Icon, name: &str, description: &str, command: &str, facts: &AppFacts) {
        while let Some(child) = self.app.first_child() {
            self.app.remove(&child);
        }
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        let image = gtk::Image::from_gicon(icon);
        image.set_pixel_size(64);
        image.add_css_class("app-card-icon");
        head.append(&image);
        let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
        titles.set_valign(gtk::Align::Center);
        titles.set_hexpand(true);
        let title = gtk::Label::new(Some(name));
        title.add_css_class("app-card-name");
        title.set_xalign(0.);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        titles.append(&title);
        let kind = if facts.kind.is_empty() { "Application" } else { facts.kind.as_str() };
        let kind = gtk::Label::new(Some(kind));
        kind.add_css_class("app-card-kind");
        kind.set_xalign(0.);
        kind.set_ellipsize(gtk::pango::EllipsizeMode::End);
        titles.append(&kind);
        head.append(&titles);
        self.app.append(&head);
        if !description.is_empty() {
            let desc = gtk::Label::new(Some(description));
            desc.add_css_class("app-card-desc");
            desc.set_xalign(0.);
            desc.set_wrap(true);
            desc.set_max_width_chars(60);
            self.app.append(&desc);
        }
        if !facts.categories.is_empty() {
            let chips = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            chips.add_css_class("app-card-chips");
            for c in &facts.categories {
                chips.append(&nyx_ui::widgets::label(c, "app-chip"));
            }
            self.app.append(&chips);
        }
        let info = gtk::Grid::builder().row_spacing(8).column_spacing(18).build();
        info.add_css_class("app-card-info");
        let mut row = 0;
        for (key, value) in [("Command", command), ("Source", facts.source), ("Keywords", facts.keywords.as_str())] {
            if value.is_empty() {
                continue;
            }
            let k = gtk::Label::new(Some(key));
            k.add_css_class("detail-key");
            k.set_xalign(0.);
            k.set_valign(gtk::Align::Start);
            let v = gtk::Label::new(Some(value));
            v.add_css_class(if key == "Command" { "app-card-command" } else { "detail-value" });
            v.set_xalign(0.);
            v.set_hexpand(true);
            v.set_ellipsize(gtk::pango::EllipsizeMode::End);
            v.set_tooltip_text(Some(value));
            info.attach(&k, 0, row, 1, 1);
            info.attach(&v, 1, row, 1, 1);
            row += 1;
        }
        let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        spacer.set_vexpand(true);
        self.app.append(&spacer);
        self.app.append(&info);
        self.frame.set_visible_child_name("app");
    }
    fn text(&self, markup: &str) {
        self.text.set_markup(markup);
        self.text_scroll.vadjustment().set_value(0.);
        self.frame.set_visible_child_name("text");
    }
    fn image(&self, texture: &gdk::Texture) {
        self.picture.set_paintable(Some(texture));
        self.frame.set_visible_child_name("image");
    }
    fn clear_details(&self) {
        while let Some(child) = self.details.first_child() {
            self.details.remove(&child);
        }
        self.detail_rows.set(0);
    }
    fn add_detail(&self, key: &str, value: &str) {
        let row = self.detail_rows.get();
        let k = gtk::Label::new(Some(key));
        k.set_xalign(0.);
        k.set_valign(gtk::Align::Start);
        k.add_css_class("detail-key");
        let v = gtk::Label::new(Some(value));
        v.set_xalign(0.);
        v.set_hexpand(true);
        // A folder keeps its end; anything else keeps its start.
        v.set_ellipsize(if key == "Folder" { gtk::pango::EllipsizeMode::Start } else { gtk::pango::EllipsizeMode::End });
        v.set_tooltip_text(Some(value));
        v.add_css_class("detail-value");
        self.details.attach(&k, 0, row, 1, 1);
        self.details.attach(&v, 1, row, 1, 1);
        self.detail_rows.set(row + 1);
    }
    fn set_details(&self, details: &[(String, String)]) {
        self.clear_details();
        for (key, value) in details {
            self.add_detail(key, value);
        }
    }
}
/// What the preview thread hands back: the content to frame, and the details below it.
enum Body {
    /// Nothing to show beyond the details: the frame shows the result's icon.
    Icon,
    /// Pango markup for the text frame.
    Text(String),
    /// An encoded image (a picture, a PDF page, a video still).
    Image(Vec<u8>),
    /// Leave the inline player that was attached on selection.
    Media,
}
struct Preview {
    body: Body,
    details: Vec<(String, String)>,
}
thread_local! {static APPS: RefCell<std::collections::HashMap<String, gio::AppInfo>> = RefCell::new(Default::default());}
fn action_icon(action: &Action) -> gio::Icon {
    let icon = match action {
        Action::App(id) => APPS.with(|apps| apps.borrow().get(id).and_then(|app| app.icon())),
        Action::File(path, _) => Some(gio::content_type_get_symbolic_icon(&content_type(path))),
        _ => None,
    };
    icon.unwrap_or_else(|| gio::ThemedIcon::new(symbolic_icon(action)).upcast())
}
const RELATED: u8 = 5;
fn count_text(n: usize) -> String {
    match n {
        0 => String::new(),
        1 => "1 result".into(),
        n => format!("{n} results"),
    }
}
fn system_description(cmd: &str) -> &'static str {
    match cmd {
        "lock" => "Lock the screen and turn off the display.",
        "logout" => "Log out after closing application windows.",
        "reboot" => "Restart after closing application windows.",
        _ => "Shut down after closing application windows.",
    }
}
fn show_empty(slot: &gtk::Box, query: &str) {
    while let Some(child) = slot.first_child() {
        slot.remove(&child);
    }
    slot.append(&nyx_ui::widgets::empty_state(
        "system-search-symbolic",
        &format!("No results for “{query}”"),
        "Try fewer or different words, or add a folder to search.",
        None,
    ));
}
/// The status bar's key hints for what Enter and the shortcuts do to this result.
fn set_hints(hints: &gtk::Box, action: Option<&Action>) {
    while let Some(child) = hints.first_child() {
        hints.remove(&child);
    }
    let enter: &[&str] = &["Enter"];
    let list: Vec<(&[&str], &str)> = match action {
        Some(Action::File(path, _)) => {
            let mut list = vec![(enter, "Open"), (&["Ctrl", "Enter"][..], "Reveal"), (&["Ctrl", "C"][..], "Copy")];
            if media_kind(path).is_some() {
                list.push((&["Ctrl", "P"][..], "Play"));
            }
            list
        }
        Some(Action::App(_)) => vec![(enter, "Launch")],
        Some(Action::Copy(_)) => vec![(enter, "Copy")],
        Some(Action::System("lock")) => vec![(enter, "Lock")],
        Some(Action::System(_)) | Some(Action::Command(_)) => vec![(enter, "Run")],
        Some(Action::Plugin(..)) => vec![(enter, "Open")],
        Some(Action::Password(p)) => p.hints(),
        Some(Action::Clipboard(c)) => c.hints(),
        None => vec![],
    };
    for (keys, label) in list {
        let hint = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let caps = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        caps.set_valign(gtk::Align::Center);
        for key in keys {
            caps.append(&nyx_ui::widgets::label(key, "hint-key"));
        }
        hint.append(&caps);
        hint.append(&nyx_ui::widgets::label(label, "hint-label"));
        hints.append(&hint);
    }
}
fn symbolic_icon(action: &Action) -> &'static str {
    match action {
        Action::App(_) => "application-x-executable-symbolic",
        Action::File(..) => "text-x-generic-symbolic",
        Action::Copy(_) => "accessories-calculator-symbolic",
        Action::System("lock") => "system-lock-screen-symbolic",
        Action::System("logout") => "system-log-out-symbolic",
        Action::System("reboot") => "system-reboot-symbolic",
        Action::System(_) => "system-shutdown-symbolic",
        Action::Command(_) => "system-run-symbolic",
        Action::Plugin(..) => "application-x-addon-symbolic",
        Action::Password(p) => p.icon(),
        Action::Clipboard(c) => c.icon(),
    }
}
/// Apps show their own icon; files the symbolic icon for their type.
fn result_icon(action: &Action) -> gtk::Image {
    gtk::Image::from_gicon(&action_icon(action))
}
fn result_row(result: &ResultRow) -> gtk::ListBoxRow {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    line.add_css_class("result");
    let icon = result_icon(&result.action);
    icon.set_pixel_size(20);
    icon.set_valign(gtk::Align::Center);
    icon.add_css_class(if matches!(result.action, Action::App(_)) { "result-app-icon" } else { "result-icon" });
    line.append(&icon);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let title = gtk::Label::new(Some(&result.title));
    title.set_xalign(0.);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.add_css_class("result-title");
    text.append(&title);
    if !result.folder.is_empty() {
        let folder = gtk::Label::new(Some(&result.folder));
        folder.set_xalign(0.);
        // A folder keeps its end, which is what tells two README.md files apart.
        let file = matches!(result.action, Action::File(..));
        folder.set_ellipsize(if file { gtk::pango::EllipsizeMode::Start } else { gtk::pango::EllipsizeMode::End });
        folder.add_css_class("result-folder");
        text.append(&folder);
    }
    line.append(&text);
    if Paths::discover().load().is_ok_and(|c| c.favorites.contains(&result.identity())) {
        let favorite = gtk::Image::from_icon_name("starred-symbolic");
        favorite.set_pixel_size(14);
        favorite.add_css_class("result-icon");
        favorite.set_tooltip_text(Some("Favorite"));
        line.append(&favorite);
    }
    if !result.reason.is_empty() {
        line.append(&nyx_ui::widgets::tag(&result.reason, if result.reason == "Related" { "" } else { "accent" }));
    }
    let row = gtk::ListBoxRow::new();
    row.add_css_class(if result.folder.is_empty() { "single" } else { "double" });
    if let Action::File(path, _) = &result.action {
        row.set_tooltip_text(Some(&path.display().to_string()));
    }
    row.set_child(Some(&line));
    row
}
fn calculator_preview(entry: &gtk::SearchEntry) -> (gtk::Box, gtk::Label) {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.add_css_class("calculator");
    let display = gtk::Label::new(None);
    display.set_wrap(true);
    display.set_selectable(true);
    display.set_xalign(1.);
    display.add_css_class("calculation-display");
    root.append(&display);
    let grid = gtk::Grid::builder().row_spacing(6).column_spacing(6).column_homogeneous(true).build();
    for (i, label) in ["C", "⌫", "(", ")", "7", "8", "9", "÷", "4", "5", "6", "×", "1", "2", "3", "−", "0", ".", "=", "+"]
        .into_iter()
        .enumerate()
    {
        let button = gtk::Button::with_label(label);
        button.set_hexpand(true);
        button.set_focusable(false);
        let entry = entry.clone();
        button.connect_clicked(move |_| {
            match label {
                "C" => {
                    entry.set_text("=");
                    entry.set_position(-1);
                }
                "⌫" => {
                    if let Some((start, end)) = entry.selection_bounds() {
                        entry.delete_text(start, end);
                        entry.set_position(start);
                    } else {
                        let end = entry.position();
                        if end > 0 {
                            entry.delete_text(end - 1, end);
                            entry.set_position(end - 1);
                        }
                    }
                }
                "=" => {}
                text => {
                    let mut pos = entry.position();
                    if let Some((start, end)) = entry.selection_bounds() {
                        entry.delete_text(start, end);
                        pos = start;
                    }
                    entry.insert_text(text, &mut pos);
                    entry.set_position(pos);
                }
            }
            let position = entry.position();
            entry.grab_focus();
            entry.select_region(position, position);
            entry.set_position(position);
        });
        grid.attach(&button, (i % 4) as i32, (i / 4) as i32, 1, 1);
    }
    root.append(&grid);
    let copy = gtk::Button::with_label("Copy answer");
    copy.set_focusable(false);
    let input = entry.clone();
    copy.connect_clicked(move |_| {
        if let Ok(answers) = utility::answers(&input.text(), &Paths::discover().load().unwrap_or_default())
            && let Some(answer) = answers.first()
        {
            input.clipboard().set_text(&answer.copy);
        }
        let position = input.position();
        input.grab_focus();
        input.select_region(position, position);
        input.set_position(position);
    });
    root.append(&copy);
    (root, display)
}
/// Builds a file or plugin preview off the main thread. Apps, utilities and system
/// actions are previewed directly on selection.
fn make_preview(result: &ResultRow, query: &str) -> Preview {
    let note = |text: &str| Preview { body: Body::Text(glib::markup_escape_text(text).to_string()), details: vec![] };
    let p = Paths::discover();
    let Ok(c) = p.load() else {
        return note("Glimpse's preferences could not be read.");
    };
    match &result.action {
        Action::File(path, hit) => {
            if !config::allowed(&c, path) {
                return note("This folder is no longer searched.");
            }
            let Ok(meta) = path.metadata() else {
                return note("This file is no longer available.");
            };
            let mut details = file_details(path, &meta, hit);
            let body = file_body(path, hit, query, &mut details);
            Preview { body, details }
        }
        Action::Plugin(id, result_id, _) => {
            let plugin = plugins::get(&p, id).ok();
            let mut details = vec![("Plugin".to_string(), plugin.as_ref().map_or(id.clone(), |p| p.manifest.name.clone()))];
            if !result.folder.is_empty() {
                details.insert(0, ("Result".into(), result.folder.clone()));
            }
            let text = plugin
                .filter(|plugin| plugin.manifest.capabilities.contains(&"preview".into()))
                .and_then(|_| {
                    plugins::call(&p, id, "preview", "preview", json!({"result_id":result_id,"title":result.title,"query":query}))
                        .ok()
                })
                .and_then(|v| v["text"].as_str().map(|s| s.chars().take(16000).collect::<String>()));
            Preview { body: text.map_or(Body::Icon, |t| Body::Text(highlight(&t, query))), details }
        }
        _ => Preview { body: Body::Icon, details: vec![] },
    }
}
/// Kind, size, date, folder and where the match is: never the name, which the list shows.
fn file_details(path: &Path, meta: &std::fs::Metadata, hit: &Value) -> Vec<(String, String)> {
    let kind = gio::content_type_get_description(&content_type(path)).to_string();
    let mut chars = kind.chars();
    let kind = chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or(kind);
    let mut details = vec![("Kind".to_string(), kind)];
    if meta.is_file() {
        details.push(("Size".into(), glib::format_size(meta.len()).to_string()));
    }
    if let Ok(modified) = meta.modified() {
        details.push(("Modified".into(), when(modified)));
    }
    details.push(("Folder".into(), folder_of(path)));
    if let Some(place) = match_place(hit) {
        details.push(("Match".into(), place));
    } else if hit["related"] == true {
        details.push(("Match".into(), "Similar meaning".into()));
    } else if !hit.is_null() {
        details.push(("Match".into(), "In the contents".into()));
    }
    details
}
/// "Today, 14:02", "Yesterday, 09:15", "Oct 12" or "Oct 12, 2024".
fn when(time: std::time::SystemTime) -> String {
    let secs = time.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let (Ok(at), Ok(now)) = (glib::DateTime::from_unix_local(secs), glib::DateTime::now_local()) else {
        return String::new();
    };
    let yesterday = now.add_days(-1).map(|d| d.ymd()).ok();
    let format = if at.ymd() == now.ymd() {
        "Today, %H:%M"
    } else if Some(at.ymd()) == yesterday {
        "Yesterday, %H:%M"
    } else if at.year() == now.year() {
        "%b %-d"
    } else {
        "%b %-d, %Y"
    };
    at.format(format).map(|s| s.to_string()).unwrap_or_default()
}
fn capture_image(cmd: Command) -> Option<Vec<u8>> {
    nyx_glimpse::process::capture(cmd, vec![], Duration::from_secs(3), 6 * 1024 * 1024).ok().filter(|b| !b.is_empty())
}
fn file_body(path: &Path, hit: &Value, query: &str, details: &mut Vec<(String, String)>) -> Body {
    let ext = path.extension().unwrap_or_default().to_string_lossy().to_lowercase();
    if let Some(is_video) = media_kind(path) {
        if let Some(seconds) = media_length(path) {
            details.insert(2, ("Length".into(), clock(seconds)));
        }
        if crate::player::available() {
            return Body::Media;
        }
        // Without libmpv: a still from a few seconds in, past any black intro.
        if is_video {
            for seek in ["5", "0"] {
                let mut cmd = Command::new("ffmpeg");
                cmd.args(["-v", "error", "-threads", "1", "-ss", seek, "-i"]).arg(path).args([
                    "-frames:v",
                    "1",
                    "-vf",
                    "scale=800:800:force_original_aspect_ratio=decrease",
                    "-f",
                    "image2pipe",
                    "-vcodec",
                    "png",
                    "-",
                ]);
                if let Some(bytes) = capture_image(cmd) {
                    return Body::Image(bytes);
                }
            }
        }
        return Body::Icon;
    }
    if ext == "pdf" {
        let page = hit["info"]["page"]
            .as_u64()
            .or_else(|| hit["loc"].as_f64().filter(|_| hit["kind"] == "pdf").map(|n| n as u64))
            .unwrap_or(1)
            .max(1)
            .to_string();
        let mut image = Command::new("pdftoppm");
        image.args(["-f", &page, "-l", &page, "-singlefile", "-scale-to", "800", "-png"]).arg(path);
        if let Some(bytes) = capture_image(image) {
            return Body::Image(bytes);
        }
        let mut cmd = Command::new("pdftotext");
        cmd.args(["-f", &page, "-l", &page, "--"]).arg(path).arg("-");
        if let Ok(bytes) = nyx_glimpse::process::capture(cmd, vec![], Duration::from_secs(3), 65536) {
            return text_body(&String::from_utf8_lossy(&bytes), path, hit, query);
        }
        return Body::Icon;
    }
    if ["png", "jpg", "jpeg", "webp", "gif", "heic", "heif", "bmp", "tiff", "tif", "svg", "avif"].contains(&ext.as_str()) {
        if let Some((_, width, height)) = gtk::gdk_pixbuf::Pixbuf::file_info(path) {
            details.insert(1, ("Dimensions".into(), format!("{width} × {height}")));
        }
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-v", "error", "-threads", "1", "-i"]).arg(path).args([
            "-frames:v",
            "1",
            "-vf",
            "scale=800:800:force_original_aspect_ratio=decrease",
            "-f",
            "image2pipe",
            "-vcodec",
            "png",
            "-",
        ]);
        return capture_image(cmd).map_or(Body::Icon, Body::Image);
    }
    if ["docx", "odt", "pptx", "xlsx", "ods", "odp"].contains(&ext.as_str()) {
        let member = match ext.as_str() {
            "docx" => "word/document.xml",
            "pptx" => "ppt/slides/slide*.xml",
            "xlsx" => "xl/sharedStrings.xml",
            _ => "content.xml",
        };
        let mut cmd = Command::new("unzip");
        cmd.arg("-p").arg(path).arg(member);
        if let Ok(bytes) = nyx_glimpse::process::capture(cmd, vec![], Duration::from_secs(3), 2 * 1024 * 1024) {
            let text = semantic::strip_markup(&String::from_utf8_lossy(&bytes));
            // Office XML has no line breaks worth keeping; collapse the runs of spaces.
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            return text_body(&text, path, hit, query);
        }
        return Body::Icon;
    }
    if let Ok(f) = std::fs::File::open(path) {
        let mut bytes = Vec::new();
        if f.take(65536).read_to_end(&mut bytes).is_ok() && !bytes.is_empty() && !bytes.contains(&0) {
            return text_body(&String::from_utf8_lossy(&bytes), path, hit, query);
        }
    }
    Body::Icon
}
/// The passage that matched: the hit's line, the semantic excerpt, or the first line
/// with the search word; the start of the file when nothing points elsewhere.
fn text_body(text: &str, path: &Path, hit: &Value, query: &str) -> Body {
    let keyword = query.split_whitespace().find(|s| s.chars().count() >= 3).unwrap_or("").to_lowercase();
    let line_hit = hit["info"]["line"]
        .as_u64()
        .or_else(|| hit["loc"].as_f64().filter(|_| hit["segment"] == "lines").map(|n| n as u64))
        .map(|n| n as usize);
    let keyword_line = (!keyword.is_empty())
        .then(|| text.to_lowercase().find(&keyword).map(|at| text.to_lowercase()[..at].matches('\n').count() + 1))
        .flatten();
    let from_line = |line: usize| text.lines().skip(line.saturating_sub(3)).collect::<Vec<_>>().join("\n");
    let excerpt = if let Some(line) = line_hit {
        from_line(line)
    } else if let Some(line) = keyword_line {
        from_line(line)
    } else if let Some(excerpt) = hit["excerpt"].as_str() {
        excerpt.to_string()
    } else {
        text.to_string()
    };
    let excerpt: String = excerpt.trim_start_matches('\n').chars().take(12000).collect();
    Body::Text(code_markup(&excerpt, path).unwrap_or_else(|| highlight(&excerpt, query)))
}
fn media_length(path: &Path) -> Option<f64> {
    let mut probe = Command::new("ffprobe");
    probe.args(["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1"]).arg(path);
    let bytes = nyx_glimpse::process::capture(probe, vec![], Duration::from_secs(2), 256).ok()?;
    String::from_utf8_lossy(&bytes).trim().parse::<f64>().ok().filter(|n| n.is_finite() && *n > 0.)
}
fn activate(
    result: &ResultRow,
    window: &gtk::ApplicationWindow,
    status: &gtk::Label,
    armed: &Rc<RefCell<Option<String>>>,
    reveal: bool,
) {
    if let Action::Command(target) = &result.action {
        dispatch_command(target.clone(), result.clone(), window.clone(), status.clone(), armed.clone());
        return;
    }
    if let Action::Plugin(id, result_id, plugins::Action::Activate { token }) = &result.action {
        if let Some(result) = plugins::notes_host_action(&Paths::discover(), id, token) {
            match result {
                Ok(()) => window.set_visible(false),
                Err(e) => status_error(status, e),
            }
            return;
        }
        if let Some(flag) = plugins::capture_host_args(id, token) {
            match plugins::spawn_capture(flag) {
                Ok(()) => window.set_visible(false),
                Err(e) => status_error(status, e),
            }
            return;
        }
        if let Some(done) = nyx_glimpse::pim::host_action(&Paths::discover(), id, token) {
            match done {
                Ok(()) => window.set_visible(false),
                Err(e) => status_error(status, e),
            }
            return;
        }
        let (id, result_id, token) = (id.clone(), result_id.clone(), token.clone());
        let (tx, rx) = mpsc::channel();
        let worker_id = id.clone();
        std::thread::spawn(move || {
            let _ = tx.send(plugins::activate(&Paths::discover(), &worker_id, &token).map_err(|e| e.to_string()));
        });
        let window = window.clone();
        let status = status.clone();
        let activation_query = SEARCH.with(|s| s.borrow().as_ref().map(|e| e.text().to_string()));
        status.set_text("Running plugin action…");
        glib::timeout_add_local(Duration::from_millis(25), move || {
            if !window.is_visible() || SEARCH.with(|s| s.borrow().as_ref().map(|e| e.text().to_string())) != activation_query {
                return glib::ControlFlow::Break;
            }
            if let Ok(reply) = rx.try_recv() {
                match reply {
                    Ok(action) => activate(
                        &ResultRow {
                            title: String::new(),
                            folder: String::new(),
                            reason: String::new(),
                            action: Action::Plugin(id.clone(), result_id.clone(), action),
                        },
                        &window,
                        &status,
                        &Rc::new(RefCell::new(None)),
                        reveal,
                    ),
                    Err(e) => status.set_text(&e),
                }
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
        return;
    }
    let p = Paths::discover();
    let action = result.action.clone();
    let perform = (|| -> anyhow::Result<()> {
        match action {
            Action::Copy(text) => window.clipboard().set_text(&text),
            Action::App(id) => {
                let providers::Effect::LaunchApp(id, _) = providers::resolve(&p, &providers::Target::App(id), "open")? else {
                    unreachable!()
                };
                gio_unix::DesktopAppInfo::new(&id)
                    .ok_or_else(|| anyhow::anyhow!("Application unavailable"))?
                    .launch(&[], None::<&gio::AppLaunchContext>)?;
            }
            Action::File(path, hit) => {
                let providers::Effect::OpenFile(path, reveal) =
                    providers::resolve(&p, &providers::Target::File(path), if reveal { "reveal" } else { "open" })?
                else {
                    unreachable!()
                };
                open_file(&path, &hit, reveal)?;
            }
            Action::System(cmd) => {
                if cmd != "lock" && armed.borrow().as_deref() != Some(cmd) {
                    *armed.borrow_mut() = Some(cmd.into());
                    status.set_text("Press Enter again to confirm");
                    return Ok(());
                }
                system_command(cmd).spawn()?;
                *armed.borrow_mut() = None;
            }
            Action::Plugin(id, _, action) => {
                let plugin = plugins::get(&p, &id)?;
                if !plugins::authorized(&plugin) || !plugins::valid_action(&p, &plugin, &action) {
                    anyhow::bail!("plugin permission revoked");
                }
                let action = if let plugins::Action::Activate { token } = action {
                    if let Some(flag) = plugins::capture_host_args(&id, &token) {
                        plugins::spawn_capture(flag)?;
                        return Ok(());
                    }
                    plugins::activate(&p, &id, &token)?
                } else {
                    action
                };
                match action {
                    plugins::Action::Copy { text } => window.clipboard().set_text(&text),
                    plugins::Action::Open { path } => open_file(&path, &Value::Null, reveal)?,
                    plugins::Action::Url { url } => gio::AppInfo::launch_default_for_uri(&url, None::<&gio::AppLaunchContext>)?,
                    plugins::Action::Activate { .. } => unreachable!(),
                }
            }
            // Routed to `pw::activate` before this; nothing to do here.
            Action::Password(_) | Action::Clipboard(_) | Action::Command(_) => {}
        }
        Ok(())
    })();
    match perform {
        Err(e) => status_error(status, e),
        Ok(()) => {
            if armed.borrow().is_none() {
                record_selection(result);
                window.set_visible(false);
            }
        }
    }
}
fn open_file(path: &Path, hit: &Value, reveal: bool) -> anyhow::Result<()> {
    if reveal {
        let parent = path.parent().unwrap_or(path);
        gio::AppInfo::launch_default_for_uri(&gio::File::for_path(parent).uri(), None::<&gio::AppLaunchContext>)?;
        return Ok(());
    }
    let uri = gio::File::for_path(path).uri();
    let content_type = content_type(path);
    if let Some(app) = gio::AppInfo::default_for_type(&content_type, false)
        && let Some(id) = app.id()
    {
        let page = hit["info"]["page"]
            .as_u64()
            .or_else(|| hit["loc"].as_f64().filter(|_| hit["kind"] == "pdf").map(|n| n as u64))
            .unwrap_or(1)
            .max(1);
        let moment = hit["info"]["seconds"]
            .as_f64()
            .or_else(|| hit["loc"].as_f64().filter(|_| hit["kind"] == "audio" || hit["kind"] == "video"))
            .filter(|n| n.is_finite() && *n >= 0.);
        let line = hit["info"]["line"]
            .as_u64()
            .or_else(|| hit["focus"].as_u64())
            .or_else(|| hit["loc"].as_f64().filter(|_| hit["segment"] == "lines").map(|n| n as u64));
        let mut special: Option<Command> = None;
        if content_type == "application/pdf" {
            special = match id.as_str() {
                "io.github.design_nexus.nyx.Pdf.desktop" => {
                    let mut c = Command::new("nyx-pdf");
                    c.args(["--page", &page.to_string()]);
                    Some(c)
                }
                "org.gnome.Evince.desktop" => {
                    let mut c = Command::new("evince");
                    c.arg(format!("--page-label={page}"));
                    Some(c)
                }
                "org.kde.okular.desktop" => {
                    let mut c = Command::new("okular");
                    c.args(["--page", &page.to_string()]);
                    Some(c)
                }
                "org.pwmt.zathura.desktop" => {
                    let mut c = Command::new("zathura");
                    c.args(["-P", &page.to_string()]);
                    Some(c)
                }
                _ => None,
            };
        }
        if let Some(seconds) = moment {
            special = match id.as_str() {
                "mpv.desktop" => {
                    let mut c = Command::new("mpv");
                    c.arg(format!("--start={seconds}"));
                    Some(c)
                }
                "vlc.desktop" | "org.videolan.VLC.desktop" => {
                    let mut c = Command::new("vlc");
                    c.arg(format!("--start-time={seconds}"));
                    Some(c)
                }
                _ => None,
            };
        }
        if let Some(line) = line {
            match id.as_str() {
                "code.desktop" | "code-url-handler.desktop" | "codium.desktop" => {
                    let mut c = Command::new(if id.starts_with("codium") { "codium" } else { "code" });
                    c.args(["-g", &format!("{}:{line}", path.display())]).spawn()?;
                    return Ok(());
                }
                "dev.zed.Zed.desktop" => {
                    Command::new("zed").arg(format!("{}:{line}", path.display())).spawn()?;
                    return Ok(());
                }
                _ => {}
            }
        }
        if let Some(mut cmd) = special {
            cmd.arg(path).spawn()?;
            return Ok(());
        }
    }
    gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>)?;
    Ok(())
}

fn highlight(text: &str, query: &str) -> String {
    let keyword = query.split_whitespace().find(|s| s.chars().count() >= 3).unwrap_or("");
    if keyword.is_empty() {
        return glib::markup_escape_text(text).to_string();
    }
    // Match regardless of case, but show the text as it is written.
    let needle: Vec<char> = keyword.chars().flat_map(char::to_lowercase).collect();
    let match_at = |start: usize| {
        let mut matched = 0;
        for (offset, ch) in text[start..].char_indices() {
            for lower in ch.to_lowercase() {
                if needle.get(matched) != Some(&lower) {
                    return None;
                }
                matched += 1;
            }
            if matched == needle.len() {
                return Some(start + offset + ch.len_utf8());
            }
        }
        None
    };
    let mut out = String::new();
    let (mut plain, mut at) = (0, 0);
    while let Some(ch) = text[at..].chars().next() {
        if let Some(end) = match_at(at) {
            out.push_str(&glib::markup_escape_text(&text[plain..at]));
            out.push_str(&format!("<b>{}</b>", glib::markup_escape_text(&text[at..end])));
            (plain, at) = (end, end);
        } else {
            at += ch.len_utf8();
        }
    }
    out.push_str(&glib::markup_escape_text(&text[plain..]));
    out
}
fn close_controls(window: &gtk::ApplicationWindow, root: &gtk::Box) {
    let close = gtk::Button::with_label("Close");
    close.set_halign(gtk::Align::End);
    let weak = window.downgrade();
    close.connect_clicked(move |_| {
        if let Some(w) = weak.upgrade() {
            w.close();
        }
    });
    root.append(&close);
    escape_closes(window);
}
/// Esc closes the window.
fn escape_closes(window: &gtk::ApplicationWindow) {
    if window.has_css_class("escape-close") {
        return;
    }
    window.add_css_class("escape-close");
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = window.downgrade();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape {
            if let Some(w) = weak.upgrade() {
                w.close();
            }
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(keys);
}
fn task_window(app: &gtk::Application, title: &str) -> (gtk::ApplicationWindow, gtk::Label) {
    let parent = gtk::ApplicationWindow::builder().application(app).title(title).default_width(500).default_height(180).build();
    parent.add_css_class("glimpse-window");
    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.add_css_class("glimpse-dialog");
    root.set_margin_top(16);
    root.set_margin_start(16);
    root.set_margin_end(16);
    root.set_margin_bottom(16);
    let label = gtk::Label::new(Some(title));
    label.set_wrap(true);
    root.append(&label);
    close_controls(&parent, &root);
    parent.set_child(Some(&root));
    (parent, label)
}
pub fn add_folder(app: &gtk::Application) {
    dismiss();
    let (parent, label) = task_window(app, "Glimpse · Add folder");
    parent.present();
    let dialog = gtk::FileDialog::builder().title("Choose a folder for Glimpse").build();
    dialog.select_folder(Some(&parent.clone()), None::<&gio::Cancellable>, move |result| {
        if let Ok(file) = result
            && let Some(path) = file.path()
        {
            parent.present();
            run_task(label, move || {
                nyx_glimpse::cli::dispatch(&["sources".into(), "add".into(), path.display().to_string()])?;
                Ok("Folder added. Background indexing is enabled.".into())
            });
        } else {
            parent.close();
        }
    });
}
pub fn add_plugin(app: &gtk::Application) {
    dismiss();
    let (parent, label) = task_window(app, "Glimpse · Add plugin");
    parent.present();
    let dialog = gtk::FileDialog::builder().title("Choose a plugin folder").build();
    dialog.select_folder(Some(&parent.clone()), None::<&gio::Cancellable>, move |result| {
        if let Ok(file) = result
            && let Some(path) = file.path()
        {
            parent.present();
            run_task(label, move || {
                plugins::install_local(&Paths::discover(), &path, &format!("local:{}", path.display()), "", false)?;
                Ok("Plugin installed, disabled. Review its permissions in Plugins.".into())
            });
        } else {
            parent.close();
        }
    });
}
/// The desktop entry for app `id`: where it was found, and its contents.
fn desktop_entry(id: &str) -> Option<(PathBuf, glib::KeyFile)> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
    let mut dirs = vec![data];
    dirs.extend(std::env::split_paths(
        &std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into()),
    ));
    dirs.into_iter().find_map(|dir| {
        let path = dir.join("applications").join(id);
        let key = glib::KeyFile::new();
        key.load_from_file(&path, glib::KeyFileFlags::NONE).ok().map(|_| (path, key))
    })
}
fn app_keywords(id: &str) -> String {
    desktop_entry(id)
        .and_then(|(_, key)| key.string_list("Desktop Entry", "Keywords").ok())
        .map(|v| v.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}
/// What an app card shows beyond the name and description.
struct AppFacts {
    kind: String,
    categories: Vec<&'static str>,
    keywords: String,
    source: &'static str,
}
fn app_facts(id: &str) -> AppFacts {
    let Some((path, key)) = desktop_entry(id) else {
        return AppFacts { kind: String::new(), categories: Vec::new(), keywords: String::new(), source: "" };
    };
    let kind = key.locale_string("Desktop Entry", "GenericName", None).map(|s| s.to_string()).unwrap_or_default();
    let listed: Vec<String> =
        key.string_list("Desktop Entry", "Categories").map(|v| v.iter().map(|s| s.to_string()).collect()).unwrap_or_default();
    // The freedesktop categories people recognise, in plain words; the rest are skipped.
    const NAMES: &[(&str, &str)] = &[
        ("AudioVideo", "Media"),
        ("Audio", "Audio"),
        ("Video", "Video"),
        ("Development", "Development"),
        ("Education", "Education"),
        ("Game", "Games"),
        ("Graphics", "Graphics"),
        ("Network", "Internet"),
        ("Office", "Office"),
        ("Science", "Science"),
        ("Settings", "Settings"),
        ("System", "System"),
        ("Utility", "Utilities"),
        ("FileManager", "File manager"),
        ("TerminalEmulator", "Terminal"),
        ("WebBrowser", "Web browser"),
        ("TextEditor", "Text editor"),
        ("IDE", "IDE"),
        ("Email", "Email"),
        ("Chat", "Chat"),
        ("Player", "Player"),
    ];
    let mut categories: Vec<&'static str> =
        listed.iter().filter_map(|c| NAMES.iter().find(|(k, _)| k == c).map(|(_, v)| *v)).collect();
    categories.dedup();
    categories.truncate(4);
    let keywords = key
        .locale_string_list("Desktop Entry", "Keywords", None)
        .map(|v| v.iter().map(|s| s.to_string()).filter(|s| !s.is_empty()).take(6).collect::<Vec<_>>().join(", "))
        .unwrap_or_default();
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let shown = path.to_string_lossy();
    let source = if shown.contains("/flatpak/") {
        "Flatpak"
    } else if !home.as_os_str().is_empty() && path.starts_with(&home) {
        "Installed for you"
    } else {
        "System"
    };
    AppFacts { kind, categories, keywords, source }
}
thread_local! {
    /// The plugin window's open tab ("installed" or "browse"), kept across rebuilds.
    static PLUGIN_TAB: RefCell<String> = RefCell::new("installed".into());
}
pub fn plugin_manager(app: &gtk::Application) {
    dismiss();
    if let Some(win) = app.windows().into_iter().find(|w| w.has_css_class("plugins-window")) {
        win.present();
        return;
    }
    // A fixed size: Hyprland floats it like a dialog instead of tiling it.
    let win = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Glimpse · Plugins")
        .default_width(720)
        .default_height(560)
        .resizable(false)
        .build();
    win.add_css_class("glimpse-window");
    win.add_css_class("plugins-window");
    win.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
    escape_closes(&win);

    let header = nyx_ui::chrome::app_header(&win, "application-x-addon-symbolic", "Glimpse");
    header.set_subtitle("Plugins");
    let pages = gtk::Stack::new();
    pages.set_vexpand(true);
    pages.set_transition_type(gtk::StackTransitionType::Crossfade);
    pages.set_transition_duration(120);
    let current = PLUGIN_TAB.with(|t| t.borrow().clone());
    let tabs =
        nyx_ui::widgets::segmented(&[("installed".into(), "Installed".into()), ("browse".into(), "Browse".into())], &current, {
            let pages = pages.clone();
            move |id| {
                pages.set_visible_child_name(&id);
                PLUGIN_TAB.with(|t| *t.borrow_mut() = id);
            }
        });
    header.center.append(&tabs);
    let note = gtk::Label::new(None);
    note.add_css_class("plugins-note");
    note.set_xalign(0.);
    note.set_wrap(true);
    header.end.set_spacing(4);
    let add = nyx_ui::widgets::bar_button("list-add-symbolic", "Add a plugin from a folder");
    let app_clone = app.clone();
    add.connect_clicked(move |_| add_plugin(&app_clone));
    header.end.append(&add);
    let refresh = nyx_ui::widgets::bar_button("view-refresh-symbolic", "Refresh the catalog");
    let st = note.clone();
    refresh.connect_clicked(move |_| {
        run_task(st.clone(), || {
            let p = Paths::discover();
            plugins::catalog(&p, true)?;
            Ok(std::fs::read_to_string(p.state.join("catalog-status.txt"))?)
        })
    });
    header.end.append(&refresh);
    let close = nyx_ui::widgets::bar_button("window-close-symbolic", "Close (Esc)");
    let weak = win.downgrade();
    close.connect_clicked(move |_| {
        if let Some(w) = weak.upgrade() {
            w.close();
        }
    });
    close.set_margin_start(6);
    header.end.append(&close);

    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.add_css_class("window-frame");
    frame.append(&header.root);
    frame.append(&pages);
    frame.append(&note);
    win.set_child(Some(&frame));
    rebuild_plugin_manager(app, &pages, &note);

    let weak = win.downgrade();
    let app = app.clone();
    let mut snapshot = plugin_snapshot();
    glib::timeout_add_local(Duration::from_millis(500), move || {
        if weak.upgrade().is_none() {
            return glib::ControlFlow::Break;
        }
        let fresh = plugin_snapshot();
        if fresh != snapshot {
            snapshot = fresh;
            rebuild_plugin_manager(&app, &pages, &note);
        }
        glib::ControlFlow::Continue
    });
    win.present();
}
fn plugin_snapshot() -> Vec<Vec<u8>> {
    let p = Paths::discover();
    [p.config.join("plugins.json"), p.data.join("plugin-catalog.json"), p.state.join("catalog-status.txt")]
        .iter()
        .map(|path| std::fs::read(path).unwrap_or_default())
        .collect()
}
/// The plugin window's pages, built again whenever the plugins or the catalog change.
fn rebuild_plugin_manager(app: &gtk::Application, pages: &gtk::Stack, note: &gtk::Label) {
    let p = Paths::discover();
    if note.text().is_empty() || note.text() == "Working…" {
        let status = std::fs::read_to_string(p.state.join("catalog-status.txt")).unwrap_or_default();
        note.set_text(if status.trim().is_empty() {
            "Plugins run separately. Review a plugin's permissions before you turn it on."
        } else {
            status.trim()
        });
    }
    for name in ["installed", "browse"] {
        if let Some(old) = pages.child_by_name(name) {
            pages.remove(&old);
        }
    }
    let installed = plugins::list(&p).unwrap_or_default();
    pages.add_named(&plugin_page(&installed_page(app, &installed, note)), Some("installed"));
    pages.add_named(&plugin_page(&browse_page(&installed, note)), Some("browse"));
    pages.set_visible_child_name(&PLUGIN_TAB.with(|t| t.borrow().clone()));
}
fn plugin_page(body: &gtk::Box) -> gtk::ScrolledWindow {
    body.add_css_class("plugins-page");
    gtk::ScrolledWindow::builder()
        .vscrollbar_policy(gtk::PolicyType::External)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(body)
        .vexpand(true)
        .build()
}
fn installed_page(app: &gtk::Application, items: &[plugins::Plugin], note: &gtk::Label) -> gtk::Box {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    if items.is_empty() {
        body.append(&nyx_ui::widgets::empty_state(
            "application-x-addon-symbolic",
            "No plugins yet",
            "Install one from Browse, or add a plugin from a folder with +.",
            None,
        ));
        return body;
    }
    let p = Paths::discover();
    for plugin in items {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
        card.add_css_class("plugin-card");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let icon = gtk::Image::from_icon_name("application-x-addon-symbolic");
        icon.add_css_class("plugin-icon");
        icon.set_valign(gtk::Align::Start);
        head.append(&icon);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        let title = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        title.append(&nyx_ui::widgets::label(&plugin.manifest.name, "plugin-name"));
        title.append(&nyx_ui::widgets::label(&plugin.manifest.version, "plugin-version"));
        text.append(&title);
        if !plugin.manifest.description.is_empty() {
            let desc = nyx_ui::widgets::label(&plugin.manifest.description, "plugin-desc");
            desc.set_xalign(0.);
            desc.set_wrap(true);
            text.append(&desc);
        }
        head.append(&text);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        actions.set_valign(gtk::Align::Start);
        actions.set_margin_end(6);
        if plugin.manifest.capabilities.contains(&"settings".into()) {
            let (app, id) = (app.clone(), plugin.manifest.id.clone());
            let b = nyx_ui::chrome::settings_button(move || plugin_settings(&app, &id));
            b.set_tooltip_text(Some("Plugin settings"));
            actions.append(&b);
        }
        for (icon, tip, op) in [
            ("folder-download-symbolic", "Update", "update"),
            ("edit-undo-symbolic", "Roll back to the previous version", "rollback"),
        ] {
            let b = nyx_ui::widgets::bar_button(icon, tip);
            let (id, st) = (plugin.manifest.id.clone(), note.clone());
            b.connect_clicked(move |_| {
                let id = id.clone();
                run_task(st.clone(), move || {
                    let p = Paths::discover();
                    if op == "update" {
                        plugins::update(&p, &id)?;
                    } else {
                        plugins::rollback(&p, &id)?;
                    }
                    Ok("Plugin updated.".into())
                });
            });
            actions.append(&b);
        }
        let (id, st) = (plugin.manifest.id.clone(), note.clone());
        // Removing deletes the package, so it takes the suite's two clicks.
        actions.append(&armed_button("user-trash-symbolic", "Remove", "Click again to remove", move || {
            let id = id.clone();
            run_task(st.clone(), move || {
                plugins::remove(&Paths::discover(), &id, false)?;
                Ok("Plugin removed.".into())
            });
        }));
        head.append(&actions);
        let switch = gtk::Switch::builder().active(plugin.grant.enabled).valign(gtk::Align::Start).build();
        switch.set_tooltip_text(Some(if plugin.grant.enabled {
            "Turn off"
        } else {
            "Turn on: approves the permissions listed below"
        }));
        let (id, st) = (plugin.manifest.id.clone(), note.clone());
        switch.connect_state_set(move |_, on| {
            let id = id.clone();
            run_task(st.clone(), move || {
                let p = Paths::discover();
                if on {
                    plugins::enable(&p, &id, true)?
                } else {
                    plugins::disable(&p, &id)?
                }
                Ok(if on { "Plugin turned on." } else { "Plugin turned off." }.into())
            });
            glib::Propagation::Proceed
        });
        head.append(&switch);
        card.append(&head);

        // What it may touch, as chips under the description.
        let perms = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        perms.add_css_class("plugin-perms");
        let want = if plugin.grant.enabled { &plugin.grant.permissions } else { &plugin.manifest.permissions };
        let mut chips = vec![if want.network { "Network".to_string() } else { "No network".to_string() }];
        if plugin.manifest.permissions.selected_sources {
            chips.push("Your search folders".into());
        }
        chips.extend(want.read.iter().map(|path| format!("Reads {}", nyx_core::paths::pretty(path))));
        for chip in chips {
            let l = nyx_ui::widgets::label(&chip, "plugin-chip");
            l.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            l.set_max_width_chars(36);
            perms.append(&l);
        }
        card.append(&perms);
        if let Some(error) = plugins::last_error(&p, &plugin.manifest.id) {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            line.add_css_class("plugin-error");
            let icon = gtk::Image::from_icon_name("dialog-warning-symbolic");
            icon.set_valign(gtk::Align::Start);
            line.append(&icon);
            let label = nyx_ui::widgets::label(error.trim_start_matches("plugin: ").trim_matches('"'), "");
            label.set_xalign(0.);
            label.set_wrap(true);
            label.set_tooltip_text(Some("The plugin's last error"));
            line.append(&label);
            card.append(&line);
        }

        body.append(&card);
    }
    body
}
fn browse_page(installed: &[plugins::Plugin], note: &gtk::Label) -> gtk::Box {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.append(&group_label("Catalog"));
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.add_css_class("plugin-card");
    list.add_css_class("plugin-list");
    let catalog = plugins::catalog(&Paths::discover(), false).unwrap_or_default();
    if catalog.is_empty() {
        let empty = nyx_ui::widgets::label("The catalog isn't loaded. Refresh it with the button above.", "plugin-desc");
        empty.set_xalign(0.);
        empty.set_wrap(true);
        list.append(&empty);
    }
    for item in catalog {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.add_css_class("plugin-row");
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        text.append(&nyx_ui::widgets::label(&item.name, "plugin-name"));
        let desc = nyx_ui::widgets::label(&item.description, "plugin-desc");
        desc.set_xalign(0.);
        desc.set_wrap(true);
        text.append(&desc);
        row.append(&text);
        if installed.iter().any(|p| p.manifest.id == item.id) {
            row.append(&nyx_ui::widgets::tag("Installed", ""));
        } else {
            let install = gtk::Button::with_label("Install");
            install.add_css_class("plugin-install");
            install.set_valign(gtk::Align::Center);
            let st = note.clone();
            install.connect_clicked(move |_| {
                let item = item.clone();
                run_task(st.clone(), move || {
                    let p = Paths::discover();
                    if item.id == "quick-tools" {
                        plugins::install_builtin(&p)?;
                    } else {
                        plugins::install_git(&p, &item.repo, &item.revision, &item.folder, false)?;
                    }
                    Ok("Installed and turned off. Review its permissions in Installed.".into())
                });
            });
            row.append(&install);
        }
        list.append(&row);
    }
    body.append(&list);

    body.append(&group_label("From Git"));
    let git = gtk::Box::new(gtk::Orientation::Vertical, 8);
    git.add_css_class("plugin-card");
    let url = gtk::Entry::builder().placeholder_text("HTTPS Git repository").build();
    let revision = gtk::Entry::builder().placeholder_text("Full 40-character commit hash").build();
    let folder = gtk::Entry::builder().placeholder_text("Plugin subfolder (optional)").build();
    git.append(&url);
    let pair = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    revision.set_hexpand(true);
    folder.set_hexpand(true);
    pair.append(&revision);
    pair.append(&folder);
    git.append(&pair);
    let install = gtk::Button::with_label("Install from Git");
    install.add_css_class("plugin-install");
    install.set_halign(gtk::Align::End);
    let st = note.clone();
    install.connect_clicked(move |_| {
        let (u, r, f) = (url.text().to_string(), revision.text().to_string(), folder.text().to_string());
        run_task(st.clone(), move || {
            plugins::install_git(&Paths::discover(), &u, &r, &f, false)?;
            Ok("Installed and turned off. Review its permissions in Installed.".into())
        });
    });
    git.append(&install);
    body.append(&git);
    body
}
fn group_label(text: &str) -> gtk::Label {
    let l = nyx_ui::widgets::label(&text.to_uppercase(), "plugins-group");
    l.set_xalign(0.);
    l
}
/// An icon button that needs two clicks: the first arms it (danger tint, `armed`
/// tooltip) for four seconds, the second acts.
fn armed_button(icon: &str, idle: &str, armed: &str, act: impl Fn() + 'static) -> gtk::Button {
    let b = nyx_ui::widgets::bar_button(icon, idle);
    b.add_css_class("danger-button");
    let (idle, armed) = (idle.to_string(), armed.to_string());
    b.connect_clicked(move |b| {
        if b.has_css_class("armed") {
            b.remove_css_class("armed");
            b.set_tooltip_text(Some(&idle));
            act();
            return;
        }
        b.add_css_class("armed");
        b.set_tooltip_text(Some(&armed));
        let (b2, idle2) = (b.clone(), idle.clone());
        glib::timeout_add_local_once(Duration::from_secs(4), move || {
            b2.remove_css_class("armed");
            b2.set_tooltip_text(Some(&idle2));
        });
    });
    b
}
fn run_task(label: gtk::Label, task: impl FnOnce() -> anyhow::Result<String> + Send + 'static) {
    label.set_text("Working…");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(task().map_err(|e| e.to_string()));
    });
    glib::timeout_add_local(Duration::from_millis(100), move || {
        if let Ok(result) = rx.try_recv() {
            label.set_text(&result.unwrap_or_else(|e| e));
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}
type ValueGetter = Rc<dyn Fn() -> Value>;
fn setting_control(row: &Value) -> Option<(gtk::Widget, ValueGetter)> {
    match row["kind"].as_str().unwrap_or("entry") {
        "entry" => {
            let entry = gtk::Entry::builder().text(row["value"].as_str().unwrap_or("")).build();
            let read = entry.clone();
            Some((entry.upcast(), Rc::new(move || json!(read.text().to_string()))))
        }
        "switch" => {
            let switch = gtk::Switch::builder().active(row["value"].as_bool().unwrap_or(false)).build();
            let read = switch.clone();
            Some((switch.upcast(), Rc::new(move || json!(read.is_active()))))
        }
        "slider" => {
            let (min, max, step) = (row["min"].as_f64()?, row["max"].as_f64()?, row["step"].as_f64().unwrap_or(1.));
            if !min.is_finite() || !max.is_finite() || !step.is_finite() || min >= max || step <= 0. {
                return None;
            }
            let spin = gtk::SpinButton::with_range(min, max, step);
            spin.set_value(row["value"].as_f64().unwrap_or(min).clamp(min, max));
            let read = spin.clone();
            Some((spin.upcast(), Rc::new(move || json!(read.value()))))
        }
        "choice" => {
            let options: Vec<(String, String)> = serde_json::from_value(row["options"].clone()).ok()?;
            if options.is_empty() || options.len() > 128 {
                return None;
            }
            let labels = options.iter().map(|(_, label)| label.as_str()).collect::<Vec<_>>();
            let dropdown = gtk::DropDown::from_strings(&labels);
            dropdown
                .set_selected(options.iter().position(|(id, _)| Some(id.as_str()) == row["value"].as_str()).unwrap_or(0) as u32);
            let read = dropdown.clone();
            Some((
                dropdown.upcast(),
                Rc::new(move || json!(options.get(read.selected() as usize).map(|(id, _)| id).cloned().unwrap_or_default())),
            ))
        }
        _ => None,
    }
}
fn plugin_settings(app: &gtk::Application, id: &str) {
    let win = gtk::ApplicationWindow::builder()
        .application(app)
        .title(format!("Glimpse · {id} settings"))
        .default_width(600)
        .default_height(400)
        .build();
    win.add_css_class("glimpse-window");
    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.add_css_class("glimpse-dialog");
    root.set_margin_top(16);
    root.set_margin_start(16);
    root.set_margin_end(16);
    root.set_margin_bottom(16);
    let status = gtk::Label::new(Some("Loading settings…"));
    status.set_wrap(true);
    root.append(&status);
    close_controls(&win, &root);
    let scroll = gtk::ScrolledWindow::builder()
        .vscrollbar_policy(gtk::PolicyType::External)
        .hscrollbar_policy(gtk::PolicyType::External)
        .child(&root)
        .build();
    win.set_child(Some(&scroll));
    win.present();
    let id = id.to_owned();
    let (tx, rx) = mpsc::channel();
    let worker_id = id.clone();
    std::thread::spawn(move || {
        let _ =
            tx.send(plugins::call(&Paths::discover(), &worker_id, "settings", "settings", json!({})).map_err(|e| e.to_string()));
    });
    glib::timeout_add_local(Duration::from_millis(50), move || {
        if let Ok(reply) = rx.try_recv() {
            match reply {
                Err(e) => status.set_text(&e),
                Ok(value) => {
                    status.set_text("Plugin settings");
                    if let Some(rows) = value["rows"].as_array() {
                        for row in rows.iter().take(32) {
                            let Some(key) = row["key"].as_str() else {
                                continue;
                            };
                            if key.len() > 128 {
                                continue;
                            }
                            let title = row["title"].as_str().unwrap_or(key);
                            let description = row["desc"].as_str().unwrap_or("");
                            let Some((control, read)) = setting_control(row) else {
                                root.append(&gtk::Label::new(Some(title)));
                                continue;
                            };
                            root.append(&nyx_ui::widgets::row(title, description, Some(&control)));
                            let button = gtk::Button::with_label(&format!("Save {title}"));
                            let id = id.clone();
                            let key = key.to_owned();
                            let label = status.clone();
                            button.connect_clicked(move |_| {
                                let value = read();
                                let id = id.clone();
                                let key = key.clone();
                                run_task(label.clone(), move || {
                                    plugins::call(
                                        &Paths::discover(),
                                        &id,
                                        "settings",
                                        "configure",
                                        json!({"key":key,"value":value}),
                                    )?;
                                    Ok("Saved".into())
                                });
                            });
                            root.append(&button);
                        }
                    }
                }
            }
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

fn stop_playback(slot: &gtk::Box, playback: &RefCell<Option<Player>>) {
    if let Some(player) = playback.borrow_mut().take() {
        slot.remove(player.widget());
    }
}
/// The file's type from its name, except that a `.ts` file which isn't an MPEG transport
/// stream is TypeScript, so it neither looks like nor opens as a video.
fn content_type(path: &Path) -> String {
    let typescript = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("ts")) && media_kind(path).is_none();
    if typescript {
        return "application/typescript".into();
    }
    gio::content_type_guess(Some(path), None::<&[u8]>).0.to_string()
}
fn media_kind(path: &Path) -> Option<bool> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if ["mp4", "mkv", "webm", "mov", "avi", "m4v", "mpeg", "mpg", "ogv"].contains(&ext.as_str()) {
        Some(true)
    } else if ext == "ts" {
        // .ts is far more often TypeScript; a transport stream starts with the 0x47 sync byte.
        let mut first = [0u8; 1];
        let is_stream = std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut first)).is_ok() && first[0] == 0x47;
        is_stream.then_some(true)
    } else if ["mp3", "wav", "flac", "ogg", "opus", "m4a", "aac", "aiff", "wma"].contains(&ext.as_str()) {
        Some(false)
    } else {
        None
    }
}
/// Syntax colours drawn from the Nyx palette rather than a fixed editor theme, so code
/// previews match every theme: the accent for structure, dim italics for comments.
fn code_markup(code: &str, path: &Path) -> Option<String> {
    use std::{str::FromStr, sync::OnceLock};
    use syntect::{
        easy::HighlightLines,
        highlighting::{Color, FontStyle, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings},
        parsing::SyntaxSet,
        util::LinesWithEndings,
    };
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    let ss = SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines);
    let syntax = ss.find_syntax_by_extension(path.extension()?.to_str()?)?;
    if syntax.name == "Plain Text" {
        return None;
    }
    let palette = nyx_theme::current_palette();
    let color = |hex: &str| {
        let v = u32::from_str_radix(hex.trim_start_matches('#').get(..6)?, 16).ok()?;
        Some(Color { r: (v >> 16) as u8, g: (v >> 8) as u8, b: v as u8, a: 255 })
    };
    let item = |scope: &str, hex: &str, style: FontStyle| ThemeItem {
        scope: ScopeSelectors::from_str(scope).unwrap_or_default(),
        style: StyleModifier { foreground: color(hex), background: None, font_style: Some(style) },
    };
    let (accent, dim) = (palette.accent.as_str(), palette.dim_text.as_str());
    let theme = Theme {
        settings: ThemeSettings { foreground: color(&palette.text), ..Default::default() },
        scopes: vec![
            item("comment, punctuation.definition.comment, markup.quote", dim, FontStyle::ITALIC),
            item(
                "keyword, storage, constant.numeric, constant.language, entity.name.type, support.type",
                accent,
                FontStyle::empty(),
            ),
            item("markup.heading, entity.name.section, punctuation.definition.heading", accent, FontStyle::BOLD),
            item(
                "markup.raw, markup.inline.raw, markup.underline.link, punctuation.definition.list_item",
                accent,
                FontStyle::empty(),
            ),
            item("entity.name.function, markup.bold", &palette.text, FontStyle::BOLD),
            item("markup.italic", &palette.text, FontStyle::ITALIC),
        ],
        ..Default::default()
    };
    let mut highlighter = HighlightLines::new(syntax, &theme);
    let mut markup = String::new();
    for line in LinesWithEndings::from(code) {
        for (style, token) in highlighter.highlight_line(line, ss).ok()? {
            let c = style.foreground;
            let weight = if style.font_style.contains(FontStyle::BOLD) { " weight=\"bold\"" } else { "" };
            let slant = if style.font_style.contains(FontStyle::ITALIC) { " style=\"italic\"" } else { "" };
            markup.push_str(&format!(
                "<span foreground=\"#{:02x}{:02x}{:02x}\"{weight}{slant}>{}</span>",
                c.r,
                c.g,
                c.b,
                glib::markup_escape_text(token)
            ));
        }
    }
    Some(markup)
}
pub fn dismiss() {
    PANEL.with(|p| {
        if let Some(window) = p.borrow().as_ref() {
            window.set_visible(false);
        }
    });
}

/// Select a result by index and scroll just enough to show it.
fn select_and_reveal(list: &gtk::ListBox, scroll: &gtk::ScrolledWindow, browsing: &std::cell::Cell<bool>, index: i32) {
    let Some(row) = list.row_at_index(index) else { return };
    browsing.set(true);
    list.select_row(Some(&row));
    if let Some(bounds) = row.compute_bounds(list) {
        let adj = scroll.vadjustment();
        let y = bounds.y() as f64;
        let bottom = y + bounds.height() as f64;
        if y < adj.value() {
            adj.set_value(y.max(0.));
        } else if bottom > adj.value() + adj.page_size() {
            adj.set_value((bottom - adj.page_size()).min(adj.upper() - adj.page_size()).max(0.));
        }
    }
}
fn navigate_result(list: &gtk::ListBox, direction: i32) {
    let next = list.selected_row().map(|row| (row.index() + direction).max(0)).unwrap_or(0);
    if let Some(row) = list.row_at_index(next) {
        list.select_row(Some(&row));
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn highlight_ignores_case_and_keeps_text() {
        assert_eq!(super::highlight("The Report & report", "report"), "The <b>Report</b> &amp; <b>report</b>");
        assert_eq!(super::highlight("Ärger ÄRGER", "ärger"), "<b>Ärger</b> <b>ÄRGER</b>");
        assert_eq!(super::highlight("abc", "zz"), "abc");
    }
}

/// Resolve commands off the GTK thread and recheck availability immediately before launch.
fn dispatch_command(
    target: providers::Target,
    result: ResultRow,
    window: gtk::ApplicationWindow,
    status: gtk::Label,
    armed: Rc<RefCell<Option<String>>>,
) {
    let original_query = SEARCH.with(|s| s.borrow().as_ref().map(|e| e.text().to_string()));
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let resolved = (|| -> anyhow::Result<(Vec<String>, bool, String)> {
            match target {
                providers::Target::Omarchy(id) => {
                    let providers::Effect::Command(argv, confirm) =
                        providers::resolve(&Paths::discover(), &providers::Target::Omarchy(id), "execute")?
                    else {
                        unreachable!()
                    };
                    Ok((argv, confirm, String::new()))
                }
                providers::Target::Service(unit) => {
                    if !unit.ends_with(".service") || !unit.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.@:".contains(&b)) {
                        anyhow::bail!("Invalid service");
                    }
                    let mut cmd = Command::new("systemctl");
                    cmd.args([
                        "--user",
                        "show",
                        "--no-pager",
                        "--property=Id,Description,LoadState,ActiveState,SubState",
                        "--",
                        &unit,
                    ]);
                    let output = nyx_glimpse::process::capture(cmd, vec![], Duration::from_secs(2), 16384)?;
                    let text = String::from_utf8_lossy(&output).into_owned();
                    if text.is_empty() {
                        anyhow::bail!("Service unavailable");
                    }
                    Ok((vec![], false, text))
                }
                _ => anyhow::bail!("Unsupported command provider"),
            }
        })();
        let _ = tx.send(resolved);
    });
    let identity = result.identity();
    glib::timeout_add_local(Duration::from_millis(20), move || {
        if !window.is_visible() {
            return glib::ControlFlow::Break;
        }
        let Ok(reply) = rx.try_recv() else {
            return glib::ControlFlow::Continue;
        };
        if SEARCH.with(|s| s.borrow().as_ref().map(|e| e.text().to_string())) != original_query {
            return glib::ControlFlow::Break;
        }
        match reply {
            Err(e) => status.set_text(&e.to_string()),
            Ok((argv, confirm, detail)) => {
                if argv.is_empty() {
                    status.set_text(&detail.lines().collect::<Vec<_>>().join(" · "));
                    return glib::ControlFlow::Break;
                }

                if confirm && armed.borrow().as_deref() != Some(identity.as_str()) {
                    *armed.borrow_mut() = Some(identity.clone());
                    status.set_text("Press Enter again to confirm");
                } else {
                    let launched = Command::new(&argv[0]).args(&argv[1..]).spawn();
                    match launched {
                        Ok(_) => {
                            *armed.borrow_mut() = None;
                            record_selection(&result);
                            window.set_visible(false);
                        }
                        Err(e) => status.set_text(&e.to_string()),
                    }
                }
            }
        }
        glib::ControlFlow::Break
    });
}

#[derive(Clone)]
enum PanelVerb {
    Open,
    Reveal,
    Copy,
    Details,
    Favorite,
    Alias,
    SaveAlias,
    Desktop(String),
    Password(passwords::Verb),
    Clipboard(cb::How),
    Plugin(String),
    History(String),
}
#[derive(Clone)]
struct PanelItem {
    title: String,
    verb: PanelVerb,
}
struct ActionPanel {
    popover: gtk::Popover,
    entry: gtk::SearchEntry,
    list: gtk::ListBox,
    items: RefCell<Vec<PanelItem>>,
    visible: RefCell<Vec<PanelItem>>,
    selected: RefCell<Option<ResultRow>>,
    alias_mode: Cell<bool>,
    details: RefCell<Option<String>>,
    request: Cell<u64>,
}
impl ActionPanel {
    fn new(
        anchor: &gtk::SearchEntry,
        window: &gtk::ApplicationWindow,
        status: &gtk::Label,
        search: &gtk::SearchEntry,
        armed: &Rc<RefCell<Option<String>>>,
    ) -> Rc<Self> {
        let popover = gtk::Popover::new();
        popover.set_parent(anchor);
        popover.set_has_arrow(false);
        popover.set_position(gtk::PositionType::Bottom);
        popover.add_css_class("glimpse-actions");
        let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
        body.set_size_request(330, -1);
        let entry = gtk::SearchEntry::new();
        entry.set_placeholder_text(Some("Search actions…"));
        body.append(&entry);
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.add_css_class("results");
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .max_content_height(300)
            .propagate_natural_height(true)
            .child(&list)
            .build();
        body.append(&scroll);
        popover.set_child(Some(&body));
        let panel = Rc::new(Self {
            popover,
            entry,
            list,
            items: RefCell::new(vec![]),
            visible: RefCell::new(vec![]),
            selected: RefCell::new(None),
            alias_mode: Cell::new(false),
            details: RefCell::new(None),
            request: Cell::new(0),
        });
        let weak = Rc::downgrade(&panel);
        panel.entry.connect_search_changed(move |_| {
            if let Some(p) = weak.upgrade() {
                p.rebuild();
            }
        });
        let weak = Rc::downgrade(&panel);
        let (w, s, q, a) = (window.clone(), status.clone(), search.clone(), armed.clone());
        panel.list.connect_row_activated(move |_, row| {
            if let Some(p) = weak.upgrade() {
                p.run(row.index() as usize, &w, &s, &q, &a);
            }
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&panel);
        let (w, s, q, a) = (window.clone(), status.clone(), search.clone(), armed.clone());
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(p) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            match key {
                gdk::Key::Escape => {
                    p.popover.popdown();
                    q.grab_focus();
                }
                gdk::Key::Up | gdk::Key::Down => {
                    let n = p.visible.borrow().len() as i32;
                    if n > 0 {
                        let i = p.list.selected_row().map(|r| r.index()).unwrap_or(0);
                        let next = (i + if key == gdk::Key::Down { 1 } else { -1 }).rem_euclid(n);
                        p.list.select_row(p.list.row_at_index(next).as_ref());
                    }
                }
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    let i = p.list.selected_row().map(|r| r.index() as usize).unwrap_or(0);
                    p.run(i, &w, &s, &q, &a);
                }
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        panel.popover.add_controller(keys);
        let weak = Rc::downgrade(&panel);
        let q = search.clone();
        panel.popover.connect_closed(move |_| {
            if let Some(p) = weak.upgrade() {
                p.request.set(p.request.get() + 1);
                p.selected.borrow_mut().take();
            }
            q.grab_focus();
        });
        panel
    }
    fn rebuild(&self) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        if let Some(detail) = self.details.borrow().as_ref() {
            let label = gtk::Label::new(Some(detail));
            label.set_wrap(true);
            label.set_selectable(true);
            label.set_max_width_chars(45);
            self.list.append(&label);
            return;
        }
        let query = self.entry.text().to_lowercase();
        let items = if self.alias_mode.get() {
            vec![PanelItem {
                title: if query.trim().is_empty() { "Remove alias".into() } else { format!("Save alias: {}", self.entry.text()) },
                verb: PanelVerb::SaveAlias,
            }]
        } else {
            self.items
                .borrow()
                .iter()
                .filter(|i| query.is_empty() || i.title.to_lowercase().contains(query.as_str()))
                .cloned()
                .collect()
        };
        for item in &items {
            let label = nyx_ui::widgets::label(&item.title, "hint-label");
            label.set_xalign(0.);
            label.set_margin_start(10);
            label.set_margin_end(10);
            label.set_margin_top(8);
            label.set_margin_bottom(8);
            self.list.append(&label);
        }
        *self.visible.borrow_mut() = items;
        self.list.select_row(self.list.row_at_index(0).as_ref());
    }
    fn open(self: &Rc<Self>, result: ResultRow) {
        self.alias_mode.set(false);
        self.details.borrow_mut().take();
        self.request.set(self.request.get() + 1);
        let mut items = vec![PanelItem { title: "Open".into(), verb: PanelVerb::Open }];
        match &result.action {
            Action::App(id) => {
                if let Some(app) = gio_unix::DesktopAppInfo::new(id) {
                    for action in app.list_actions() {
                        let title = app.action_name(&action).to_string();
                        items.push(PanelItem { title, verb: PanelVerb::Desktop(action.to_string()) });
                    }
                }
            }
            Action::File(path, _) => {
                items = providers::actions(&providers::Target::File(path.clone()))
                    .into_iter()
                    .map(|a| PanelItem {
                        title: a.title,
                        verb: match a.id.as_str() {
                            "reveal" => PanelVerb::Reveal,
                            "copy-path" => PanelVerb::Copy,
                            _ => PanelVerb::Open,
                        },
                    })
                    .collect();
            }
            Action::Password(p) => {
                items.clear();
                if let pw::Pw::Item { kind, has_totp, .. } = p {
                    items.push(PanelItem { title: "Copy username".into(), verb: PanelVerb::Password(passwords::Verb::Username) });
                    if kind == "login" {
                        items.push(PanelItem {
                            title: "Copy password".into(),
                            verb: PanelVerb::Password(passwords::Verb::Password),
                        });
                        items.push(PanelItem { title: "Open site".into(), verb: PanelVerb::Password(passwords::Verb::Open) });
                        if *has_totp {
                            items.push(PanelItem {
                                title: "Copy one-time code".into(),
                                verb: PanelVerb::Password(passwords::Verb::Totp),
                            });
                        }
                    }
                } else {
                    items.push(PanelItem { title: "Open in Passwords".into(), verb: PanelVerb::Open });
                }
            }
            Action::Clipboard(c) => {
                items.clear();
                if let cb::Cb::Entry { kind, pinned, .. } = c {
                    items.push(PanelItem { title: "Paste".into(), verb: PanelVerb::Clipboard(cb::How::Paste) });
                    if kind != "image" {
                        items.push(PanelItem { title: "Paste as plain text".into(), verb: PanelVerb::Clipboard(cb::How::PastePlain) });
                    }
                    items.push(PanelItem { title: "Copy".into(), verb: PanelVerb::Clipboard(cb::How::Copy) });
                    let pin = if *pinned { "Unpin" } else { "Pin" };
                    items.push(PanelItem { title: pin.into(), verb: PanelVerb::Clipboard(cb::How::Pin) });
                    items.push(PanelItem { title: "Delete".into(), verb: PanelVerb::Clipboard(cb::How::Delete) });
                    items.push(PanelItem { title: "More actions in Clipboard".into(), verb: PanelVerb::Clipboard(cb::How::Open) });
                }
            }
            Action::Command(providers::Target::Service(_)) => {
                items[0].title = "View status".into();
            }
            Action::System(_) | Action::Command(_) => {
                items[0].title = "Execute".into();
            }
            Action::Copy(_) => items[0].title = "Copy".into(),
            _ => {}
        }
        let id = result.identity();
        let c = Paths::discover().load().unwrap_or_default();
        if learning::eligible_id(&id) {
            items.push(PanelItem {
                title: if c.favorites.contains(&id) { "Remove favorite" } else { "Add favorite" }.into(),
                verb: PanelVerb::Favorite,
            });
            items.push(PanelItem { title: "Set alias…".into(), verb: PanelVerb::Alias });
        }
        items.push(PanelItem { title: "View details".into(), verb: PanelVerb::Details });
        *self.selected.borrow_mut() = Some(result.clone());
        *self.items.borrow_mut() = items;
        self.entry.set_placeholder_text(Some("Search actions…"));
        self.entry.set_text("");
        self.rebuild();
        self.popover.popup();
        self.entry.grab_focus();
        if let Action::Plugin(id, hit, _) = result.action {
            let request = self.request.get();
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(plugins::contextual_actions(&Paths::discover(), &id, &hit));
            });
            let weak = Rc::downgrade(self);
            glib::timeout_add_local(Duration::from_millis(25), move || {
                let Some(p) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                if p.request.get() != request || !p.popover.is_visible() {
                    return glib::ControlFlow::Break;
                }
                match rx.try_recv() {
                    Ok(Ok(actions)) => {
                        for action in actions {
                            p.items.borrow_mut().push(PanelItem { title: action.title, verb: PanelVerb::Plugin(action.id) });
                        }
                        p.rebuild();
                    }
                    Ok(Err(_)) => {}
                    Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                    Err(_) => {}
                }
                glib::ControlFlow::Break
            });
        }
    }
    fn history(&self) {
        self.details.borrow_mut().take();
        self.alias_mode.set(false);
        self.selected.borrow_mut().take();
        *self.items.borrow_mut() = learning::history(&Paths::discover())
            .unwrap_or_default()
            .into_iter()
            .map(|q| PanelItem { title: q.clone(), verb: PanelVerb::History(q) })
            .collect();
        if self.items.borrow().is_empty() {
            return;
        }
        self.entry.set_placeholder_text(Some("Search history…"));
        self.entry.set_text("");
        self.rebuild();
        self.popover.popup();
        self.entry.grab_focus();
    }
    fn run(
        &self,
        index: usize,
        window: &gtk::ApplicationWindow,
        status: &gtk::Label,
        search: &gtk::SearchEntry,
        armed: &Rc<RefCell<Option<String>>>,
    ) {
        let Some(item) = self.visible.borrow().get(index).cloned() else {
            return;
        };
        if let PanelVerb::History(query) = item.verb {
            self.popover.popdown();
            search.set_text(&query);
            search.grab_focus();
            return;
        }
        let Some(result) = self.selected.borrow().clone() else {
            return;
        };
        if matches!(item.verb, PanelVerb::Alias) {
            self.alias_mode.set(true);
            self.entry.set_placeholder_text(Some("Alias (empty removes it)"));
            self.entry.set_text(
                Paths::discover().load().ok().and_then(|c| c.aliases.get(&result.identity()).cloned()).as_deref().unwrap_or(""),
            );
            self.rebuild();
            return;
        }
        if matches!(item.verb, PanelVerb::Details) {
            self.entry.set_text("");
            let detail = if matches!(result.action, Action::Password(_) | Action::Clipboard(_)) {
                format!("{}\n{}", result.title, result.folder)
            } else {
                format!("{}\n{}\n{}", result.title, result.folder, result.identity())
            };
            *self.details.borrow_mut() = Some(detail);
            self.rebuild();
            return;
        }
        let alias = self.entry.text().to_string();
        self.popover.popdown();
        let outcome = (|| -> anyhow::Result<()> {
            match item.verb {
                PanelVerb::Open => {
                    if let Action::Password(p) = &result.action {
                        pw::activate(p, None, window, status);
                    } else if let Action::Clipboard(c) = &result.action {
                        cb::activate(c, cb::How::Paste, window, status);
                    } else {
                        activate(&result, window, status, armed, false);
                    }
                }
                PanelVerb::Reveal => activate(&result, window, status, armed, true),
                PanelVerb::Copy => {
                    if let Action::File(path, _) = &result.action {
                        if !config::allowed(&Paths::discover().load()?, path) {
                            anyhow::bail!("Source permission revoked");
                        }
                        window.clipboard().set_text(&path.display().to_string());
                        status.set_text("Copied path");
                        record_selection(&result);
                    }
                }
                PanelVerb::Favorite => {
                    learning::favorite(&Paths::discover(), &result.identity())?;
                    status.set_text("Favorite updated");
                }
                PanelVerb::SaveAlias => {
                    learning::alias_for_query(&Paths::discover(), &result.identity(), &alias, Some(&search.text()))?;
                    status.set_text("Alias updated");
                }
                PanelVerb::Desktop(action) => {
                    if let Action::App(id) = &result.action {
                        let app = gio_unix::DesktopAppInfo::new(id).ok_or_else(|| anyhow::anyhow!("Application unavailable"))?;
                        if !app.list_actions().iter().any(|a| a == &action) {
                            anyhow::bail!("Desktop action unavailable");
                        }
                        app.launch_action(&action, None::<&gio::AppLaunchContext>);
                        record_selection(&result);
                        window.set_visible(false);
                    }
                }
                PanelVerb::Password(verb) => {
                    if let Action::Password(p) = &result.action {
                        pw::activate(p, Some(verb), window, status);
                    }
                }
                PanelVerb::Clipboard(how) => {
                    if let Action::Clipboard(c) = &result.action {
                        cb::activate(c, how, window, status);
                    }
                }
                PanelVerb::Plugin(action_id) => {
                    if let Action::Plugin(id, hit, _) = &result.action {
                        let (id, hit) = (id.clone(), hit.clone());
                        let (tx, rx) = mpsc::channel();
                        std::thread::spawn(move || {
                            let _ = tx.send(plugins::contextual_activate(&Paths::discover(), &id, &hit, &action_id));
                        });
                        let (window, status, armed, result) = (window.clone(), status.clone(), armed.clone(), result.clone());
                        let original_query = search.text().to_string();
                        glib::timeout_add_local(Duration::from_millis(20), move || {
                            if !window.is_visible()
                                || SEARCH.with(|s| s.borrow().as_ref().map(|e| e.text().to_string())).as_deref()
                                    != Some(original_query.as_str())
                            {
                                return glib::ControlFlow::Break;
                            }
                            match rx.try_recv() {
                                Ok(Ok(action)) => {
                                    if let Action::Plugin(id, hit, _) = &result.action {
                                        let mut next = result.clone();
                                        next.action = Action::Plugin(id.clone(), hit.clone(), action);
                                        activate(&next, &window, &status, &armed, false);
                                    }
                                }
                                Ok(Err(e)) => status.set_text(&e.to_string()),
                                Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                                Err(_) => status.set_text("Plugin action failed"),
                            }
                            glib::ControlFlow::Break
                        });
                    }
                }
                _ => {}
            }
            Ok(())
        })();
        if let Err(e) = outcome {
            status.set_text(&e.to_string());
        }
        search.grab_focus();
    }
}

fn trace_frame(window: &gtk::ApplicationWindow, event: &'static str, start: Instant) {
    if !metrics::enabled() {
        return;
    }
    if let Some(clock) = window.frame_clock() {
        let id = Rc::new(RefCell::new(None));
        let handle = id.clone();
        *id.borrow_mut() = Some(clock.connect_after_paint(move |clock| {
            metrics::timing(event, start.elapsed());
            if let Some(id) = handle.borrow_mut().take() {
                clock.disconnect(id);
            }
        }));
        window.queue_draw();
    }
}

#[cfg(test)]
mod workflow_ui_tests {
    use super::*;
    fn pump_for(duration: Duration) {
        let until = Instant::now() + duration;
        while Instant::now() < until {
            while glib::MainContext::default().pending() {
                glib::MainContext::default().iteration(false);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn descendants(widget: &gtk::Widget) -> Vec<gtk::Widget> {
        let mut all = vec![widget.clone()];
        let mut child = widget.first_child();
        while let Some(w) = child {
            all.extend(descendants(&w));
            child = w.next_sibling();
        }
        all
    }
    #[test]
    #[ignore = "Requires an isolated Wayland output and scratch XDG directories"]
    fn action_panel_alias_and_escape_workflow() {
        assert!(std::env::var("GLIMPSE_MONITOR").is_ok_and(|s| s.starts_with("HEADLESS-")));
        assert!(std::env::var("XDG_CONFIG_HOME").is_ok_and(|s| s.starts_with("/tmp/glimpse-ui-")));
        metrics::start();
        gtk::init().unwrap();
        let app = gtk::Application::builder().application_id("io.github.design_nexus.nyx.GlimpseTest").build();
        app.register(None::<&gio::Cancellable>).unwrap();
        theme();
        present(&app, false, Some("glimpseaudit"));
        pump_for(Duration::from_millis(500));
        let window = app.windows().into_iter().find(|w| w.is_visible()).unwrap().downcast::<gtk::ApplicationWindow>().unwrap();
        gio::prelude::ActionGroupExt::activate_action(&window, "context-actions", None);
        pump_for(Duration::from_millis(80));
        let popover = descendants(window.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Popover>().ok())
            .find(|p| p.is_visible())
            .expect("Actions must open");
        let nodes = descendants(popover.upcast_ref());
        let list = nodes.iter().find_map(|w| w.clone().downcast::<gtk::ListBox>().ok()).unwrap();
        let alias = descendants(list.upcast_ref())
            .iter()
            .find_map(|w| w.clone().downcast::<gtk::Label>().ok().filter(|l| l.text() == "Set alias…"))
            .unwrap();
        let row = alias.parent().unwrap().downcast::<gtk::ListBoxRow>().unwrap();
        list.emit_by_name::<()>("row-activated", &[&row]);
        let entry = nodes.iter().find_map(|w| w.clone().downcast::<gtk::SearchEntry>().ok()).unwrap();
        entry.set_text("audit-alias");
        pump_for(Duration::from_millis(30));
        list.emit_by_name::<()>("row-activated", &[&list.row_at_index(0).unwrap()]);
        pump_for(Duration::from_millis(30));
        assert!(Paths::discover().load().unwrap().aliases.values().any(|a| a == "audit-alias"));
        let search = SEARCH.with(|s| s.borrow().clone().unwrap());
        search.set_text("audit-alias");
        pump_for(Duration::from_millis(150));
        gio::prelude::ActionGroupExt::activate_action(&window, "context-actions", None);
        pump_for(Duration::from_millis(30));
        assert!(popover.is_visible());
        if let Ok(path) = std::env::var("GLIMPSE_TEST_SCREENSHOT") {
            assert!(
                Command::new("grim").args(["-o", &std::env::var("GLIMPSE_MONITOR").unwrap(), &path]).status().unwrap().success()
            );
        }
        entry.set_text("details");
        pump_for(Duration::from_millis(180));
        list.emit_by_name::<()>("row-activated", &[&list.row_at_index(0).unwrap()]);
        pump_for(Duration::from_millis(180));
        assert!(
            descendants(list.upcast_ref())
                .iter()
                .any(|w| w.clone().downcast::<gtk::Label>().is_ok_and(|l| l.text().contains("Glimpseaudit")))
        );
        let controllers = popover.observe_controllers();
        let key = (0..controllers.n_items()).find_map(|i| controllers.item(i).and_downcast::<gtk::EventControllerKey>()).unwrap();
        assert!(key.emit_by_name::<bool>("key-pressed", &[&gdk::Key::Escape, &0u32, &gdk::ModifierType::empty()]));
        pump_for(Duration::from_millis(30));
        assert!(!popover.is_visible());
        assert_eq!(search.text(), "audit-alias");
        println!(
            "UI_RSS {}",
            std::fs::read_to_string("/proc/self/status").unwrap().lines().find(|l| l.starts_with("VmRSS:")).unwrap()
        );
        window.destroy();
    }
}

/// Omarchy's system command, or the session's own tools where Omarchy isn't installed.
fn system_command(cmd: &str) -> Command {
    if plugins::on_path("omarchy") {
        let mut c = Command::new("omarchy");
        c.args(["system", cmd]);
        return c;
    }
    let argv: &[&str] = match cmd {
        "lock" => &["loginctl", "lock-session"],
        "logout" if plugins::on_path("uwsm") => &["uwsm", "stop"],
        "logout" => &["hyprctl", "dispatch", "exit"],
        "reboot" => &["systemctl", "reboot"],
        _ => &["systemctl", "poweroff"],
    };
    let mut c = Command::new(argv[0]);
    c.args(&argv[1..]);
    c
}
