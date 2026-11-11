//! Nyx apps' actions as Glimpse results, through nyx-actions: the registry, word
//! resolution, completion and checks the `nyx` command uses, so `notes create Ideas`
//! here and `nyx notes create Ideas` in a terminal reach the app the same way.
//!
//! A query whose first word names an app (`notes`, `calendar today`) lists that app's
//! commands, the command the words resolve to with its arguments, and completions for
//! the word being typed. Other queries match actions by title, alias and keyword
//! ([`Fuzzy`]). Choosing a command that still needs arguments fills the search instead
//! of running it; one that asks first waits for a second Enter; the app has the last word.

use crate::providers::{Candidate, Provider, SearchContext, Target};
use anyhow::Result;
use nyx_actions::resolve::{self, Resolution};
use nyx_actions::{Catalog, Entry, InvokeOptions, Listed, Options, Registry, ResultKind, Risk};
use serde_json::{Map, Value};
use std::sync::Arc;

/// Rows for one query, at most.
const MAX_ROWS: usize = 8;

pub struct Commands {
    runtime: tokio::runtime::Runtime,
    registry: Registry,
    /// [`PANELS`], and the `example` panel for test copies (`GLIMPSE_TEST_PANELS=1`).
    panels: Vec<String>,
}

/// An action with the arguments typed so far.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub app: String,
    pub app_id: String,
    pub icon: String,
    pub action: String,
    pub title: String,
    pub description: String,
    pub arguments: Map<String, Value>,
    pub missing: Vec<String>,
    pub usage: String,
    pub risk: Risk,
    pub asks: bool,
    pub available: bool,
    pub reason: Option<String>,
    /// What the search becomes on Tab (or on Enter while arguments are missing).
    pub fill: String,
    /// Typed as a command (`notes create …`), not only the app's name.
    pub explicit: bool,
    pub result: Option<ResultKind>,
}

/// Icons for the words in action names, as Umbra (the suite's icon theme) draws them.
const VERB_ICONS: &[(&[&str], &str)] = &[
    (&["search", "find"], "edit-find-symbolic"),
    (&["query", "ask"], "dialog-question-symbolic"),
    (&["open", "show"], "document-open-symbolic"),
    (&["reveal"], "folder-open-symbolic"),
    (&["list", "lists", "pages", "page", "playlists", "routines", "queue"], "view-list-symbolic"),
    (&["location", "locations"], "find-location-symbolic"),
    (&["status", "capabilities", "info", "current", "now"], "dialog-information-symbolic"),
    (&["today"], "task-due-symbolic"),
    (&["next", "upcoming", "times", "slots"], "appointment-soon-symbolic"),
    (&["play", "start", "resume", "control"], "media-playback-start-symbolic"),
    (&["pause"], "media-playback-pause-symbolic"),
    (&["stop"], "media-playback-stop-symbolic"),
    (&["restart", "refresh", "reload", "sync"], "view-refresh-symbolic"),
    (&["skip"], "media-skip-forward-symbolic"),
    (&["previous"], "media-skip-backward-symbolic"),
    (&["volume", "ring"], "audio-volume-high-symbolic"),
    (&["output", "outputs"], "audio-speakers-symbolic"),
    (&["add", "create", "new"], "list-add-symbolic"),
    (&["append"], "insert-text-symbolic"),
    (&["edit", "update", "rename", "set"], "document-edit-symbolic"),
    (&["value"], "preferences-other-symbolic"),
    (&["replace"], "edit-find-replace-symbolic"),
    (&["delete", "remove"], "edit-delete-symbolic"),
    (&["trash"], "user-trash-symbolic"),
    (&["clear"], "edit-clear-all-symbolic"),
    (&["restore", "undo"], "edit-undo-symbolic"),
    (&["complete", "done", "select", "choose", "use"], "object-select-symbolic"),
    (&["copy"], "edit-copy-symbolic"),
    (&["move"], "edit-cut-symbolic"),
    (&["paste", "clip"], "edit-paste-symbolic"),
    (&["send", "share"], "document-send-symbolic"),
    (&["lock"], "system-lock-screen-symbolic"),
    (&["unlock"], "changes-allow-symbolic"),
    (&["private"], "view-conceal-symbolic"),
    (&["read"], "text-x-generic-symbolic"),
    (&["devices"], "phone-symbolic"),
    (&["monitors"], "video-display-symbolic"),
    (&["switch"], "system-shutdown-symbolic"),
    (&["light"], "display-brightness-symbolic"),
    (&["scene"], "preferences-desktop-appearance-symbolic"),
    (&["climate"], "weather-few-clouds-symbolic"),
];

