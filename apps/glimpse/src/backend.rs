//! Glimpse's search without GTK, for its Quickshell interface (`quickshell/Glimpse`).
//! `nyx-glimpse-backend` serves it over `$XDG_RUNTIME_DIR/nyx-glimpse/ui.sock`
//! (nyx-ui-link); the GTK panel runs the same providers in its own process.
//!
//! Topics:
//! - `panel`: `{open, serial, keep, query}`, whether the panel shows; `keep` reopens it
//!   with the search it had, `query` fills the search;
//! - `results`: `{generation, query, rows, status, complete, calc}` for the latest query,
//!   sorted into the GTK panel's categories; never an older query's over a newer one;
//! - `config`: `{preview, action_key, favorite_key}`;
//! - `plugins`: the installed plugins, the catalog and the plugin window's note;
//! - `manager`: `{open, serial}`, whether the plugin window shows;
//! - `panel:<app-id>:<topic>`: an app's live state for its mini-app panel,
//!   `{state}` or `{error}`, followed only while a panel subscribes;
//! - `pins`: `{pins: [offer + {monitor}]}`, the pinned panels, in the order pinned, with
//!   whether each app can answer now.
//!
//! Events: `message {text}` for the status line, `rerun {}` to search again.
//!
//! Operations: `Query`, `Activate {id, generation, how, confirmed}` (replies `{done}`,
//! `{ask}`, `{message}`, `{message, rerun}`, `{fill}` for a command that needs more
//! words, or `{message, preview}` when a command's result is in its preview),
//! `Preview`, `Actions`, `ContextActions`,
//! `History`, `CopyAnswer`, `Show {query?}`, `Hide`, `Toggle`, `OpenPlugins`,
//! `ClosePlugins`, `PluginOp`, `PluginSettings`, `PluginConfigure`, `AddFolder`,
//! `OpenSettings`, and for panels `PanelRun {app, action, arguments, confirmed}` (only
//! that app's actions; replies `{result}` or `{ask}`), `PanelOpen {app, arguments}`,
//! `Pin {app, monitor?, at?}` (only an app with one of Glimpse's panels; `monitor` is
//! where Shell Core put it and `at` where it was dragged, null back in the stack, kept
//! for next time) and `Unpin {app}`.
//!
//! Only Glimpse's own programs connect: its Quickshell panel and `nyx-glimpse`
//! (`NYX_GLIMPSE_ALLOW_ANY=1` lets a test copy take any same-user client).
//!
//! Search runs on one worker thread: `pw` and `cb` queries go to Nyx Passwords and Nyx
//! Clipboard alone; others get the fast providers first (apps, file names, file contents
//! by words, utilities, power, and app commands as last seen), then the slower ones
//! (Omarchy, services, plugins, app commands brought up to date and matched by name,
//! contents by meaning). A newer query cancels the rest.

use crate::{
    clipboard, clips,
    commands::{self, Command as AppCommand, Commands, PanelOffer},
    config::{self, Paths},
    index, learning, passwords, plugins,
    preview::{self, Body, Flavor},
    providers::{self, SearchContext, Target},
    query, semantic, utility, vault,
};
use anyhow::{Result, anyhow, bail};
use gtk::{gio, glib, prelude::*};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

/// What a result does when chosen.
#[derive(Clone, Debug)]
pub enum Item {
    Provider(Target),
    /// A file, and the content hit that found it (`Null` for a name match).
    File(PathBuf, Value),
    Copy(String),
    System(&'static str),
    Password(vault::Pw),
    Clipboard(clips::Cb),
    /// An app's action, with the arguments typed so far.
    Command(AppCommand),
    /// An app's mini-app panel.
    Panel(PanelOffer),
    /// Words that start with an app's name but match none of its commands, offered to
    /// Nyx Assistant only if picked.
    Ask(String),
}

/// Files found only by meaning come last.
const RELATED: u8 = 5;

#[derive(Clone, Debug)]
pub struct Row {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub reason: String,
    pub kind: &'static str,
    pub icon: String,
    pub item: Item,
}

impl Row {
    /// The heading it's listed under, and that heading's place (the GTK panel's).
    fn category(&self) -> (u8, &'static str) {
        match &self.item {
            Item::Provider(Target::App(_)) | Item::Panel(_) => (0, "Applications"),
            Item::File(_, hit) if hit["related"] == true => (RELATED, "Related"),
            Item::File(..) | Item::Provider(Target::File(_)) => (1, "Files"),
            Item::Copy(_) => (2, "Utilities"),
            Item::Provider(Target::Plugin(..)) => (4, "Plugins"),
            // Typed as a command, it comes first; matched by the app's name or a word, after files.
            Item::Command(c) if c.explicit => (0, "Commands"),
            Item::Ask(_) => (0, "Commands"),
            Item::Command(_) | Item::Provider(Target::Shared(_)) => (3, "Commands"),
            Item::System(_) | Item::Provider(_) => (3, "System"),
            Item::Password(_) => (0, "Passwords"),
            Item::Clipboard(_) => (0, "Clipboard"),
        }
    }

    /// The status bar's keys for what Enter and the shortcuts do to it.
    fn hints(&self) -> Vec<(&'static [&'static str], &'static str)> {
        let enter: &'static [&'static str] = &["Enter"];
        match &self.item {
            Item::File(path, _) => {
                let mut list = vec![(enter, "Open"), (&["Ctrl", "Enter"][..], "Reveal"), (&["Ctrl", "C"][..], "Copy")];
                if preview::media_kind(path).is_some() {
                    list.push((&["Ctrl", "P"][..], "Play"));
                }
                list
            }
            Item::Provider(Target::App(_)) => vec![(enter, "Launch")],
            Item::Copy(_) => vec![(enter, "Copy")],
            Item::System("lock") => vec![(enter, "Lock")],
            Item::Provider(Target::Plugin(..)) => vec![(enter, "Open")],
            Item::System(_) | Item::Provider(_) => vec![(enter, "Run")],
            Item::Password(p) => p.hints(),
            Item::Clipboard(c) => c.hints(),
            Item::Panel(_) => vec![(enter, "Use"), (&["Ctrl", "Enter"][..], "Open in app"), (&["Ctrl", "P"][..], "Pin")],
            Item::Ask(_) => vec![(enter, "Ask")],
            Item::Command(c) => {
                let run = if !c.missing.is_empty() && c.available {
                    "Continue"
                } else if c.asks {
                    "Run (asks first)"
                } else {
                    "Run"
                };
                vec![(enter, run), (&["Tab"][..], "Complete")]
            }
        }
    }

    fn json(&self, favorites: &[String]) -> Value {
        let (order, heading) = self.category();
        let hints: Vec<Value> = self.hints().into_iter().map(|(keys, label)| json!({"keys": keys, "label": label})).collect();
        let (tooltip, media) = match &self.item {
            Item::File(path, _) => (path.display().to_string(), preview::media_kind(path).is_some()),
            _ => (String::new(), false),
        };
        json!({
            "id": self.id,
            "title": self.title,
            "subtitle": self.subtitle,
            "reason": self.reason,
            "related": self.reason == "Related",
            "kind": self.kind,
            "icon": self.icon,
            "order": order,
            "category": heading,
            "favorite": favorites.contains(&self.id),
            "hints": hints,
            "tooltip": tooltip,
            "media": media,
            "fill": match &self.item {
                Item::Command(c) => c.fill.as_str(),
                _ => "",
            },
            "panel": match &self.item {
                Item::Panel(o) => offer_json(o),
                _ => Value::Null,
            },
        })
    }
}

fn app_icon(id: &str) -> String {
    gio_unix::DesktopAppInfo::new(id)
        .and_then(|a| a.icon())
        .and_then(|i| IconExt::to_string(&i))
        .map(|s| s.to_string())
        .unwrap_or_else(|| "application-x-executable".into())
}

/// The symbolic icon for a file's type, as the GTK panel's rows show it.
fn file_icon(path: &Path) -> String {
    gio::content_type_get_generic_icon_name(&preview::content_type(path))
        .map(|name| format!("{name}-symbolic"))
        .unwrap_or_else(|| "text-x-generic-symbolic".into())
}

fn provider_row(c: providers::Candidate) -> Row {
    let (kind, icon) = match &c.target {
        Target::App(id) => ("app", app_icon(id)),
        Target::File(path) => ("file", file_icon(path)),
        Target::System(_) => ("system", "system-shutdown-symbolic".to_string()),
        Target::Omarchy(_) => ("command", "system-run-symbolic".to_string()),
        Target::Service(_) => ("service", "system-run-symbolic".to_string()),
        Target::Plugin(..) => ("plugin", "application-x-addon-symbolic".to_string()),
        Target::Shared(_) => ("action", "system-run-symbolic".to_string()),
    };
    let item = match c.target {
        Target::File(path) => Item::File(path, Value::Null),
        target => Item::Provider(target),
    };
    Row { id: c.id, title: c.title, subtitle: c.subtitle, reason: String::new(), kind, icon, item }
}

fn command_row(c: AppCommand) -> Row {
    Row {
        id: c.id(),
        title: c.row_title(),
        subtitle: c.subtitle(),
        reason: c.badge(),
        kind: "command",
        icon: c.icon.clone(),
        item: Item::Command(c),
    }
}

fn panel_row(o: PanelOffer) -> Row {
    let subtitle = o.reason.clone().unwrap_or_else(|| format!("{} at a glance", o.app));
    Row { id: format!("panel:{}", o.app_id), title: o.app.clone(), subtitle, reason: String::new(), kind: "panel", icon: o.icon.clone(), item: Item::Panel(o) }
}

