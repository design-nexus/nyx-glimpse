//! Glimpse's search without GTK, for its Quickshell interface (`quickshell/Glimpse`).
//! `nyx-glimpse-backend` serves it over `$XDG_RUNTIME_DIR/nyx-glimpse/ui.sock`
//! (nyx-ui-link); the GTK panel runs the same providers in its own process.
//!
//! - topic `panel`: `{open, serial}`, whether the interface should show;
//! - topic `results`: `{generation, query, rows, status, complete}` for the latest
//!   query; rows of an older query are never published over a newer one;
//! - operations: `Query {text, generation}`, `Activate {id, generation}` (replies
//!   `{done}`, `{ask: message}` for a second press, or `{message}` to show),
//!   `Show`, `Hide`, `Toggle`.
//!
//! Search runs on one worker thread: the fast providers first (apps, file names, file
//! contents by words, utilities, power), then the slower ones (Omarchy, services,
//! plugins, shared actions, contents by meaning). A newer query cancels the rest.

use crate::{
    config::Paths,
    index, learning, plugins,
    providers::{self, SearchContext, Target},
    query, semantic, utility,
};
use anyhow::{Result, anyhow, bail};
use gtk::gio::{self, prelude::*};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

/// What a result does when chosen.
#[derive(Clone, Debug)]
pub enum Item {
    Provider(Target),
    File(PathBuf),
    Copy(String),
    System(&'static str),
}

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
    fn json(&self) -> Value {
        json!({"id": self.id, "title": self.title, "subtitle": self.subtitle, "reason": self.reason, "kind": self.kind, "icon": self.icon})
    }
}

fn provider_row(c: providers::Candidate) -> Row {
    let (kind, icon) = match &c.target {
        Target::App(id) => (
            "app",
            gio_unix::DesktopAppInfo::new(id)
                .and_then(|a| a.icon())
                .and_then(|i| IconExt::to_string(&i))
                .map(|s| s.to_string())
                .unwrap_or_else(|| "application-x-executable".into()),
        ),
        Target::File(_) => ("file", "text-x-generic".to_string()),
        Target::System(_) => ("system", "system-shutdown-symbolic".to_string()),
        Target::Omarchy(_) => ("command", "utilities-terminal-symbolic".to_string()),
        Target::Service(_) => ("service", "system-run-symbolic".to_string()),
        Target::Plugin(..) => ("plugin", "application-x-addon-symbolic".to_string()),
        Target::Shared(_) => ("action", "media-playback-start-symbolic".to_string()),
    };
    let item = match c.target {
        Target::File(path) => Item::File(path),
        target => Item::Provider(target),
    };
    Row { id: c.id, title: c.title, subtitle: c.subtitle, reason: String::new(), kind, icon, item }
}

fn file_row(path: PathBuf, hit: &Value) -> Row {
    Row {
        id: format!("file:{}", path.display()),
        title: path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
        subtitle: folder_of(&path),
        reason: if hit["related"] == true {
            "Related".into()
        } else if let Some(page) = hit["loc"].as_f64().filter(|_| hit["kind"] == "pdf") {
            format!("p. {page}")
        } else if hit.is_null() {
            String::new()
        } else {
            "Content".into()
        },
        kind: "file",
        icon: "text-x-generic".into(),
        item: Item::File(path),
    }
}