/// The action's own icon (`x-nyx.icon`), else one for the last word of its name that
/// has one (`location` in `select_location`), else a neutral one: never the app's tile.
fn action_icon(a: &nyx_actions::spec::Action) -> String {
    if !a.icon.is_empty() {
        return a.icon.clone();
    }
    a.verb()
        .rsplit(['.', '_'])
        .find_map(|word| VERB_ICONS.iter().find(|(words, _)| words.contains(&word)).map(|(_, icon)| *icon))
        .unwrap_or("system-run-symbolic")
        .to_string()
}

impl Command {
    fn new(entry: &Entry, listed: &Listed, arguments: Map<String, Value>, fill: String, explicit: bool) -> Command {
        let a = &listed.action;
        let missing = a.required().into_iter().filter(|r| !arguments.contains_key(*r)).map(str::to_string).collect();
        Command {
            app: entry.app.name.clone(),
            app_id: entry.app.app.clone(),
            icon: action_icon(a),
            action: a.id.clone(),
            title: a.title.clone(),
            description: a.description.clone(),
            arguments,
            missing,
            usage: usage(a.namespace(), a),
            risk: a.risk,
            asks: a.asks(),
            available: listed.available,
            reason: listed.reason.clone(),
            fill,
            explicit,
            result: a.result,
        }
    }

    /// One per action and arguments; plain actions share the fuzzy rows' IDs, so
    /// favorites and learning follow them.
    pub fn id(&self) -> String {
        if self.arguments.is_empty() {
            format!("action:{}", self.action)
        } else {
            format!("command:{}:{}", self.action, Value::Object(self.arguments.clone()))
        }
    }

    /// The title with the values given: "Create a note · Project ideas".
    pub fn row_title(&self) -> String {
        let values: Vec<String> =
            self.arguments.values().map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).collect();
        if values.is_empty() { self.title.clone() } else { format!("{} · {}", self.title, values.join(", ")) }
    }

    pub fn subtitle(&self) -> String {
        if self.missing.is_empty() { format!("{} · {}", self.app, self.usage) } else { self.usage.clone() }
    }

    /// The row's tag: what still stops it.
    pub fn badge(&self) -> String {
        if !self.available {
            return "Unavailable".into();
        }
        if !self.missing.is_empty() {
            return format!("Needs {}", self.missing.iter().map(|m| m.to_uppercase()).collect::<Vec<_>>().join(", "));
        }
        if self.asks { "Asks first".into() } else { String::new() }
    }

    pub fn ready(&self) -> bool {
        self.available && self.missing.is_empty()
    }

    /// The preview's facts.
    pub fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![("App".to_string(), self.app.clone())];
        for (k, v) in &self.arguments {
            d.push((k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())));
        }
        if !self.missing.is_empty() {
            d.push(("Needs".into(), self.missing.join(", ")));
        }
        let mut what = self.risk.label().to_string();
        if self.asks {
            what.push_str("; asks before it runs");
        }
        d.push(("It".into(), what));
        if let Some(reason) = self.reason.as_ref().filter(|_| !self.available) {
            d.push(("Now".into(), reason.clone()));
        }
        d
    }
}

/// `notes create TITLE [--pinned]`.
pub fn usage(namespace: &str, a: &nyx_actions::Action) -> String {
    let mut out = format!("{namespace} {}", a.verb().replace('.', " "));
    let required = a.required();
    for p in &a.positional {
        let name = p.to_uppercase();
        out.push_str(&if required.contains(&p.as_str()) { format!(" {name}") } else { format!(" [{name}]") });
    }
    if let Some(props) = a.input_schema.get("properties").and_then(Value::as_object) {
        for (name, schema) in props.iter().filter(|(n, _)| !a.positional.contains(n)) {
            let flag = name.replace('_', "-");
            let part = if schema.get("type").and_then(Value::as_str) == Some("boolean") {
                format!("--{flag}")
            } else {
                format!("--{flag} {}", name.to_uppercase())
            };
            out.push_str(&if required.contains(&name.as_str()) { format!(" {part}") } else { format!(" [{part}]") });
        }
    }
    out
}