/// A query's command rows and, first, the app's panel; when the words start with an
/// app's name and match none of its commands, "Ask Nyx Assistant" (where it's installed).
fn command_rows(commands: &Commands, fresh: bool, q: &str) -> Vec<Row> {
    let catalog = commands.catalog(fresh);
    let mut rows: Vec<Row> = commands.panel(&catalog, q).into_iter().map(panel_row).collect();
    let found: Vec<Row> = commands.rows(&catalog, q, fresh).into_iter().map(command_row).collect();
    let words = nyx_actions::resolve::split_words(q);
    if found.is_empty() && words.len() > 1 && catalog.app(&words[0]).is_some() && on_path("nyx-assistant") {
        let text = q.trim().to_string();
        rows.push(Row {
            id: format!("ask:{text}"),
            title: "Ask Nyx Assistant".into(),
            subtitle: format!("“{text}”: no {} command matches", words[0]),
            reason: String::new(),
            kind: "command",
            icon: "io.github.design_nexus.nyx.Assistant".into(),
            item: Item::Ask(text),
        });
    }
    rows.extend(found);
    rows
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
}

fn file_row(path: PathBuf, hit: &Value) -> Row {
    Row {
        id: format!("file:{}", path.display()),
        title: path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
        subtitle: preview::folder_of(&path),
        reason: preview::match_reason(hit),
        kind: "file",
        icon: file_icon(&path),
        item: Item::File(path, hit.clone()),
    }
}

fn system_row(name: &str, cmd: &'static str) -> Row {
    let icon = match cmd {
        "lock" => "system-lock-screen-symbolic",
        "logout" => "system-log-out-symbolic",
        "reboot" => "system-reboot-symbolic",
        _ => "system-shutdown-symbolic",
    };
    Row {
        id: format!("system:{cmd}"),
        title: name.into(),
        subtitle: String::new(),
        reason: String::new(),
        kind: "system",
        icon: icon.into(),
        item: Item::System(cmd),
    }
}

/// Adds `more` to `found` as the GTK panel merges late results: a file found again by its
/// contents takes the new tag in place (and stays out of Related once listed by name);
/// anything else already listed is skipped.
fn merge(found: &mut Vec<Row>, more: Vec<Row>) {
    for mut row in more {
        match found.iter_mut().find(|r| r.id == row.id) {
            // Brought up to date (the app started, or stopped).
            Some(existing) if matches!(row.item, Item::Command(_) | Item::Panel(_)) => *existing = row,
            Some(existing) => {
                if let Item::File(_, hit) = &mut row.item
                    && !hit.is_null()
                {
                    if existing.category().0 != RELATED {
                        hit["related"] = false.into();
                    }
                    row.reason = preview::match_reason(hit);
                    *existing = row;
                }
            }
            None => found.push(row),
        }
    }
}

/// The installed applications, as the GTK panel lists them.
pub fn applications() -> Vec<(String, String, String)> {
    gio::AppInfo::all()
        .into_iter()
        .filter(|a| a.should_show())
        .filter_map(|a| {
            let id = a.id()?.to_string();
            let keywords = gio_unix::DesktopAppInfo::new(&id)
                .map(|d| d.keywords().iter().map(|k| k.to_string()).collect::<Vec<_>>().join(" "))
                .unwrap_or_default();
            Some((id, a.display_name().to_string(), format!("{} {} {keywords}", a.name(), a.description().unwrap_or_default()).to_lowercase()))
        })
        .collect()
}

pub struct Engine {
    paths: Paths,
    fast: providers::Registry,
    slow: providers::Registry,
    files: providers::Files,
    extensions: providers::Extensions,
    commands: Arc<Commands>,
    session: Option<semantic::Session>,
    db: Option<rusqlite::Connection>,
}

impl Engine {
    pub fn new(paths: Paths, apps: Vec<(String, String, String)>) -> Engine {
        Engine::with_commands(paths, apps, Arc::new(Commands::default()))
    }

    pub fn with_commands(paths: Paths, apps: Vec<(String, String, String)>, commands: Arc<Commands>) -> Engine {
        let mut fast = providers::Registry::default();
        let _ = fast.register(providers::Applications(apps));
        let mut slow = providers::Registry::default();
        let _ = slow.register(crate::omarchy::Omarchy::default());
        let _ = slow.register(providers::DesktopServices);
        let _ = slow.register(commands::Fuzzy(commands.clone()));
        Engine {
            paths,
            fast,
            slow,
            files: providers::Files::default(),
            extensions: providers::Extensions,
            commands,
            session: None,
            db: None,
        }
    }

    /// The fast results for `q`.
    pub fn fast(&mut self, q: &str, cancelled: &dyn Fn() -> bool) -> Result<(Vec<Row>, String)> {
        let p = &self.paths;
        let c = p.load()?;
        let q = q.trim();
        if q.is_empty() {
            return Ok((vec![], String::new()));
        }
        // The vault and the clipboard history: never searched anywhere else.
        if vault_query(q) {
            return Ok((vec![], String::new()));
        }
        if utility::intent(q) != utility::Intent::Search {
            return match utility::answers_local(p, q, &c, cancelled) {
                Ok(answers) => Ok((
                    answers
                        .into_iter()
                        .map(|a| Row {
                            id: format!("copy:{}", a.copy),
                            title: a.title,
                            // Key hints live in the status bar, not in every row.
                            subtitle: a.detail.replace(" · Enter to copy", "").replace("Enter to copy answer", ""),
                            reason: String::new(),
                            kind: "answer",
                            icon: "accessories-calculator-symbolic".into(),
                            item: Item::Copy(a.copy),
                        })
                        .collect(),
                    String::new(),
                )),
                Err(e) => Ok((vec![], e.to_string())),
            };
        }
        let profile = learning::Profile::load(p, &c).unwrap_or_default();
        let ctx = SearchContext { paths: p, config: &c, query: q, cancelled };
        let mut rows = command_rows(&self.commands, false, q);
        let mut apps = 0;
        for (_, candidate) in self.fast.search(&ctx, &profile).0 {
            if matches!(candidate.target, Target::App(_)) {
                apps += 1;
                if apps > 12 {
                    continue;
                }
            }
            rows.push(provider_row(candidate));
        }
        if let Ok(hits) = providers::Provider::discover(&mut self.files, &ctx) {
            merge(&mut rows, hits.into_iter().map(provider_row).collect());
        }
        if self.db.is_none() {
            self.db = index::open(p).ok();
        }
        if let Some(conn) = &self.db
            && let Ok(hits) = index::content_search_in(conn, p, &c, q)
        {
            merge(&mut rows, hits.into_iter().map(|(file, info)| file_row(file.path, &info)).collect());
        }
        // Matched like app names, so a stray letter doesn't offer Reboot or Shut down.
        for (name, cmd) in [("Lock screen", "lock"), ("Log out", "logout"), ("Reboot", "reboot"), ("Shut down", "shutdown")] {
            if query::name_rank(name, q).is_some() {
                rows.push(system_row(name, cmd));
            }
        }
        Ok((rows, String::new()))
    }

    /// The slower results for `q`, to add after the fast ones.
    pub fn slow(&mut self, q: &str, cancelled: &dyn Fn() -> bool) -> Result<(Vec<Row>, String)> {
        let p = &self.paths;
        let c = p.load()?;
        let q = q.trim();
        if q.is_empty() || vault_query(q) || utility::intent(q) != utility::Intent::Search {
            return Ok((vec![], String::new()));
        }
        let profile = learning::Profile::load(p, &c).unwrap_or_default();
        let ctx = SearchContext { paths: p, config: &c, query: q, cancelled };
        let mut rows = command_rows(&self.commands, true, q);
        rows.extend(self.slow.search(&ctx, &profile).0.into_iter().take(30).map(|(_, c)| provider_row(c)));
        if let Ok(hits) = providers::Provider::discover(&mut self.extensions, &ctx) {
            rows.extend(hits.into_iter().map(provider_row));
        }
        let mut status = String::new();
        if !c.semantic {
            self.session = None;
        } else if query::semantic_allowed(q) && !cancelled() {
            let session = match self.session.as_mut() {
                Some(s) => s,
                None => self.session.insert(semantic::Session::start(p)?),
            };
            match session.search(p, q, cancelled) {
                Ok(hits) => {
                    status = session.note().unwrap_or_default().to_string();
                    for hit in hits {
                        if let Some(path) = hit["path"].as_str() {
                            rows.push(file_row(PathBuf::from(path), &hit));
                        }
                    }
                }
                Err(e) => {
                    status = e.to_string();
                    self.session = None;
                }
            }
        }
        Ok((rows, status))
    }

    pub fn idle(&mut self) {
        if let Some(s) = self.session.as_mut() {
            s.release_idle();
        }
    }
}

/// `pw …` and `cb …`: for Nyx Passwords and Nyx Clipboard only.
fn vault_query(q: &str) -> bool {
    passwords::parse(q).is_some() || clipboard::parse(q).is_some()
}

/// Runs `f` to the end on this thread, for the D-Bus clients written for GLib's loop.
fn block_on<F: std::future::Future>(f: F) -> F::Output {
    let ctx = glib::MainContext::new();
    ctx.with_thread_default(|| ctx.block_on(f)).expect("a fresh main context is free")
}

/// The rows for a `pw` or `cb` query, from the service alone.
fn vault_rows(q: &str) -> Option<(Vec<Row>, String)> {
    if let Some(pq) = passwords::parse(q) {
        let (found, status) = block_on(vault::lookup(&pq));
        let rows = found
            .into_iter()
            .map(|(title, subtitle, pw)| Row {
                id: pw.key(),
                title,
                subtitle,
                reason: String::new(),
                kind: "password",
                icon: pw.icon().into(),
                item: Item::Password(pw),
            })
            .collect();
        return Some((rows, status));
    }
    let cq = clipboard::parse(q)?;
    let (found, status) = block_on(clips::lookup(&cq));
    let rows = found
        .into_iter()
        .map(|(title, subtitle, cb)| Row {
            id: cb.key(),
            title,
            subtitle,
            reason: String::new(),
            kind: "clipboard",
            icon: cb.icon().into(),
            item: Item::Clipboard(cb),
        })
        .collect();
    Some((rows, status))
}