fn folder_of(path: &Path) -> String {
    let parent = path.parent().unwrap_or(path);
    match std::env::var_os("HOME").map(PathBuf::from).and_then(|home| parent.strip_prefix(home).ok().map(Path::to_owned)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => parent.display().to_string(),
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
    session: Option<semantic::Session>,
    db: Option<rusqlite::Connection>,
}

impl Engine {
    pub fn new(paths: Paths, apps: Vec<(String, String, String)>) -> Engine {
        let mut fast = providers::Registry::default();
        let _ = fast.register(providers::Applications(apps));
        let mut slow = providers::Registry::default();
        let _ = slow.register(crate::omarchy::Omarchy::default());
        let _ = slow.register(providers::DesktopServices);
        let _ = slow.register(providers::SharedActions::default());
        Engine { paths, fast, slow, files: providers::Files::default(), extensions: providers::Extensions, session: None, db: None }
    }

    /// The fast results for `q`: `None` when a newer query came first.
    pub fn fast(&mut self, q: &str, cancelled: &dyn Fn() -> bool) -> Result<(Vec<Row>, String)> {
        let p = &self.paths;
        let c = p.load()?;
        let q = q.trim();
        if q.is_empty() {
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
        let mut rows = Vec::new();
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
            rows.extend(hits.into_iter().map(provider_row));
        }
        if self.db.is_none() {
            self.db = index::open(p).ok();
        }
        if let Some(conn) = &self.db
            && let Ok(hits) = index::content_search_in(conn, p, &c, q)
        {
            rows.extend(hits.into_iter().map(|(file, info)| file_row(file.path, &info)));
        }
        for (name, cmd) in [("Lock screen", "lock"), ("Log out", "logout"), ("Reboot", "reboot"), ("Shut down", "shutdown")] {
            if query::name_rank(name, q).is_some() {
                rows.push(Row {
                    id: format!("system:{cmd}"),
                    title: name.into(),
                    subtitle: String::new(),
                    reason: String::new(),
                    kind: "system",
                    icon: "system-shutdown-symbolic".into(),
                    item: Item::System(cmd),
                });
            }
        }
        Ok((rows, String::new()))
    }

    /// The slower results for `q`, to add after the fast ones.
    pub fn slow(&mut self, q: &str, cancelled: &dyn Fn() -> bool) -> Result<(Vec<Row>, String)> {
        let p = &self.paths;
        let c = p.load()?;
        let q = q.trim();
        if q.is_empty() || utility::intent(q) != utility::Intent::Search {
            return Ok((vec![], String::new()));
        }
        let profile = learning::Profile::load(p, &c).unwrap_or_default();
        let ctx = SearchContext { paths: p, config: &c, query: q, cancelled };
        let mut rows: Vec<Row> = self.slow.search(&ctx, &profile).0.into_iter().take(30).map(|(_, c)| provider_row(c)).collect();
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
                Err(e) => status = e.to_string(),
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

/// What choosing a result did.
pub enum Done {
    /// Done; the interface closes.
    Closed,
    /// Asks first: the same result chosen again runs it.
    Ask(String),
    /// Something to show in the status line; the interface stays.
    Show(String),
}

/// Run `row`'s action. `confirmed` is true for the second press on a result that asked.
pub fn activate(p: &Paths, row: &Row, confirmed: bool) -> Result<Done> {
    let c = p.load()?;
    match &row.item {
        Item::Copy(text) => {
            copy(text)?;
            Ok(Done::Closed)
        }
        Item::File(path) => {
            if !crate::config::allowed(&c, path) {
                bail!("Source permission revoked");
            }
            gio::AppInfo::launch_default_for_uri(&gio::File::for_path(path).uri(), None::<&gio::AppLaunchContext>)?;
            Ok(Done::Closed)
        }
        Item::System(cmd) => {
            if *cmd != "lock" && !confirmed {
                return Ok(Done::Ask(format!("{} · Press Enter again to confirm", row.title)));
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
                return Ok(Done::Ask(format!("{} · Press Enter again to confirm", row.title)));
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
            Ok(Done::Show(String::from_utf8_lossy(&output).lines().collect::<Vec<_>>().join(" · ")))
        }
        Item::Provider(Target::Shared(id)) => match crate::aicore::Core::new()?.invoke(id, json!({}), confirmed)? {
            crate::aicore::Outcome::Ask => Ok(Done::Ask(format!("{} · Press Enter again to run it", row.title))),
            crate::aicore::Outcome::Done(value) => {
                let summary = providers::summary(&value);
                Ok(if summary.is_empty() { Done::Closed } else { Done::Show(summary) })
            }
        },
        Item::Provider(Target::Plugin(id, _, action)) => {
            let plugin = plugins::get(p, id)?;
            if !plugins::authorized(&plugin) || !plugins::valid_action(p, &plugin, action) {
                bail!("plugin permission revoked");
            }
            let action = match action {
                plugins::Action::Activate { token } => plugins::activate(p, id, token)?,
                other => other.clone(),
            };
            match action {
                plugins::Action::Copy { text } => copy(&text)?,
                plugins::Action::Open { path } => {
                    gio::AppInfo::launch_default_for_uri(&gio::File::for_path(&path).uri(), None::<&gio::AppLaunchContext>)?
                }
                plugins::Action::Url { url } => gio::AppInfo::launch_default_for_uri(&url, None::<&gio::AppLaunchContext>)?,
                plugins::Action::Activate { .. } => bail!("the plugin answered with another activation"),
            }
            Ok(Done::Closed)
        }
        Item::Provider(Target::System(_) | Target::File(_)) => bail!("Unsupported result"),
    }
}

fn copy(text: &str) -> Result<()> {
    use std::io::Write;
    let mut child = Command::new("wl-copy").stdin(std::process::Stdio::piped()).spawn()?;
    child.stdin.take().ok_or_else(|| anyhow!("wl-copy has no input"))?.write_all(text.as_bytes())?;
    child.wait()?;
    Ok(())
}

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

/// Whether a Quickshell interface is connected to this backend.
fn interface_connected(link: &nyx_ui_link::Link) -> bool {
    link.peers().iter().any(|peer| {
        peer.exe.as_deref().and_then(Path::file_name).is_some_and(|n| {
            let n = n.to_string_lossy();
            n == "qs" || n.starts_with("quickshell") || n == ".quickshell-wrapped"
        })
    })
}

/// Serve the interface until the process ends.
pub fn serve(p: Paths) -> Result<()> {
    let generation = Arc::new(AtomicU64::new(0));
    let rows: Arc<Mutex<(u64, HashMap<String, Row>)>> = Arc::default();
    let panel = Arc::new(Mutex::new((false, 0u64)));
    let (jobs, job_rx) = mpsc::channel::<(u64, String)>();
    let link_slot: Arc<Mutex<Option<Arc<nyx_ui_link::Link>>>> = Arc::default();

    let (gen_h, rows_h, panel_h, slot_h, paths_h) = (generation.clone(), rows.clone(), panel.clone(), link_slot.clone(), p.clone());
    let link = Arc::new(
        nyx_ui_link::serve(nyx_ui_link::Options::new("nyx-glimpse"), move |req| {
            let Some(link) = slot_h.lock().unwrap().clone() else {
                return req.err("starting", "Glimpse is starting");
            };
            let publish_panel = |open: bool| {
                let mut panel = panel_h.lock().unwrap();
                panel.0 = open;
                panel.1 += 1;
                link.publish("panel", json!({"open": panel.0, "serial": panel.1}));
            };
            let op = req.op.clone();
            match op.as_str() {
                "Show" => {
                    publish_panel(true);
                    req.ok(json!({}))
                }
                "Hide" => {
                    if !interface_connected(&link) {
                        return req.err("no_interface", "Glimpse's Quickshell interface isn't running");
                    }
                    publish_panel(false);
                    req.ok(json!({}))
                }
                "Toggle" => {
                    if !interface_connected(&link) {
                        return req.err("no_interface", "Glimpse's Quickshell interface isn't running");
                    }
                    let open = !panel_h.lock().unwrap().0;
                    publish_panel(open);
                    req.ok(json!({"open": open}))
                }
                "Query" => {
                    let g = req.body["generation"].as_u64().unwrap_or(0);
                    let text = req.body["text"].as_str().unwrap_or("").to_string();
                    gen_h.store(g, Ordering::SeqCst);
                    let _ = jobs.send((g, text));
                    req.ok(json!({}))
                }
                "Activate" => {
                    let g = req.body["generation"].as_u64().unwrap_or(0);
                    let id = req.body["id"].as_str().unwrap_or("").to_string();
                    let confirmed = req.body["confirmed"].as_bool().unwrap_or(false);
                    let row = {
                        let rows = rows_h.lock().unwrap();
                        (rows.0 == g).then(|| rows.1.get(&id).cloned()).flatten()
                    };
                    let Some(row) = row else { return req.err("stale", "That result is gone; search again") };
                    let p = paths_h.clone();
                    std::thread::spawn(move || match activate(&p, &row, confirmed) {
                        Ok(Done::Closed) => {
                            if learning::eligible_id(&row.id) {
                                let _ = learning::record(&p, &row.id, "");
                            }
                            req.ok(json!({"done": true}))
                        }
                        Ok(Done::Ask(message)) => req.ok(json!({"ask": message})),
                        Ok(Done::Show(message)) => req.ok(json!({"message": message})),
                        Err(e) => req.err("failed", e.to_string()),
                    });
                }
                other => req.err("unknown_op", format!("no operation {other}")),
            }
        })
        .map_err(|e| anyhow!("{e}"))?,
    );
    *link_slot.lock().unwrap() = Some(link.clone());
    link.publish("panel", json!({"open": false, "serial": 0}));
    link.publish("results", json!({"generation": 0, "query": "", "rows": [], "status": "", "complete": true}));

    let mut engine = Engine::new(p.clone(), applications());
    let publish = |g: u64, q: &str, found: &[Row], status: &str, complete: bool| {
        if generation.load(Ordering::SeqCst) != g {
            return;
        }
        *rows.lock().unwrap() = (g, found.iter().map(|r| (r.id.clone(), r.clone())).collect());
        link.publish(
            "results",
            json!({"generation": g, "query": q, "rows": found.iter().map(Row::json).collect::<Vec<_>>(), "status": status, "complete": complete}),
        );
    };
    loop {
        let (mut g, mut q) = match job_rx.recv_timeout(Duration::from_secs(1)) {
            Ok(job) => job,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                engine.idle();
                continue;
            }
            Err(_) => return Ok(()),
        };
        std::thread::sleep(Duration::from_millis(10));
        while let Ok((ng, nq)) = job_rx.try_recv() {
            (g, q) = (ng, nq);
        }
        let cancelled = || generation.load(Ordering::SeqCst) != g;
        if cancelled() {
            continue;
        }
        let (mut found, status) = engine.fast(&q, &cancelled).unwrap_or_else(|e| (vec![], e.to_string()));
        publish(g, &q, &found, &status, false);
        std::thread::sleep(Duration::from_millis(150));
        if cancelled() {
            continue;
        }
        let (more, status) = engine.slow(&q, &cancelled).unwrap_or_else(|e| (vec![], e.to_string()));
        for row in more {
            if !found.iter().any(|r| r.id == row.id) {
                found.push(row);
            }
        }
        publish(g, &q, &found, &status, true);
    }
}

/// `nyx-glimpse --toggle` and `--hide` go to the Quickshell interface when it runs
/// (Shell Core); `Err` means it doesn't, and the GTK panel answers instead.
pub fn ask_interface(op: &str) -> Result<()> {
    if std::env::var("GLIMPSE_UI").is_ok_and(|v| v == "gtk") {
        bail!("GTK panel requested");
    }
    let path = nyx_ui_link::socket_path("nyx-glimpse");
    if !path.exists() {
        bail!("no backend");
    }
    nyx_ui_link::call(&path, op, json!({})).map(|_| ()).map_err(|e| anyhow!("{e}"))
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
        let (rows, _) = engine.fast("2+2", &|| false).unwrap();
        assert_eq!(rows[0].kind, "answer");
        let reboot = engine.fast("reboot", &|| false).unwrap().0.into_iter().find(|r| r.id == "system:reboot").unwrap();
        assert!(matches!(activate(&p, &reboot, false).unwrap(), Done::Ask(_)), "reboot asks first");
        assert!(engine.fast("report", &|| true).is_ok());
    }
}