impl Default for Commands {
    fn default() -> Self {
        Commands::new(Options::default())
    }
}

impl Commands {
    pub fn new(options: Options) -> Commands {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("glimpse-actions")
            .enable_all()
            .build()
            .expect("a runtime for app actions");
        let mut panels: Vec<String> = PANELS.iter().map(|p| p.to_string()).collect();
        if std::env::var("GLIMPSE_TEST_PANELS").is_ok_and(|v| v == "1") {
            panels.push("example".into());
        }
        Commands { runtime, registry: Registry::new(options), panels }
    }

    /// Panels other than Glimpse's own list (tests).
    pub fn with_panels(mut self, panels: &[&str]) -> Commands {
        self.panels = panels.iter().map(|p| p.to_string()).collect();
        self
    }

    /// What's on offer: as last seen (`fresh: false`, never waits), or brought up to date.
    pub fn catalog(&self, fresh: bool) -> Catalog {
        if fresh { self.runtime.block_on(self.registry.catalog()) } else { self.runtime.block_on(self.registry.cached()) }
    }

    /// The command rows for `q`, when its first word names an app.
    /// `suggest`: also ask running apps' `x-nyx.suggest` actions (the slower pass).
    pub fn rows(&self, catalog: &Catalog, q: &str, suggest: bool) -> Vec<Command> {
        let text = q.trim_start();
        let words = resolve::split_words(text);
        let Some(first) = words.first() else { return Vec::new() };
        let trailing = text.ends_with(char::is_whitespace);
        let Ok(entry) = resolve::find_app(catalog, first) else { return Vec::new() };
        // A lone word that only starts a namespace is likely an app or file name.
        let exact = catalog.app(first).is_some();
        if words.len() == 1 && !trailing && !exact && first.chars().count() < 3 {
            return Vec::new();
        }
        let explicit = words.len() > 1 || trailing;
        let namespace = entry
            .app
            .namespaces
            .iter()
            .find(|n| n.starts_with(&first.to_lowercase()))
            .unwrap_or(&entry.app.namespaces[0])
            .clone();
        let (done, partial): (Vec<String>, String) = if trailing || words.len() == 1 {
            (vec![namespace.clone()].into_iter().chain(words[1..].iter().cloned()).collect(), String::new())
        } else {
            (
                vec![namespace.clone()].into_iter().chain(words[1..words.len() - 1].iter().cloned()).collect(),
                words[words.len() - 1].clone(),
            )
        };
        // The words as typed first, then each completion of the last one.
        let mut texts: Vec<(Vec<String>, bool)> = Vec::new();
        if explicit && !(words.len() == 1 && trailing) {
            let mut typed = done.clone();
            if !partial.is_empty() {
                typed.push(partial.clone());
            }
            texts.push((typed, true));
        }
        // A namespace alone lists verbs; an action's other words come once one is typed.
        let verbs: Vec<String> = entry.actions.iter().map(|l| l.action.verb().split('.').next().unwrap_or_default().to_string()).collect();
        let mut candidates = resolve::complete(catalog, &done, &partial);
        if suggest && done.len() > 1 {
            candidates.extend(self.runtime.block_on(self.registry.suggest(catalog, &done, &partial)));
        }
        for candidate in candidates {
            if candidate.eq_ignore_ascii_case(&partial) && !partial.is_empty() {
                continue;
            }
            if done.len() == 1 && partial.is_empty() && !verbs.contains(&candidate) {
                continue;
            }
            let mut filled = done.clone();
            filled.push(candidate);
            texts.push((filled, false));
        }
        let mut out: Vec<(Command, bool)> = Vec::new();
        for (t, typed) in texts {
            if out.len() >= MAX_ROWS {
                break;
            }
            let Ok(Resolution::Call(call)) = resolve::resolve(catalog, &t) else { continue };
            let Some((entry, listed)) = catalog.find(&call.action) else { continue };
            let fill = format!("{} ", t.iter().map(|w| quote(w)).collect::<Vec<_>>().join(" "));
            let command = Command::new(entry, listed, call.arguments, fill, explicit);
            match out.iter().position(|(c, _)| c.id() == command.id()) {
                // A completion spells out what a typed prefix found: Tab takes its words.
                Some(i) if out[i].1 => {
                    out[i].0.fill = command.fill;
                    out[i].1 = false;
                }
                Some(_) => {}
                None => out.push((command, typed)),
            }
        }
        out.into_iter().map(|(c, _)| c).collect()
    }