/// What choosing a result did.
pub enum Done {
    /// Done; the interface closes.
    Closed,
    /// Asks first: the same result chosen again runs it.
    Ask(String),
    /// Something to show in the status line; the interface stays.
    Show(String),
    /// Shown, and the search runs again (a clipboard entry pinned or deleted).
    Refresh(String),
    /// The vault opened: the panel comes back and searches again.
    Unlocked,
    /// A command needs more words: the search becomes this.
    Fill(String),
    /// A command ran; its result is in its preview, this in the status line.
    Result(String),
    /// The keyboard goes to the panel on show.
    FocusPanel,
}

/// Run `row`'s action as plain Enter (`reveal`: Ctrl+Enter). `confirmed` is true for the
/// second press on a result that asked.
pub fn activate(p: &Paths, row: &Row, confirmed: bool, reveal: bool) -> Result<Done> {
    let c = p.load()?;
    match &row.item {
        Item::Copy(text) => {
            copy(text)?;
            Ok(Done::Closed)
        }
        Item::File(path, hit) => {
            if !config::allowed(&c, path) {
                bail!("Source permission revoked");
            }
            let providers::Effect::OpenFile(path, reveal) =
                providers::resolve(p, &Target::File(path.clone()), if reveal { "reveal" } else { "open" })?
            else {
                bail!("Unsupported result");
            };
            preview::open_file(&path, hit, reveal)?;
            Ok(Done::Closed)
        }
        Item::System(cmd) => {
            if *cmd != "lock" && !confirmed {
                return Ok(Done::Ask("Press Enter again to confirm".into()));
            }
            system_command(cmd).spawn()?;
            Ok(Done::Closed)
        }
        Item::Provider(Target::App(id)) => {
            gio_unix::DesktopAppInfo::new(id)
                .ok_or_else(|| anyhow!("Application unavailable"))?
                .launch(&[], None::<&gio::AppLaunchContext>)?;
            Ok(Done::Closed)
        }
        Item::Provider(Target::Omarchy(id)) => {
            let providers::Effect::Command(argv, confirm) = providers::resolve(p, &Target::Omarchy(id.clone()), "execute")? else {
                bail!("Unsupported command");
            };
            if confirm && !confirmed {
                return Ok(Done::Ask("Press Enter again to confirm".into()));
            }
            Command::new(&argv[0]).args(&argv[1..]).spawn()?;
            Ok(Done::Closed)
        }
        Item::Provider(Target::Service(unit)) => {
            if !unit.ends_with(".service") || !unit.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.@:".contains(&b)) {
                bail!("Invalid service");
            }
            let mut cmd = Command::new("systemctl");
            cmd.args(["--user", "show", "--no-pager", "--property=Id,Description,LoadState,ActiveState,SubState", "--", unit]);
            let output = crate::process::capture(cmd, vec![], Duration::from_secs(2), 16384)?;
            let text = String::from_utf8_lossy(&output).into_owned();
            if text.is_empty() {
                bail!("Service unavailable");
            }
            Ok(Done::Show(text.lines().collect::<Vec<_>>().join(" · ")))
        }
        Item::Provider(Target::Plugin(id, _, action)) => run_plugin_action(p, id, action, reveal),
        // Commands run with the backend's registry (`run_command`).
        Item::Provider(Target::System(_) | Target::File(_) | Target::Shared(_))
        | Item::Password(_)
        | Item::Clipboard(_)
        | Item::Command(_)
        | Item::Panel(_)
        | Item::Ask(_) => {
            bail!("Unsupported result")
        }
    }
}

/// A plugin result's action, after checking the plugin may still do it.
fn run_plugin_action(p: &Paths, id: &str, action: &plugins::Action, reveal: bool) -> Result<Done> {
    let plugin = plugins::get(p, id)?;
    if !plugins::authorized(&plugin) || !plugins::valid_action(p, &plugin, action) {
        bail!("plugin permission revoked");
    }
    let action = match action {
        // Notes, Capture, Mail, Calendar and Reminders run in Glimpse, not the plugin.
        plugins::Action::Activate { token } => {
            if let Some(done) = plugins::notes_host_action(p, id, token) {
                done?;
                return Ok(Done::Closed);
            }
            if let Some(flag) = plugins::capture_host_args(id, token) {
                plugins::spawn_capture(flag)?;
                return Ok(Done::Closed);
            }
            if let Some(done) = crate::pim::host_action(p, id, token) {
                done?;
                return Ok(Done::Closed);
            }
            plugins::activate(p, id, token)?
        }
        other => other.clone(),
    };
    match action {
        plugins::Action::Copy { text } => copy(&text)?,
        plugins::Action::Open { path } => preview::open_file(&path, &Value::Null, reveal)?,
        plugins::Action::Url { url } => gio::AppInfo::launch_default_for_uri(&url, None::<&gio::AppLaunchContext>)?,
        plugins::Action::Activate { .. } => bail!("the plugin answered with another activation"),
    }
    Ok(Done::Closed)
}

fn copy(text: &str) -> Result<()> {
    use std::io::Write;
    let mut child = Command::new("wl-copy").stdin(std::process::Stdio::piped()).spawn()?;
    child.stdin.take().ok_or_else(|| anyhow!("wl-copy has no input"))?.write_all(text.as_bytes())?;
    child.wait()?;
    Ok(())
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

/// Run a command as Glimpse's own: missing words fill the search first; one that asks
/// waits for a second Enter; the app checks the call again and may refuse.
fn run_command(shared: &Shared, c: &AppCommand, confirmed: bool, id: &str) -> Result<Done> {
    if !c.available {
        bail!("{}", c.reason.clone().unwrap_or_else(|| format!("{} isn't available now", c.title)));
    }
    if !c.missing.is_empty() {
        return Ok(Done::Fill(c.fill.clone()));
    }
    match shared.commands.invoke(c, confirmed) {
        Ok(value) => {
            if learning::eligible_id(id) {
                let _ = learning::record(&shared.paths, id, &c.fill);
            }
            if value.is_null() {
                return Ok(Done::Closed);
            }
            let (summary, html, details) = commands::result_view(c, &value);
            if html.is_none() && details.is_empty() {
                return Ok(Done::Show(summary));
            }
            let body = match html {
                Some(html) => json!({"type": "text", "html": html}),
                None => json!({"type": "card", "icon": c.icon, "title": c.row_title(), "desc": summary}),
            };
            let details: Vec<Value> = details.into_iter().map(|(k, v)| json!([k, v])).collect();
            shared.ran.lock().unwrap().insert(id.to_string(), json!({"body": body, "details": details}));
            Ok(Done::Result(summary))
        }
        Err(e) if e.kind == nyx_actions::ErrorKind::ConfirmationRequired => {
            let details: Vec<Value> = c.details().into_iter().map(|(k, v)| json!([k, v])).collect();
            let card = json!({"type": "card", "icon": "dialog-warning-symbolic", "title": format!("{}?", c.row_title()),
                "desc": format!("{} {}. Press Enter again to go ahead; anything else cancels.", c.app, c.risk.label())});
            shared.ran.lock().unwrap().insert(id.to_string(), json!({"body": card, "details": details}));
            Ok(Done::Ask(format!("{}: {} {}. Press Enter again to run it", c.app, c.row_title(), c.risk.label())))
        }
        Err(e) => Err(anyhow!(e.message)),
    }
}

/// A command's preview before it runs: what it does, the arguments, what it still needs.
fn command_preview(c: &AppCommand) -> Value {
    let details: Vec<Value> = c.details().into_iter().map(|(k, v)| json!([k, v])).collect();
    json!({"body": {"type": "card", "icon": c.icon, "title": c.row_title(), "desc": c.description}, "details": details})
}

/// The action panel's entries for a row (Ctrl+K): `(title, verb)`.
fn panel_actions(p: &Paths, row: &Row) -> Vec<(String, String)> {
    let mut items = vec![("Open".to_string(), "open".to_string())];
    match &row.item {
        Item::Provider(Target::App(id)) => {
            if let Some(app) = gio_unix::DesktopAppInfo::new(id) {
                for action in app.list_actions() {
                    items.push((app.action_name(&action).to_string(), format!("desktop:{action}")));
                }
            }
        }
        Item::File(path, _) => {
            items = providers::actions(&Target::File(path.clone()))
                .into_iter()
                .map(|a| {
                    let verb = match a.id.as_str() {
                        "reveal" => "reveal",
                        "copy-path" => "copy_path",
                        _ => "open",
                    };
                    (a.title, verb.to_string())
                })
                .collect();
        }
        Item::Password(pw) => {
            items.clear();
            if let vault::Pw::Item { kind, has_totp, .. } = pw {
                items.push(("Copy username".into(), "pw:username".into()));
                if kind == "login" {
                    items.push(("Copy password".into(), "pw:password".into()));
                    items.push(("Open site".into(), "pw:open".into()));
                    if *has_totp {
                        items.push(("Copy one-time code".into(), "pw:totp".into()));
                    }
                }
            } else {
                items.push(("Open in Passwords".into(), "open".into()));
            }
        }
        Item::Clipboard(cb) => {
            items.clear();
            if let clips::Cb::Entry { kind, pinned, .. } = cb {
                items.push(("Paste".into(), "cb:paste".into()));
                if kind != "image" {
                    items.push(("Paste as plain text".into(), "cb:plain".into()));
                }
                items.push(("Copy".into(), "cb:copy".into()));
                items.push((if *pinned { "Unpin" } else { "Pin" }.into(), "cb:pin".into()));
                items.push(("Delete".into(), "cb:delete".into()));
                items.push(("More actions in Clipboard".into(), "cb:open".into()));
            }
        }
        Item::Provider(Target::Service(_)) => items[0].0 = "View status".into(),
        Item::System(_) | Item::Provider(Target::Omarchy(_) | Target::Shared(_) | Target::System(_)) => items[0].0 = "Execute".into(),
        Item::Copy(_) => items[0].0 = "Copy".into(),
        _ => {}
    }
    if learning::eligible_id(&row.id) {
        let favorite = p.load().is_ok_and(|c| c.favorites.contains(&row.id));
        items.push((if favorite { "Remove favorite" } else { "Add favorite" }.into(), "favorite".into()));
        items.push(("Set alias…".into(), "alias".into()));
    }
    items.push(("View details".into(), "details".into()));
    items
}

/// The preview pane's content for a row, as JSON for the Quickshell panel.
fn preview_json(row: &Row, query: &str, images: &Path) -> Value {
    let card = |icon: &str, title: &str, desc: &str| json!({"type": "card", "icon": icon, "title": title, "desc": desc});
    let details = |d: Vec<(String, String)>| d.into_iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>();
    match &row.item {
        Item::Provider(Target::App(id)) => {
            let app = gio_unix::DesktopAppInfo::new(id);
            let description = app.as_ref().and_then(|a| a.description()).map(|d| d.to_string()).unwrap_or_default();
            // Without the desktop-entry field codes (%U, %f, …).
            let command = app
                .and_then(|a| a.commandline())
                .map(|c| c.to_string_lossy().split_whitespace().filter(|w| !w.starts_with('%')).collect::<Vec<_>>().join(" "))
                .unwrap_or_default();
            let facts = preview::app_facts(id);
            let info: Vec<Value> = [("Command", command.as_str()), ("Source", facts.source), ("Keywords", facts.keywords.as_str())]
                .into_iter()
                .filter(|(_, v)| !v.is_empty())
                .map(|(k, v)| json!([k, v]))
                .collect();
            let kind = if facts.kind.is_empty() { "Application".to_string() } else { facts.kind };
            json!({"body": {"type": "app", "icon": row.icon, "name": row.title, "kind": kind, "desc": description, "chips": facts.categories, "info": info}, "details": []})
        }
        Item::Copy(value) => json!({"body": card(&row.icon, &row.title, &row.subtitle), "details": [["Copies", value]]}),
        Item::System(cmd) => json!({"body": card(&row.icon, "", preview::system_description(cmd)), "details": []}),
        Item::Password(pw) => json!({"body": card(&row.icon, &row.title, &row.subtitle), "details": details(pw.details())}),
        Item::Clipboard(cb) => json!({"body": card(&row.icon, &row.title, &row.subtitle), "details": details(cb.details())}),
        Item::Provider(Target::Plugin(id, result_id, _)) => {
            let shown = preview::plugin_preview(id, result_id, &row.title, &row.subtitle, query, Flavor::Html);
            json!({"body": body_json(shown.body, row, images), "details": details(shown.details)})
        }
        Item::File(path, hit) => {
            let shown = preview::file_preview(path, hit, query, Flavor::Html, true);
            json!({"body": body_json(shown.body, row, images), "details": details(shown.details)})
        }
        Item::Command(c) => command_preview(c),
        Item::Panel(o) => json!({"body": card(&o.icon, &o.app, o.reason.as_deref().unwrap_or("")), "details": []}),
        Item::Ask(text) => json!({"body": card(&row.icon, "Ask Nyx Assistant", &format!("Enter sends “{text}” to Nyx Assistant. Glimpse's commands work without it.")), "details": []}),
        Item::Provider(_) => json!({"body": card(&row.icon, &row.title, &row.subtitle), "details": []}),
    }
}

static IMAGE_COUNT: AtomicU64 = AtomicU64::new(0);

/// A preview body as JSON; an image goes to a file under `images` (the last few kept).
fn body_json(body: Body, row: &Row, images: &Path) -> Value {
    match body {
        Body::Icon => json!({"type": "card", "icon": row.icon, "title": "", "desc": ""}),
        Body::Text(html) => json!({"type": "text", "html": html}),
        Body::Media => {
            let (path, video) = match &row.item {
                Item::File(path, _) => (path.display().to_string(), preview::media_kind(path) == Some(true)),
                _ => (String::new(), false),
            };
            json!({"type": "media", "path": path, "video": video, "icon": row.icon})
        }
        Body::Image(bytes) => {
            let n = IMAGE_COUNT.fetch_add(1, Ordering::Relaxed);
            let _ = std::fs::create_dir_all(images);
            let _ = std::fs::remove_file(images.join(format!("{}.png", n.saturating_sub(8))));
            let file = images.join(format!("{n}.png"));
            match std::fs::write(&file, bytes) {
                Ok(()) => json!({"type": "image", "path": file.display().to_string()}),
                Err(_) => json!({"type": "card", "icon": row.icon, "title": "", "desc": ""}),
            }
        }
    }
}

/// The calculator's display for a calculation, as the GTK panel shows it.
fn calc_display(q: &str, c: &config::Config) -> Option<String> {
    (utility::intent(q) == utility::Intent::Calculation && !vault_query(q)).then(|| {
        utility::answers(q, c).ok().and_then(|a| a.into_iter().next()).map(|a| a.title).unwrap_or_else(|| utility::pretty_math(q))
    })
}

/// The plugin window's lists.
fn plugins_json(p: &Paths, note: &str) -> Value {
    let installed = plugins::list(p).unwrap_or_default();
    let items: Vec<Value> = installed
        .iter()
        .map(|plugin| {
            let want = if plugin.grant.enabled { &plugin.grant.permissions } else { &plugin.manifest.permissions };
            let mut chips = vec![if want.network { "Network".to_string() } else { "No network".to_string() }];
            if plugin.manifest.permissions.selected_sources {
                chips.push("Your search folders".into());
            }
            chips.extend(want.read.iter().map(|path| format!("Reads {}", nyx_core::paths::pretty(path))));
            json!({
                "id": plugin.manifest.id,
                "name": plugin.manifest.name,
                "version": plugin.manifest.version,
                "description": plugin.manifest.description,
                "enabled": plugin.grant.enabled,
                "settings": plugin.manifest.capabilities.contains(&"settings".into()),
                "chips": chips,
                "error": plugins::last_error(p, &plugin.manifest.id)
                    .map(|e| e.trim_start_matches("plugin: ").trim_matches('"').to_string())
                    .unwrap_or_default(),
            })
        })
        .collect();
    let catalog: Vec<Value> = plugins::catalog(p, false)
        .unwrap_or_default()
        .into_iter()
        .map(|item| {
            let have = installed.iter().any(|p| p.manifest.id == item.id);
            json!({"id": item.id, "name": item.name, "description": item.description, "installed": have})
        })
        .collect();
    let note = if note.is_empty() {
        let status = std::fs::read_to_string(p.state.join("catalog-status.txt")).unwrap_or_default();
        if status.trim().is_empty() {
            "Plugins run separately. Review a plugin's permissions before you turn it on.".to_string()
        } else {
            status.trim().to_string()
        }
    } else {
        note.to_string()
    };
    json!({"installed": items, "catalog": catalog, "note": note})
}

/// A folder chosen in the desktop's own chooser (the file chooser portal); `None` when
/// the person cancels.
fn choose_folder(title: &str) -> Result<Option<PathBuf>> {
    let ctx = glib::MainContext::new();
    ctx.with_thread_default(|| -> Result<Option<PathBuf>> {
        let bus = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>)?;
        let sender = bus.unique_name().ok_or_else(|| anyhow!("no bus name"))?.trim_start_matches(':').replace('.', "_");
        let token = format!("glimpse{}", std::process::id() as u64 * 1000 + IMAGE_COUNT.fetch_add(1, Ordering::Relaxed));
        let handle = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
        let main = glib::MainLoop::new(Some(&ctx), false);
        let chosen: std::rc::Rc<std::cell::RefCell<Option<Option<PathBuf>>>> = Default::default();
        let (main_cb, chosen_cb) = (main.clone(), chosen.clone());
        #[allow(deprecated)]
        let subscription = bus.signal_subscribe(
            Some("org.freedesktop.portal.Desktop"),
            Some("org.freedesktop.portal.Request"),
            Some("Response"),
            Some(&handle),
            None,
            gio::DBusSignalFlags::NONE,
            move |_, _, _, _, _, params| {
                let picked = params
                    .get::<(u32, HashMap<String, glib::Variant>)>()
                    .filter(|(code, _)| *code == 0)
                    .and_then(|(_, results)| results.get("uris").and_then(|v| v.get::<Vec<String>>()))
                    .and_then(|uris| uris.into_iter().next())
                    .and_then(|uri| gio::File::for_uri(&uri).path());
                *chosen_cb.borrow_mut() = Some(picked);
                main_cb.quit();
            },
        );
        let options = glib::VariantDict::new(None);
        options.insert("handle_token", &token);
        options.insert("directory", true);
        options.insert("modal", true);
        let call = bus.call_sync(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.FileChooser",
            "OpenFile",
            Some(&("", title, options.end()).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            -1,
            None::<&gio::Cancellable>,
        );
        if let Err(e) = call {
            #[allow(deprecated)]
            bus.signal_unsubscribe(subscription);
            bail!("The folder chooser isn't available: {e}");
        }
        main.run();
        #[allow(deprecated)]
        bus.signal_unsubscribe(subscription);
        let picked = chosen.borrow_mut().take().flatten();
        Ok(picked)
    })
    .map_err(|e| anyhow!("{e}"))?
}

/// Glimpse's own programs: its Quickshell panel (in `qs`) and `nyx-glimpse`. Anything
/// else of the same user could otherwise run commands as Glimpse.
fn glimpse_peer(peer: &nyx_ui_link::Peer) -> bool {
    peer.exe.as_deref().and_then(Path::file_name).is_some_and(|n| {
        let n = n.to_string_lossy();
        let n = n.strip_prefix('.').and_then(|n| n.strip_suffix("-wrapped")).unwrap_or(&n);
        n == "qs" || n.starts_with("quickshell") || n == "nyx-glimpse"
    })
}

/// A panel's topic gained its first viewer or lost its last: follow the app's state
/// while someone looks (`panel:<app-id>:<topic>`), and stop after.
/// A panel offer as the interface reads it.
fn offer_json(o: &PanelOffer) -> Value {
    json!({"id": o.panel, "app": o.app_id, "name": o.app, "icon": o.icon, "query": o.query,
        "topics": o.topics, "size": o.size, "available": o.available, "reason": o.reason})
}

fn panel_watch(shared: &Arc<Shared>, topic: &str, viewers: usize) {
    let Some((app, name)) = topic.strip_prefix("panel:").and_then(|t| t.rsplit_once(':')) else { return };
    let mut live = shared.live.lock().unwrap();
    if viewers == 0 {
        if let Some(task) = live.remove(topic) {
            task.abort();
        }
        if let Some(link) = shared.link() {
            link.publish(topic, Value::Null);
        }
        return;
    }
    // One that ended (the app said no, or went away) starts again for a new viewer.
    if live.get(topic).is_some_and(|t| !t.is_finished()) {
        return;
    }
    let Some(link) = shared.link() else { return };
    link.publish(topic, json!({"loading": true}));
    let (commands, app, name, topic) = (shared.commands.clone(), app.to_string(), name.to_string(), topic.to_string());
    let task = shared.commands.spawn({
        let topic = topic.clone();
        async move {
            let mut follow = match commands.watch(&app, &name).await {
                Ok(w) => w,
                Err(e) => return link.publish(&topic, json!({"error": e.message})),
            };
            while let Some(next) = follow.next().await {
                match next {
                    Ok(state) => link.publish(&topic, json!({"state": state})),
                    Err(e) => return link.publish(&topic, json!({"error": e.message})),
                }
            }
        }
    });
    live.insert(topic, task);
}

/// Whether a Quickshell interface is connected to this backend.
fn interface_connected(link: &nyx_ui_link::Link) -> bool {
    link.peers().iter().any(|peer| {
        peer.exe.as_deref().and_then(Path::file_name).is_some_and(|n| {
            let n = n.to_string_lossy();
            n == "qs" || n.starts_with("quickshell") || n == ".quickshell-wrapped"
        })
    })
}

/// The generation on show, its query and its rows.
type Shown = (u64, String, HashMap<String, Row>);

/// What the handler and the threads it starts share.
struct Shared {
    paths: Paths,
    link: Mutex<Option<Arc<nyx_ui_link::Link>>>,
    generation: AtomicU64,
    rows: Mutex<Shown>,
    panel: Mutex<(bool, u64)>,
    manager: Mutex<(bool, u64)>,
    note: Mutex<String>,
    jobs: Mutex<mpsc::Sender<(u64, String)>>,
    images: PathBuf,
    commands: Arc<Commands>,
    /// Commands' results by row, shown in their preview until the next search.
    ran: Mutex<HashMap<String, Value>>,
    /// Panels' live topics being followed, by link topic.
    live: Mutex<HashMap<String, tokio::task::AbortHandle>>,
}

impl Shared {
    fn link(&self) -> Option<Arc<nyx_ui_link::Link>> {
        self.link.lock().unwrap().clone()
    }
    fn publish_panel(&self, open: bool, keep: bool, query: Option<&str>) {
        let Some(link) = self.link() else { return };
        let mut panel = self.panel.lock().unwrap();
        panel.0 = open;
        panel.1 += 1;
        if open {
            let _ = crate::ai_setup::resume(&self.paths);
        }
        link.publish("panel", json!({"open": open, "serial": panel.1, "keep": keep, "query": query}));
    }
    fn publish_manager(&self, open: bool) {
        let Some(link) = self.link() else { return };
        let mut manager = self.manager.lock().unwrap();
        manager.0 = open;
        manager.1 += 1;
        link.publish("manager", json!({"open": open, "serial": manager.1}));
    }
    fn publish_plugins(&self) {
        if let Some(link) = self.link() {
            link.publish("plugins", plugins_json(&self.paths, &self.note.lock().unwrap()));
        }
    }
    fn publish_config(&self) {
        let c = self.paths.load().unwrap_or_default();
        if let Some(link) = self.link() {
            link.publish("config", json!({"preview": c.preview, "action_key": c.action_key, "favorite_key": c.favorite_key}));
        }
    }
    /// The pinned panels as the catalog has them now; apps that are gone are left out.
    fn publish_pins(&self) {
        let pinned = self.paths.load().unwrap_or_default().pinned;
        let catalog = if pinned.is_empty() { None } else { Some(self.commands.catalog(false)) };
        let pins: Vec<Value> = pinned
            .iter()
            .filter_map(|p| {
                let mut v = offer_json(&self.commands.pinned(catalog.as_ref()?, &p.app)?);
                v["monitor"] = json!(p.monitor);
                v["at"] = json!(p.at);
                Some(v)
            })
            .collect();
        if let Some(link) = self.link() {
            link.publish("pins", json!({ "pins": pins }));
        }
    }
    /// Pin `app`'s panel (or note where it is), or unpin it.
    /// `at`: `Some(None)` back in the stack, `Some(Some(xy))` put there by hand.
    fn pin(&self, app: &str, on: bool, monitor: Option<&str>, at: Option<Option<[i32; 2]>>) -> Result<()> {
        let mut c = self.paths.load()?;
        if on {
            if self.commands.pinned(&self.commands.catalog(false), app).is_none() {
                return Err(anyhow!("{app} has no panel to pin"));
            }
            let full = c.pinned.len() >= config::MAX_PINNED;
            match c.pinned.iter_mut().find(|p| p.app == app) {
                Some(p) => {
                    p.monitor = monitor.unwrap_or(&p.monitor).to_string();
                    if let Some(at) = at {
                        p.at = at;
                    }
                }
                None if full => return Err(anyhow!("{} panels are pinned already", config::MAX_PINNED)),
                None => c.pinned.push(config::Pinned { app: app.to_string(), monitor: monitor.unwrap_or_default().to_string(), at: at.flatten() }),
            }
        } else {
            c.pinned.retain(|p| p.app != app);
        }
        self.paths.save(&c)?;
        self.publish_pins();
        Ok(())
    }
    fn message(&self, text: &str) {
        if let Some(link) = self.link() {
            link.emit("message", json!({"text": text}));
        }
    }
    /// The row `id` of the generation on show, and the query that found it.
    fn row(&self, g: u64, id: &str) -> Option<(Row, String)> {
        let rows = self.rows.lock().unwrap();
        (rows.0 == g).then(|| rows.2.get(id).cloned().map(|row| (row, rows.1.clone()))).flatten()
    }
    /// The search on show, run again (settings or plugins changed).
    fn rerun(&self) {
        let (g, q) = {
            let rows = self.rows.lock().unwrap();
            (rows.0, rows.1.clone())
        };
        if g == self.generation.load(Ordering::SeqCst) && !q.trim().is_empty() && !vault_query(&q) {
            let _ = self.jobs.lock().unwrap().send((g, q));
        }
    }
    /// Runs a plugin-window task off the handler: the note says "Working…", then how it went.
    fn plugin_task(self: &Arc<Self>, task: impl FnOnce(&Paths) -> Result<String> + Send + 'static) {
        *self.note.lock().unwrap() = "Working…".into();
        self.publish_plugins();
        let shared = self.clone();
        std::thread::spawn(move || {
            let text = task(&shared.paths).unwrap_or_else(|e| format!("{e:#}"));
            *shared.note.lock().unwrap() = text;
            shared.publish_plugins();
        });
    }
}

/// Runs `row`'s action for `how`: `enter`, `reveal` (Ctrl+Enter), `alt` (Alt+Enter),
/// `copy` (Ctrl+C), `site` (Ctrl+O), or an action-panel verb.
fn run(shared: &Arc<Shared>, row: &Row, query: &str, how: &str, confirmed: bool) -> Result<Done> {
    let p = &shared.paths;
    let learn = |row: &Row| {
        if learning::eligible_id(&row.id) {
            let _ = learning::record(p, &row.id, query);
        }
    };
    if how == "favorite" {
        learning::favorite(p, &row.id)?;
        return Ok(Done::Show("Favorite updated".into()));
    }
    if let Some(alias) = how.strip_prefix("alias:") {
        learning::alias_for_query(p, &row.id, alias, Some(query))?;
        return Ok(Done::Show("Alias updated".into()));
    }
    match &row.item {
        Item::Command(c) if how == "enter" => run_command(shared, c, confirmed, &row.id),
        Item::Ask(text) if how == "enter" => {
            std::process::Command::new("nyx-assistant")
                .args(["say", text])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()?;
            Ok(Done::Closed)
        }
        Item::Panel(o) if how == "enter" => match &o.reason {
            Some(reason) => bail!("{reason}"),
            None => Ok(Done::FocusPanel),
        },
        Item::Panel(o) if how == "reveal" => {
            shared.commands.open_app(&o.app_id, serde_json::Map::new())?;
            Ok(Done::Closed)
        }
        Item::Provider(Target::Shared(id)) if how == "enter" => {
            let c = shared.commands.plain(id).ok_or_else(|| anyhow!("That action isn't offered any more"))?;
            run_command(shared, &c, confirmed, &row.id)
        }
        Item::Password(pw) => {
            use passwords::Verb;
            let verb = match how {
                "reveal" | "pw:password" => Some(Verb::Password),
                "alt" | "pw:totp" => Some(Verb::Totp),
                "site" | "pw:open" => Some(Verb::Open),
                "copy" | "pw:username" => Some(Verb::Username),
                _ => None,
            };
            // A protected item waits on the system's sign-in dialog: the panel steps aside
            // so the dialog gets the keyboard, and comes back with an error.
            let pending = Arc::new(AtomicBool::new(true));
            let hidden = Arc::new(AtomicBool::new(false));
            {
                let (pending, hidden, shared) = (pending.clone(), hidden.clone(), shared.clone());
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(400));
                    if pending.load(Ordering::SeqCst) {
                        hidden.store(true, Ordering::SeqCst);
                        shared.publish_panel(false, false, None);
                    }
                });
            }
            let ran = block_on(vault::perform(pw, verb));
            pending.store(false, Ordering::SeqCst);
            match ran {
                Ok(vault::Ran::Done) => Ok(Done::Closed),
                Ok(vault::Ran::Unlocked) => {
                    shared.publish_panel(true, true, None);
                    Ok(Done::Unlocked)
                }
                Ok(vault::Ran::Nothing) => Ok(Done::Show(String::new())),
                Err(e) => {
                    if hidden.load(Ordering::SeqCst) {
                        shared.publish_panel(true, true, None);
                    }
                    Err(anyhow!(e))
                }
            }
        }
        Item::Clipboard(cb) => {
            use clips::How;
            let clips::Cb::Entry { id, pinned, .. } = cb else {
                if *cb == clips::Cb::Unavailable {
                    bail!("Install Clipboard with: nyx install clipboard");
                }
                return Ok(Done::Show(String::new()));
            };
            let how = match how {
                "reveal" | "cb:plain" => How::PastePlain,
                "alt" | "copy" | "cb:copy" => How::Copy,
                "cb:pin" => How::Pin,
                "cb:delete" => How::Delete,
                "cb:open" => How::Open,
                _ => How::Paste,
            };
            let hides = clips::hides_first(how);
            if hides {
                shared.publish_panel(false, false, None);
                // The paste goes to the window under the panel once it's gone.
                std::thread::sleep(Duration::from_millis(120));
            }
            match block_on(clips::perform(*id, *pinned, how)) {
                Ok(said) if matches!(how, How::Pin | How::Delete) => Ok(Done::Refresh(said.into())),
                Ok(_) => Ok(Done::Closed),
                Err(e) => {
                    if hides {
                        shared.publish_panel(true, true, None);
                    }
                    Err(anyhow!(e))
                }
            }
        }
        _ if how == "copy" => {
            let text = match &row.item {
                Item::File(path, _) => path.display().to_string(),
                Item::Copy(value) => value.clone(),
                _ => row.title.clone(),
            };
            copy(&text)?;
            Ok(Done::Show("Copied".into()))
        }
        Item::File(path, _) if how == "copy_path" => {
            if !config::allowed(&p.load()?, path) {
                bail!("Source permission revoked");
            }
            copy(&path.display().to_string())?;
            learn(row);
            Ok(Done::Show("Copied path".into()))
        }
        Item::Provider(Target::App(id)) if how.starts_with("desktop:") => {
            let action = &how["desktop:".len()..];
            let app = gio_unix::DesktopAppInfo::new(id).ok_or_else(|| anyhow!("Application unavailable"))?;
            if !app.list_actions().iter().any(|a| a == action) {
                bail!("Desktop action unavailable");
            }
            app.launch_action(action, None::<&gio::AppLaunchContext>);
            learn(row);
            Ok(Done::Closed)
        }
        Item::Provider(Target::Plugin(id, hit, _)) if how.starts_with("context:") => {
            let action = plugins::contextual_activate(p, id, hit, &how["context:".len()..])?;
            let done = run_plugin_action(p, id, &action, false)?;
            learn(row);
            Ok(done)
        }
        _ => {
            let done = activate(p, row, confirmed, how == "reveal")?;
            if matches!(done, Done::Closed) {
                learn(row);
            }
            Ok(done)
        }
    }
}