    /// The command for action `id` with no arguments yet (a fuzzy match chosen).
    pub fn plain(&self, id: &str) -> Option<Command> {
        let catalog = self.catalog(false);
        let (entry, listed) = catalog.find(id)?;
        let fill = format!("{} {} ", listed.action.namespace(), listed.action.verb().replace('.', " "));
        Some(Command::new(entry, listed, Map::new(), fill, false))
    }

    /// Run it as the person asked in Glimpse.
    pub fn invoke(&self, c: &Command, confirmed: bool) -> Result<Value, nyx_actions::Error> {
        let options = InvokeOptions { confirmed, trusted: true, on_behalf_of: None, frontend: "glimpse".into() };
        self.runtime.block_on(self.registry.invoke(&c.action, Value::Object(c.arguments.clone()), &options))
    }
}

/// The mini-app panels Glimpse ships (`quickshell/Glimpse/panels/<id>/Panel.qml`). An
/// app's manifest can only pick one of these; nothing else runs in the shell.
pub const PANELS: &[&str] = &["music", "home", "files", "calendar", "reminders", "notes", "weather", "containers", "devices"];

/// An app's panel, offered for a query that starts with the app's name.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelOffer {
    pub app: String,
    pub app_id: String,
    pub icon: String,
    pub panel: String,
    /// The words after the app's name (`jazz` in `music jazz`), for the panel to use.
    pub query: String,
    pub topics: Vec<String>,
    pub size: Value,
    pub available: bool,
    pub reason: Option<String>,
}

impl Commands {
    /// The panel for `q` when its first word is exactly an app's name or alias and the
    /// app has one of Glimpse's panels.
    pub fn panel(&self, catalog: &Catalog, q: &str) -> Option<PanelOffer> {
        let text = q.trim_start();
        let first = resolve::split_words(text).into_iter().next()?;
        let entry = catalog.app(&first)?;
        // An app with several namespaces (Settings: settings, devices) shows its panel for
        // the word that names it only.
        if entry.app.namespaces.len() > 1 && entry.app.panel.as_ref().is_some_and(|p| !p.id.eq_ignore_ascii_case(&first)) {
            return None;
        }
        let rest = text[text.find(char::is_whitespace).unwrap_or(text.len())..].trim().to_string();
        self.offer(entry, rest)
    }

    /// The panel of the app with ID `app_id`, as pinned: no words.
    pub fn pinned(&self, catalog: &Catalog, app_id: &str) -> Option<PanelOffer> {
        self.offer(catalog.apps.iter().find(|e| e.app.app == app_id)?, String::new())
    }

    fn offer(&self, entry: &nyx_actions::catalog::Entry, query: String) -> Option<PanelOffer> {
        let panel = entry.app.panel.as_ref().filter(|p| self.panels.contains(&p.id))?;
        let available = entry.running.is_some() || entry.app.activation == nyx_actions::Activation::Dbus;
        // An app with several namespaces names its panel after the panel (Devices, of Settings).
        let title = if entry.app.namespaces.len() > 1 {
            let mut c = panel.id.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        } else {
            entry.app.name.clone()
        };
        Some(PanelOffer {
            app: title,
            app_id: entry.app.app.clone(),
            icon: if entry.app.icon.is_empty() { "system-run-symbolic".into() } else { entry.app.icon.clone() },
            panel: panel.id.clone(),
            query,
            topics: panel.topics.clone(),
            size: serde_json::to_value(&panel.size).unwrap_or(Value::Null),
            available,
            reason: (!available).then(|| format!("Open {} to use this", entry.app.name)),
        })
    }