/// Serve the interface until the process ends.
pub fn serve(p: Paths) -> Result<()> {
    let (jobs, job_rx) = mpsc::channel::<(u64, String)>();
    let images = nyx_ui_link::socket_path("nyx-glimpse").parent().map(|d| d.join("previews")).unwrap_or_else(std::env::temp_dir);
    let _ = std::fs::remove_dir_all(&images);
    let shared = Arc::new(Shared {
        paths: p.clone(),
        link: Mutex::new(None),
        generation: AtomicU64::new(0),
        rows: Mutex::default(),
        panel: Mutex::new((false, 0)),
        manager: Mutex::new((false, 0)),
        note: Mutex::default(),
        jobs: Mutex::new(jobs),
        images,
        commands: Arc::new(Commands::default()),
        ran: Mutex::default(),
        live: Mutex::default(),
    });

    let mut options = nyx_ui_link::Options::new("nyx-glimpse");
    if !std::env::var("NYX_GLIMPSE_ALLOW_ANY").is_ok_and(|v| v == "1") {
        options.allow = Some(Arc::new(glimpse_peer));
    }
    let watching = shared.clone();
    options.on_watch = Some(Arc::new(move |topic, viewers| panel_watch(&watching, topic, viewers)));
    let handler = shared.clone();
    let link = Arc::new(
        nyx_ui_link::serve(options, move |req| {
            let shared = handler.clone();
            let Some(link) = shared.link() else {
                return req.err("starting", "Glimpse is starting");
            };
            let b = req.body.clone();
            let generation = b["generation"].as_u64().unwrap_or(0);
            let id = b["id"].as_str().unwrap_or("").to_string();
            let op = req.op.clone();
            match op.as_str() {
                "Show" => {
                    if !interface_connected(&link) {
                        return req.err("no_interface", "Glimpse's Quickshell interface isn't running");
                    }
                    shared.publish_panel(true, false, b["query"].as_str());
                    req.ok(json!({}))
                }
                "Hide" => {
                    if !interface_connected(&link) {
                        return req.err("no_interface", "Glimpse's Quickshell interface isn't running");
                    }
                    shared.publish_panel(false, false, None);
                    req.ok(json!({}))
                }
                "Toggle" => {
                    if !interface_connected(&link) {
                        return req.err("no_interface", "Glimpse's Quickshell interface isn't running");
                    }
                    let open = !shared.panel.lock().unwrap().0;
                    shared.publish_panel(open, false, None);
                    req.ok(json!({"open": open}))
                }
                "Query" => {
                    let text = b["text"].as_str().unwrap_or("").to_string();
                    shared.generation.store(generation, Ordering::SeqCst);
                    shared.ran.lock().unwrap().clear();
                    let _ = shared.jobs.lock().unwrap().send((generation, text));
                    req.ok(json!({}))
                }
                "Activate" => {
                    let how = b["how"].as_str().unwrap_or("enter").to_string();
                    let confirmed = b["confirmed"].as_bool().unwrap_or(false);
                    let Some((row, query)) = shared.row(generation, &id) else {
                        return req.err("stale", "That result is gone; search again");
                    };
                    std::thread::spawn(move || match run(&shared, &row, &query, &how, confirmed) {
                        Ok(Done::Closed) => req.ok(json!({"done": true})),
                        Ok(Done::Ask(message)) => req.ok(json!({"ask": message, "preview": true})),
                        Ok(Done::Show(message)) => req.ok(json!({"message": message})),
                        Ok(Done::Refresh(message)) => req.ok(json!({"message": message, "rerun": true})),
                        Ok(Done::Unlocked) => req.ok(json!({"rerun": true})),
                        Ok(Done::Fill(text)) => req.ok(json!({"fill": text})),
                        Ok(Done::Result(message)) => req.ok(json!({"message": message, "preview": true})),
                        Ok(Done::FocusPanel) => req.ok(json!({"focus": true})),
                        Err(e) => req.err("failed", e.to_string()),
                    });
                }
                "Preview" => {
                    let Some((row, query)) = shared.row(generation, &id) else {
                        return req.err("stale", "That result is gone; search again");
                    };
                    if let Some(ran) = shared.ran.lock().unwrap().get(&row.id).cloned() {
                        return req.ok(ran);
                    }
                    std::thread::spawn(move || {
                        let preview = match &row.item {
                            Item::Command(c) => command_preview(c),
                            Item::Provider(Target::Shared(id)) => match shared.commands.plain(id) {
                                Some(c) => command_preview(&c),
                                None => preview_json(&row, &query, &shared.images),
                            },
                            _ => preview_json(&row, &query, &shared.images),
                        };
                        req.ok(preview)
                    });
                }
                "Actions" => {
                    let Some((row, _)) = shared.row(generation, &id) else {
                        return req.err("stale", "That result is gone; search again");
                    };
                    let items: Vec<Value> =
                        panel_actions(&shared.paths, &row).into_iter().map(|(title, verb)| json!({"title": title, "verb": verb})).collect();
                    let alias = shared.paths.load().ok().and_then(|c| c.aliases.get(&row.id).cloned()).unwrap_or_default();
                    let details = if matches!(row.item, Item::Password(_) | Item::Clipboard(_)) {
                        format!("{}\n{}", row.title, row.subtitle)
                    } else {
                        format!("{}\n{}\n{}", row.title, row.subtitle, row.id)
                    };
                    req.ok(json!({"items": items, "alias": alias, "details": details}))
                }
                "ContextActions" => {
                    let Some((row, _)) = shared.row(generation, &id) else {
                        return req.ok(json!({"items": []}));
                    };
                    let Item::Provider(Target::Plugin(plugin, hit, _)) = row.item else {
                        return req.ok(json!({"items": []}));
                    };
                    std::thread::spawn(move || {
                        let items: Vec<Value> = plugins::contextual_actions(&shared.paths, &plugin, &hit)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|a| json!({"title": a.title, "verb": format!("context:{}", a.id)}))
                            .collect();
                        req.ok(json!({"items": items}))
                    });
                }
                "History" => req.ok(json!({"items": learning::history(&shared.paths).unwrap_or_default()})),
                "CopyAnswer" => {
                    let text = b["text"].as_str().unwrap_or("");
                    let c = shared.paths.load().unwrap_or_default();
                    match utility::answers(text, &c).ok().and_then(|a| a.into_iter().next()) {
                        Some(answer) => match copy(&answer.copy) {
                            Ok(()) => req.ok(json!({"message": "Copied"})),
                            Err(e) => req.err("failed", e.to_string()),
                        },
                        None => req.ok(json!({})),
                    }
                }
                "OpenSettings" => {
                    shared.publish_panel(false, false, None);
                    match Command::new("nyx-settings").args(["--section", "glimpse"]).spawn() {
                        Ok(_) => req.ok(json!({})),
                        Err(e) => req.err("failed", e.to_string()),
                    }
                }
                "OpenPlugins" => {
                    if !interface_connected(&link) {
                        return req.err("no_interface", "Glimpse's Quickshell interface isn't running");
                    }
                    shared.publish_panel(false, false, None);
                    shared.publish_manager(true);
                    req.ok(json!({}))
                }
                "ClosePlugins" => {
                    shared.publish_manager(false);
                    req.ok(json!({}))
                }
                "AddFolder" => {
                    if !interface_connected(&link) {
                        return req.err("no_interface", "Glimpse's Quickshell interface isn't running");
                    }
                    shared.publish_panel(false, false, None);
                    req.ok(json!({}));
                    std::thread::spawn(move || match choose_folder("Choose a folder for Glimpse") {
                        Ok(Some(path)) => {
                            let said = crate::cli::dispatch(&["sources".into(), "add".into(), path.display().to_string()])
                                .map(|_| "Folder added. Background indexing is enabled.".to_string())
                                .unwrap_or_else(|e| format!("{e:#}"));
                            shared.publish_panel(true, true, None);
                            shared.message(&said);
                        }
                        Ok(None) => {}
                        Err(e) => {
                            shared.publish_panel(true, true, None);
                            shared.message(&format!("{e:#}"));
                        }
                    });
                }
                "PluginOp" => {
                    if !interface_connected(&link) {
                        return req.err("no_interface", "Glimpse's Quickshell interface isn't running");
                    }
                    let task = b["op"].as_str().unwrap_or("").to_string();
                    let plugin = b["id"].as_str().unwrap_or("").to_string();
                    match task.as_str() {
                        "update" => shared.plugin_task(move |p| {
                            plugins::update(p, &plugin)?;
                            Ok("Plugin updated.".into())
                        }),
                        "rollback" => shared.plugin_task(move |p| {
                            plugins::rollback(p, &plugin)?;
                            Ok("Plugin updated.".into())
                        }),
                        "remove" => shared.plugin_task(move |p| {
                            plugins::remove(p, &plugin, false)?;
                            Ok("Plugin removed.".into())
                        }),
                        "enable" => {
                            let on = b["on"].as_bool().unwrap_or(false);
                            shared.plugin_task(move |p| {
                                if on {
                                    plugins::enable(p, &plugin, true)?
                                } else {
                                    plugins::disable(p, &plugin)?
                                }
                                Ok(if on { "Plugin turned on." } else { "Plugin turned off." }.into())
                            })
                        }
                        "install" => shared.plugin_task(move |p| {
                            let item = plugins::catalog(p, false)?
                                .into_iter()
                                .find(|i| i.id == plugin)
                                .ok_or_else(|| anyhow!("That plugin isn't in the catalog"))?;
                            if item.id == "quick-tools" {
                                plugins::install_builtin(p)?;
                            } else {
                                plugins::install_git(p, &item.repo, &item.revision, &item.folder, false)?;
                            }
                            Ok("Installed and turned off. Review its permissions in Installed.".into())
                        }),
                        "install_git" => {
                            let (url, revision, folder) = (
                                b["url"].as_str().unwrap_or("").to_string(),
                                b["revision"].as_str().unwrap_or("").to_string(),
                                b["folder"].as_str().unwrap_or("").to_string(),
                            );
                            shared.plugin_task(move |p| {
                                plugins::install_git(p, &url, &revision, &folder, false)?;
                                Ok("Installed and turned off. Review its permissions in Installed.".into())
                            })
                        }
                        "refresh" => shared.plugin_task(|p| {
                            plugins::catalog(p, true)?;
                            Ok(std::fs::read_to_string(p.state.join("catalog-status.txt"))?.trim().to_string())
                        }),
                        "add_local" => {
                            shared.publish_manager(false);
                            let after = shared.clone();
                            shared.plugin_task(move |p| {
                                let picked = choose_folder("Choose a plugin folder");
                                after.publish_manager(true);
                                let Some(path) = picked? else {
                                    return Ok(String::new());
                                };
                                plugins::install_local(p, &path, &format!("local:{}", path.display()), "", false)?;
                                Ok("Plugin installed, disabled. Review its permissions in Plugins.".into())
                            })
                        }
                        other => return req.err("unknown_op", format!("no plugin operation {other}")),
                    }
                    req.ok(json!({}))
                }
                "PluginSettings" => {
                    std::thread::spawn(move || match plugins::call(&shared.paths, &id, "settings", "settings", json!({})) {
                        Ok(value) => req.ok(value),
                        Err(e) => req.err("failed", format!("{e:#}")),
                    });
                }
                "PluginConfigure" => {
                    let (key, value) = (b["key"].as_str().unwrap_or("").to_string(), b["value"].clone());
                    std::thread::spawn(move || {
                        match plugins::call(&shared.paths, &id, "settings", "configure", json!({"key": key, "value": value})) {
                            Ok(_) => req.ok(json!({"message": "Saved"})),
                            Err(e) => req.err("failed", format!("{e:#}")),
                        }
                    });
                }
                "PanelRun" => {
                    let app = b["app"].as_str().unwrap_or("").to_string();
                    let action = b["action"].as_str().unwrap_or("").to_string();
                    let arguments = b["arguments"].as_object().cloned().unwrap_or_default();
                    let confirmed = b["confirmed"].as_bool().unwrap_or(false);
                    std::thread::spawn(move || match shared.commands.run_for_panel(&app, &action, arguments, confirmed) {
                        Ok((_, value)) => req.ok(json!({"result": value})),
                        Err(e) if e.kind == nyx_actions::ErrorKind::ConfirmationRequired => req.ok(json!({"ask": e.message})),
                        Err(e) => req.err(e.kind.as_str(), e.message),
                    });
                }
                "Pin" | "Unpin" => {
                    let on = op == "Pin";
                    let app = b["app"].as_str().unwrap_or("").to_string();
                    let monitor = b["monitor"].as_str().map(str::to_string);
                    let at = b.get("at").map(|a| serde_json::from_value::<[i32; 2]>(a.clone()).ok());
                    std::thread::spawn(move || match shared.pin(&app, on, monitor.as_deref(), at) {
                        Ok(()) => req.ok(json!({})),
                        Err(e) => req.err("failed", format!("{e:#}")),
                    });
                }
                "PanelOpen" => {
                    let app = b["app"].as_str().unwrap_or("").to_string();
                    let arguments = b["arguments"].as_object().cloned().unwrap_or_default();
                    std::thread::spawn(move || match shared.commands.open_app(&app, arguments) {
                        Ok(()) => {
                            shared.publish_panel(false, false, None);
                            req.ok(json!({}))
                        }
                        Err(e) => req.err("failed", format!("{e:#}")),
                    });
                }
                other => req.err("unknown_op", format!("no operation {other}")),
            }
        })
        .map_err(|e| anyhow!("{e}"))?,
    );
    *shared.link.lock().unwrap() = Some(link.clone());
    link.publish("panel", json!({"open": false, "serial": 0, "keep": false, "query": null}));
    link.publish("manager", json!({"open": false, "serial": 0}));
    link.publish("results", json!({"generation": 0, "query": "", "rows": [], "status": "", "complete": true, "calc": null}));
    shared.publish_config();
    shared.publish_plugins();
    link.publish("pins", json!({"pins": []}));
    watch_settings(shared.clone());
    // Pinned panels: now, then every 15 s while there are any, so one whose app opens
    // or closes follows it.
    let pins = shared.clone();
    std::thread::spawn(move || {
        loop {
            pins.publish_pins();
            std::thread::sleep(Duration::from_secs(15));
        }
    });
    // Which apps run, asked once now, so the first search already knows.
    let warm = shared.commands.clone();
    std::thread::spawn(move || drop(warm.catalog(true)));

    let mut engine = Engine::with_commands(p.clone(), applications(), shared.commands.clone());
    // The slower providers (meaning search through AI Core, plugins, Omarchy, services)
    // run on a thread of their own, as in the GTK panel: a search waiting for AI Core to
    // load its model never holds up the next key's results.
    let (slow_jobs, slow_rx) = mpsc::channel::<(u64, String, Vec<Row>, String)>();
    {
        let (shared, link, p) = (shared.clone(), link.clone(), p.clone());
        std::thread::spawn(move || {
            let mut engine = Engine::with_commands(p.clone(), vec![], shared.commands.clone());
            loop {
                let (mut g, mut q, mut found, mut status) = match slow_rx.recv_timeout(Duration::from_secs(1)) {
                    Ok(job) => job,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        engine.idle();
                        continue;
                    }
                    Err(_) => return,
                };
                // Typing goes on: only the search still in the field is worth this.
                std::thread::sleep(Duration::from_millis(150));
                while let Ok(next) = slow_rx.try_recv() {
                    (g, q, found, status) = next;
                }
                let cancelled = || shared.generation.load(Ordering::SeqCst) != g;
                if cancelled() {
                    continue;
                }
                let (more, slow_status) = engine.slow(&q, &cancelled).unwrap_or_else(|e| (vec![], e.to_string()));
                merge(&mut found, more);
                publish_results(&shared, &link, &p, g, &q, &found, if slow_status.is_empty() { &status } else { &slow_status }, true);
            }
        });
    }
    loop {
        let (mut g, mut q) = match job_rx.recv() {
            Ok(job) => job,
            Err(_) => return Ok(()),
        };
        std::thread::sleep(Duration::from_millis(10));
        while let Ok((ng, nq)) = job_rx.try_recv() {
            (g, q) = (ng, nq);
        }
        let cancelled = || shared.generation.load(Ordering::SeqCst) != g;
        if cancelled() {
            continue;
        }
        if let Some((rows, status)) = vault_rows(q.trim()) {
            publish_results(&shared, &link, &p, g, &q, &rows, &status, true);
            continue;
        }
        if utility::currency_query(q.trim()).is_some() {
            publish_results(&shared, &link, &p, g, &q, &[], "Getting daily currency rates…", false);
        }
        let (found, status) = engine.fast(&q, &cancelled).unwrap_or_else(|e| (vec![], e.to_string()));
        publish_results(&shared, &link, &p, g, &q, &found, &status, false);
        let _ = slow_jobs.send((g, q, found, status));
    }
}

/// Sends the rows for generation `g`, sorted into their categories, unless a newer search
/// has started since.
#[allow(clippy::too_many_arguments)]
fn publish_results(shared: &Shared, link: &nyx_ui_link::Link, p: &Paths, g: u64, q: &str, found: &[Row], status: &str, complete: bool) {
    if shared.generation.load(Ordering::SeqCst) != g {
        return;
    }
    let c = p.load().unwrap_or_default();
    let mut sorted = found.to_vec();
    sorted.sort_by_key(|r| r.category().0);
    let rows: Vec<Value> = sorted.iter().map(|r| r.json(&c.favorites)).collect();
    *shared.rows.lock().unwrap() = (g, q.to_string(), sorted.into_iter().map(|r| (r.id.clone(), r)).collect());
    link.publish(
        "results",
        json!({"generation": g, "query": q, "rows": rows, "status": status, "complete": complete, "calc": calc_display(q, &c)}),
    );
}

/// Settings, plugins and the catalog changing: the config and plugin topics are sent
/// again and the search on show runs again, as the GTK panel refreshes.
fn watch_settings(shared: Arc<Shared>) {
    std::thread::spawn(move || {
        use notify::Watcher;
        let p = shared.paths.clone();
        let _ = p.ensure();
        let (tx, rx) = mpsc::channel();
        let Ok(mut watcher) = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if let Ok(event) = event
                && matches!(event.kind, notify::EventKind::Create(_) | notify::EventKind::Modify(_) | notify::EventKind::Remove(_))
                && event.paths.iter().any(|path| {
                    path.file_name().is_some_and(|name| {
                        ["settings.toml", "plugins.json", "plugin-catalog.json", "catalog-status.txt"].iter().any(|s| name == *s)
                    })
                })
            {
                let _ = tx.send(());
            }
        }) else {
            return;
        };
        for dir in [&p.config, &p.state, &p.data] {
            let _ = watcher.watch(dir, notify::RecursiveMode::NonRecursive);
        }
        while rx.recv().is_ok() {
            // A save writes more than once: settle, then send once.
            std::thread::sleep(Duration::from_millis(150));
            while rx.try_recv().is_ok() {}
            shared.publish_config();
            shared.publish_plugins();
            shared.publish_pins();
            shared.rerun();
        }
    });
}