    /// Follow one of an app's topics for its panel (on the actions runtime, [`spawn`](Self::spawn)).
    pub async fn watch(&self, app_id: &str, topic: &str) -> Result<nyx_actions::Watch, nyx_actions::Error> {
        self.registry.watch(app_id, topic).await
    }

    /// Spawn `f` on the actions runtime (a panel's live state).
    pub fn spawn<F: std::future::Future<Output = ()> + Send + 'static>(&self, f: F) -> tokio::task::AbortHandle {
        self.runtime.spawn(f).abort_handle()
    }

    /// An action a panel runs: only its own app's, checked as any other call.
    pub fn run_for_panel(&self, app_id: &str, action: &str, arguments: Map<String, Value>, confirmed: bool) -> Result<(Command, Value), nyx_actions::Error> {
        let catalog = self.catalog(false);
        let (entry, listed) = catalog
            .find(action)
            .filter(|(e, _)| e.app.app == app_id)
            .ok_or_else(|| nyx_actions::Error::new(nyx_actions::ErrorKind::PermissionDenied, format!("{action} isn't this panel's app's")))?;
        let c = Command::new(entry, listed, arguments, String::new(), true);
        let value = self.invoke(&c, confirmed)?;
        Ok((c, value))
    }

    /// The app's own window, at `arguments` when it has an open action (`nyx.open`).
    pub fn open_app(&self, app_id: &str, arguments: Map<String, Value>) -> anyhow::Result<()> {
        let catalog = self.catalog(false);
        let entry = catalog.apps.iter().find(|e| e.app.app == app_id).ok_or_else(|| anyhow::anyhow!("{app_id} isn't installed"))?;
        if let Some(open) = &entry.app.open
            && let Some((e, l)) = catalog.find(&open.action)
        {
            let c = Command::new(e, l, arguments, String::new(), true);
            return self.invoke(&c, true).map(|_| ()).map_err(|e| anyhow::anyhow!(e.message));
        }
        use gtk::prelude::AppInfoExt;
        let info = gio_unix::DesktopAppInfo::new(&format!("{app_id}.desktop")).ok_or_else(|| anyhow::anyhow!("{} has no desktop entry", entry.app.name))?;
        info.launch(&[], None::<&gtk::gio::AppLaunchContext>)?;
        Ok(())
    }
}

fn quote(word: &str) -> String {
    if word.is_empty() || word.contains(char::is_whitespace) { format!("\"{word}\"") } else { word.to_string() }
}

/// What a command gave, for the status line and the preview pane (`text` body HTML and
/// details), by the action's result kind or the result's shape.
pub fn result_view(c: &Command, value: &Value) -> (String, Option<String>, Vec<(String, String)>) {
    let kind = c.result.or(match value {
        Value::Null => Some(ResultKind::None),
        Value::Object(m) if m.get("items").is_some_and(Value::is_array) => Some(ResultKind::List),
        Value::Object(m) if m.len() == 1 && m.get("message").is_some_and(Value::is_string) => Some(ResultKind::Message),
        Value::Array(_) => Some(ResultKind::List),
        Value::Object(_) => Some(ResultKind::Object),
        _ => None,
    });
    let text = |v: &Value| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
    match (kind, value) {
        (Some(ResultKind::None), _) | (_, Value::Null) => (format!("{}: done", c.title), None, Vec::new()),
        (Some(ResultKind::Message), v) => (v.get("message").map(text).unwrap_or_else(|| "Done".into()), None, Vec::new()),
        (Some(ResultKind::List), v) => {
            let items = v.get("items").or(Some(v)).and_then(Value::as_array).cloned().unwrap_or_default();
            let lines: Vec<String> = items.iter().take(200).map(item_line).collect();
            let summary = match items.len() {
                0 => format!("{}: nothing found", c.title),
                1 => format!("{}: 1 result", c.title),
                n => format!("{}: {n} results", c.title),
            };
            (summary, Some(lines.iter().map(|l| format!("<p>{}</p>", escape(l))).collect::<String>()), Vec::new())
        }
        (Some(ResultKind::Object), Value::Object(m)) => {
            let details = m
                .iter()
                .filter(|(_, v)| !v.is_null())
                .take(24)
                .map(|(k, v)| (k.replace('_', " "), crate::providers::summary(v)))
                .collect();
            (format!("{}: done", c.title), None, details)
        }
        (_, v) => {
            let line = crate::providers::summary(v);
            (if line.is_empty() { format!("{}: done", c.title) } else { line }, None, Vec::new())
        }
    }
}