/// `nyx-glimpse --toggle`, `--hide`, `--query`, `--plugins` and `--add-folder` go to the
/// Quickshell interface when it runs; `Err` means it doesn't, and the GTK panel answers.
pub fn ask_interface(op: &str, body: Value) -> Result<()> {
    if std::env::var("GLIMPSE_UI").is_ok_and(|v| v == "gtk") {
        bail!("GTK panel requested");
    }
    let path = nyx_ui_link::socket_path("nyx-glimpse");
    if !path.exists() {
        bail!("no backend");
    }
    nyx_ui_link::call(&path, op, body).map(|_| ()).map_err(|e| anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_results_need_no_ai_and_no_display() {
        let dir = tempfile::Builder::new().prefix("glimpse-test-").tempdir().unwrap();
        let p = Paths { config: dir.path().join("c"), data: dir.path().join("d"), state: dir.path().join("s") };
        p.ensure().unwrap();
        let root = dir.path().join("files");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("quarterly-report.txt"), "numbers").unwrap();
        let c = crate::config::Config { roots: vec![root.clone()], ..Default::default() };
        p.save(&c).unwrap();
        index::scan(&p, &c).unwrap();
        let apps = vec![("org.example.Reports.desktop".to_string(), "Reports".to_string(), "reports viewer".to_string())];
        let mut engine = Engine::new(p.clone(), apps);
        let (rows, status) = engine.fast("report", &|| false).unwrap();
        assert!(status.is_empty());
        assert!(rows.iter().any(|r| r.kind == "app" && r.title == "Reports"), "{rows:?}");
        assert!(rows.iter().any(|r| r.kind == "file" && r.title == "quarterly-report.txt"), "{rows:?}");
        assert_eq!(rows.iter().filter(|r| r.title == "quarterly-report.txt").count(), 1, "listed once: {rows:?}");
        for q in ["pw report", "cb report"] {
            assert!(engine.fast(q, &|| false).unwrap().0.is_empty(), "{q} searches nothing else");
            assert!(engine.slow(q, &|| false).unwrap().0.is_empty(), "{q} searches nothing else");
        }
        let (rows, _) = engine.fast("2+2", &|| false).unwrap();
        assert_eq!(rows[0].kind, "answer");
        assert_eq!(calc_display("2+2", &c).as_deref(), Some("2 + 2 = 4"));
        let reboot = engine.fast("reboot", &|| false).unwrap().0.into_iter().find(|r| r.id == "system:reboot").unwrap();
        assert!(matches!(activate(&p, &reboot, false, false).unwrap(), Done::Ask(_)), "reboot asks first");
        assert!(engine.fast("report", &|| true).is_ok());
    }

    #[test]
    fn app_commands_come_first_when_typed_as_commands() {
        let dir = tempfile::Builder::new().prefix("glimpse-test-").tempdir().unwrap();
        let p = Paths { config: dir.path().join("c"), data: dir.path().join("d"), state: dir.path().join("s") };
        p.ensure().unwrap();
        let actions = dir.path().join("actions");
        std::fs::create_dir(&actions).unwrap();
        let manifest = json!({"tools": [{"name": "notes.create", "description": "Create a note.", "inputSchema": {"type": "object", "properties": {"title": {"type": "string"}}, "required": ["title"]},
            "annotations": {"title": "Create a note"}, "x-nyx": {"risk": "writes", "positional": ["title"]}}],
            "nyx": {"api": 2, "app": "io.github.design_nexus.nyx.Notes", "name": "Notes", "namespaces": ["notes"], "bus_name": "io.github.design_nexus.nyx.Notes", "activation": "dbus"}});
        std::fs::write(actions.join("io.github.design_nexus.nyx.Notes.json"), manifest.to_string()).unwrap();
        let commands = Arc::new(Commands::new(nyx_actions::Options { dirs: vec![actions], bus_address: None, live: false }));
        let apps = vec![("io.github.design_nexus.nyx.Notes.desktop".to_string(), "Notes".to_string(), "notes".to_string())];
        let mut engine = Engine::with_commands(p.clone(), apps, commands);
        let sorted = |mut rows: Vec<Row>| {
            rows.sort_by_key(|r| r.category().0);
            rows
        };
        let typed = sorted(engine.fast("notes create Project ideas", &|| false).unwrap().0);
        assert_eq!((typed[0].kind, typed[0].title.as_str()), ("command", "Create a note · Project ideas"));
        assert_eq!(typed[0].json(&[])["hints"][0]["label"], "Run");
        let named = sorted(engine.fast("notes", &|| false).unwrap().0);
        assert_eq!(named[0].kind, "app", "the app's name alone finds the app first: {named:?}");
        let create = named.iter().find(|r| r.kind == "command").unwrap();
        assert_eq!((create.reason.as_str(), create.json(&[])["fill"].as_str()), ("Needs TITLE", Some("notes create ")));
        assert!(engine.fast("pw notes", &|| false).unwrap().0.is_empty(), "pw stays the vault's alone");
    }

    #[test]
    fn only_glimpse_s_own_programs_connect() {
        let peer = |exe: Option<&str>| nyx_ui_link::Peer { id: 1, pid: 1, uid: 1000, exe: exe.map(PathBuf::from) };
        for ok in ["/usr/bin/qs", "/usr/bin/quickshell", "/nix/store/x-quickshell/bin/.quickshell-wrapped", "/home/u/.local/bin/nyx-glimpse", "/nix/store/y/bin/.nyx-glimpse-wrapped"] {
            assert!(glimpse_peer(&peer(Some(ok))), "{ok}");
        }
        for refused in ["/usr/bin/python3", "/usr/bin/socat", "/tmp/nyx-glimpse-evil"] {
            assert!(!glimpse_peer(&peer(Some(refused))), "{refused}");
        }
        assert!(!glimpse_peer(&peer(None)));
    }

    #[test]
    fn rows_sort_and_merge_as_the_gtk_panel() {
        let file = |hit: Value| file_row(PathBuf::from("/tmp/a.txt"), &hit);
        let mut found = vec![file(Value::Null), system_row("Reboot", "reboot")];
        merge(&mut found, vec![file(json!({"related": true, "excerpt": "x"})), file(json!({"info": {"line": 12}}))]);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].reason, "Line 12", "{found:?}");
        assert_ne!(found[0].category().0, RELATED, "a file listed by name stays in Files");
        let json = found[1].json(&["system:reboot".into()]);
        assert_eq!(json["favorite"], true);
        assert_eq!(json["category"], "System");
        assert_eq!(json["icon"], "system-reboot-symbolic");
        assert_eq!(json["hints"][0]["label"], "Run");
    }
}