fn item_line(item: &Value) -> String {
    match item {
        Value::Object(m) => {
            let pick = |keys: &[&str]| keys.iter().find_map(|k| m.get(*k).and_then(Value::as_str)).map(str::to_string);
            match (
                pick(&["title", "name", "summary", "label", "path", "id"]),
                pick(&["subtitle", "detail", "artist", "when", "location"]),
            ) {
                (Some(t), Some(s)) => format!("{t} · {s}"),
                (Some(t), None) => t,
                _ => Value::Object(m.clone()).to_string(),
            }
        }
        Value::String(s) => s.clone(),
        v => v.to_string(),
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Actions matched by their title, aliases and keywords when the query doesn't start
/// with an app's name.
pub struct Fuzzy(pub Arc<Commands>);

impl Provider for Fuzzy {
    fn id(&self) -> &str {
        "actions"
    }
    fn discover(&mut self, ctx: &SearchContext<'_>) -> Result<Vec<Candidate>> {
        let q = ctx.query.trim();
        if q.chars().count() < 2 {
            return Ok(vec![]);
        }
        let catalog = self.0.catalog(true);
        if resolve::split_words(q).first().is_some_and(|w| catalog.app(w).is_some()) {
            return Ok(vec![]);
        }
        Ok(catalog
            .actions()
            .filter(|(e, l)| l.available && e.app.app != "io.github.design_nexus.nyx.Glimpse")
            .map(|(e, l)| {
                let a = &l.action;
                let aliases: Vec<&str> = a.aliases.iter().map(|x| x.word()).collect();
                Candidate {
                    id: format!("action:{}", a.id),
                    title: a.title.clone(),
                    subtitle: format!("{} · {}", e.app.name, usage(a.namespace(), a)),
                    keywords: format!(
                        "{} {} {} {} {}",
                        e.app.name,
                        a.id.replace(['.', '_'], " "),
                        aliases.join(" "),
                        a.keywords.join(" "),
                        a.description
                    )
                    .to_lowercase(),
                    target: Target::Shared(a.id.clone()),
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn catalog() -> Catalog {
        let notes = nyx_actions::Description::from_value(
            &json!({"tools": [
                {"name": "notes.search", "description": "Search notes.", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}, "x-nyx": {"risk": "read_only", "positional": ["query"]}},
                {"name": "notes.create", "description": "Create a note.", "inputSchema": {"type": "object", "properties": {"title": {"type": "string"}}, "required": ["title"]}, "annotations": {"title": "Create a note"}, "x-nyx": {"risk": "writes", "positional": ["title"], "aliases": ["new"]}},
                {"name": "notes.delete", "description": "Delete a note.", "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}, "x-nyx": {"risk": "destructive", "positional": ["id"]}}
            ], "nyx": {"api": 2, "app": "io.github.design_nexus.nyx.Notes", "name": "Notes", "namespaces": ["notes"], "bus_name": "io.github.design_nexus.nyx.Notes", "activation": "dbus", "default": "notes.search"}}),
            None,
        )
        .unwrap();
        Catalog::merge(vec![notes], vec![], &|_| None).0
    }

    fn rows(q: &str) -> Vec<Command> {
        Commands::new(Options { dirs: vec![], bus_address: None, live: false }).rows(&catalog(), q, false)
    }

    #[test]
    fn each_action_has_its_own_icon_not_the_app_s() {
        let action = |name: &str, icon: Option<&str>| {
            let mut x = json!({"risk": "read_only"});
            if let Some(icon) = icon {
                x["icon"] = json!(icon);
            }
            let d = nyx_actions::Description::from_value(
                &json!({"tools": [{"name": name, "description": "Do it.", "x-nyx": x}],
                    "nyx": {"api": 2, "app": "io.github.design_nexus.nyx.Weather", "name": "Weather", "namespaces": [name.split('.').next().unwrap()], "bus_name": "io.github.design_nexus.nyx.Weather"}}),
                None,
            )
            .unwrap();
            action_icon(&d.actions[0])
        };
        assert_eq!(action("weather.current", Some("weather-few-clouds-symbolic")), "weather-few-clouds-symbolic");
        assert_eq!(action("weather.select_location", None), "find-location-symbolic");
        assert_eq!(action("notes.vault_search", None), "edit-find-symbolic");
        assert_eq!(action("files.trash.restore", None), "edit-undo-symbolic");
        assert_eq!(action("music.play", None), "media-playback-start-symbolic");
        assert_eq!(action("example.frobnicate", None), "system-run-symbolic");
        assert!(rows("notes").iter().all(|c| c.icon.ends_with("-symbolic")), "never the app's tile");
    }

    #[test]
    fn an_app_word_lists_its_commands_and_resolves_arguments() {
        let all = rows("notes");
        assert_eq!(
            all.iter().map(|c| c.action.as_str()).collect::<Vec<_>>(),
            vec!["notes.create", "notes.delete", "notes.search"]
        );
        assert!(!all[0].explicit, "the app's name alone sits below apps and files");
        assert_eq!(all[0].fill, "notes create ");
        assert_eq!(all[0].badge(), "Needs TITLE");

        let typed = rows("notes create Project ideas");
        assert_eq!(typed[0].row_title(), "Create a note · Project ideas");
        assert!(typed[0].ready() && typed[0].explicit);
        assert_eq!(typed[0].id(), r#"command:notes.create:{"title":"Project ideas"}"#);

        let partial = rows("notes cr");
        assert_eq!(partial.iter().map(|c| c.action.as_str()).collect::<Vec<_>>(), vec!["notes.create"]);
        assert_eq!(partial[0].fill, "notes create ");

        let delete = rows("notes delete n1");
        assert_eq!((delete[0].badge().as_str(), delete[0].asks), ("Asks first", true));
        let words = rows("notes architecture");
        assert_eq!((words[0].action.as_str(), words[0].arguments["query"].as_str()), ("notes.search", Some("architecture")));
        assert!(rows("no").is_empty(), "two letters alone stay an ordinary search");
        assert!(rows("films play").is_empty());
    }

    #[test]
    fn panels_are_offered_for_the_app_name_and_only_glimpse_s_own() {
        let make = |panel: &str| {
            let d = nyx_actions::Description::from_value(
                &json!({"tools": [{"name": "music.status", "description": "Status.", "x-nyx": {"risk": "read_only"}}],
                    "nyx": {"api": 2, "app": "io.github.design_nexus.nyx.Music", "name": "Music", "namespaces": ["music"], "bus_name": "io.github.design_nexus.nyx.Music",
                        "activation": "dbus", "topics": [{"name": "playback"}], "panel": {"id": panel, "topics": ["playback"], "size": {"preferred": [520, 380]}}}}),
                None,
            )
            .unwrap();
            Catalog::merge(vec![d], vec![], &|_| None).0
        };
        let commands = Commands::new(Options { dirs: vec![], bus_address: None, live: false }).with_panels(&["music"]);
        let offer = commands.panel(&make("music"), "music  jazz piano").unwrap();
        assert_eq!((offer.panel.as_str(), offer.query.as_str(), offer.topics.clone()), ("music", "jazz piano", vec!["playback".to_string()]));
        assert_eq!(offer.size["preferred"], json!([520, 380]));
        assert!(offer.available);
        assert!(commands.panel(&make("music"), "mus").is_none(), "only the app's whole name");
        assert!(commands.panel(&make("evil"), "music").is_none(), "only Glimpse's panels");
    }

    #[test]
    fn results_read_by_kind() {
        let c = rows("notes create x").remove(0);
        let (summary, html, _) = result_view(&c, &json!({"items": [{"title": "Plans", "subtitle": "Today"}, {"title": "<b>"}]}));
        assert_eq!(summary, "Create a note: 2 results");
        assert_eq!(html.unwrap(), "<p>Plans · Today</p><p>&lt;b&gt;</p>");
        assert_eq!(result_view(&c, &json!({"message": "Saved"})).0, "Saved");
        let (_, _, details) = result_view(&c, &json!({"id": "n1", "folder": "Inbox"}));
        assert_eq!(details[0], ("folder".to_string(), "Inbox".to_string()));
        assert_eq!(result_view(&c, &Value::Null).0, "Create a note: done");
    }
}
