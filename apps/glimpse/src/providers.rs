//! Local provider contract. Discovery returns data; only the dispatcher executes an Effect.
use crate::{
    config::{Config, Paths},
    learning::{Profile, Rank},
    plugins,
    query::Matcher,
};
use anyhow::{Result, bail};
use std::collections::BTreeMap;
#[derive(Clone, Debug)]
pub enum Target {
    App(String),
    File(std::path::PathBuf),
    System(String),
    Omarchy(String),
    Service(String),
    Plugin(String, String, plugins::Action),
    /// An action a Nyx app offers through Nyx AI Core's shared registry.
    Shared(String),
}
#[derive(Clone, Debug)]
pub struct Candidate {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub keywords: String,
    pub target: Target,
}
#[derive(Clone, Debug)]
pub struct ContextAction {
    pub id: String,
    pub title: String,
}
pub struct SearchContext<'a> {
    pub paths: &'a Paths,
    pub config: &'a Config,
    pub query: &'a str,
    pub cancelled: &'a dyn Fn() -> bool,
}
pub trait Provider: Send {
    fn id(&self) -> &str;
    fn discover(&mut self, context: &SearchContext<'_>) -> Result<Vec<Candidate>>;
    fn actions(&self, target: &Target) -> Vec<ContextAction> {
        actions(target)
    }
    fn resolve(&self, context: &SearchContext<'_>, target: &Target, action: &str) -> Result<Effect> {
        resolve(context.paths, target, action)
    }
}
#[derive(Default)]
pub struct Registry {
    providers: BTreeMap<String, Box<dyn Provider>>,
}
impl Registry {
    pub fn register(&mut self, provider: impl Provider + 'static) -> Result<()> {
        let id = provider.id().to_string();
        if id.is_empty() || self.providers.contains_key(&id) {
            bail!("Duplicate or empty provider ID: {id}");
        }
        self.providers.insert(id, Box::new(provider));
        Ok(())
    }
    pub fn search(&mut self, context: &SearchContext<'_>, profile: &Profile) -> (Vec<(Rank, Candidate)>, Vec<String>) {
        let matcher = Matcher::new(context.query);
        let mut found = Vec::new();
        let mut errors = Vec::new();
        for (id, provider) in &mut self.providers {
            if (context.cancelled)() {
                break;
            }
            match provider.discover(context) {
                Ok(candidates) => {
                    for candidate in candidates {
                        if let Some(rank) = profile.rank(
                            context.config,
                            &matcher,
                            context.query,
                            &candidate.id,
                            &candidate.title,
                            &candidate.keywords,
                        ) {
                            found.push((rank, candidate));
                        }
                    }
                }
                Err(e) => errors.push(format!("{id}: {e}")),
            }
        }
        found.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.title.cmp(&b.1.title)).then(a.1.id.cmp(&b.1.id)));
        (found, errors)
    }
}
pub struct Applications(pub Vec<(String, String, String)>);
impl Provider for Applications {
    fn id(&self) -> &str {
        "applications"
    }
    fn discover(&mut self, _: &SearchContext<'_>) -> Result<Vec<Candidate>> {
        Ok(self
            .0
            .iter()
            .map(|(id, title, keywords)| Candidate {
                id: format!("app:{id}"),
                title: title.clone(),
                subtitle: String::new(),
                keywords: keywords.clone(),
                target: Target::App(id.clone()),
            })
            .collect())
    }
}
/// A filename-only fallback cache; refreshed on SQLite data_version or scope changes.
#[derive(Default)]
pub struct Files {
    conn: Option<rusqlite::Connection>,
    version: Option<i64>,
    scope: String,
    entries: Vec<Candidate>,
}
impl Provider for Files {
    fn id(&self) -> &str {
        "files"
    }
    fn discover(&mut self, ctx: &SearchContext<'_>) -> Result<Vec<Candidate>> {
        if self.conn.is_none() {
            self.conn = Some(crate::index::open(ctx.paths)?);
        }
        let conn = self.conn.as_ref().unwrap();
        let stamp = format!("{:?}:{:?}", ctx.config.roots, ctx.config.excluded);
        let count: i64 = conn.query_row("PRAGMA data_version", [], |r| r.get(0))?;
        if self.version != Some(count) || self.scope != stamp {
            let mut stmt = conn.prepare("SELECT path,name FROM files")?;
            self.entries.clear();
            for item in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
                if (ctx.cancelled)() {
                    self.version = None;
                    return Ok(vec![]);
                }
                let (path, name) = item?;
                let path = std::path::PathBuf::from(path);
                if !crate::config::forbidden(&path)
                    && ctx.config.roots.iter().any(|root| path.starts_with(root))
                    && !ctx.config.excluded.iter().any(|root| path.starts_with(root))
                {
                    self.entries.push(Candidate {
                        id: format!("file:{}", path.display()),
                        title: name,
                        subtitle: path.parent().unwrap_or(&path).display().to_string(),
                        keywords: String::new(),
                        target: Target::File(path),
                    });
                }
            }
            self.scope = stamp;
            self.version = Some(count);
        }
        let matcher = Matcher::new(ctx.query);
        // Avoid cloning a large catalogue. The registry applies the same ranker after this filter.
        let learned = Profile::load(ctx.paths, ctx.config).unwrap_or_default();
        let mut matched: Vec<_> = self
            .entries
            .iter()
            .take_while(|_| !(ctx.cancelled)())
            .filter_map(|c| learned.rank(ctx.config, &matcher, ctx.query, &c.id, &c.title, "").map(|rank| (rank, c)))
            .collect();
        matched.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.title.cmp(&b.1.title)));
        Ok(matched
            .into_iter()
            .filter(|(_, c)| match &c.target {
                Target::File(path) => crate::config::allowed(ctx.config, path),
                _ => false,
            })
            .take(30)
            .map(|(_, c)| c.clone())
            .collect())
    }
}
/// Read only the currently running, explicitly supported desktop service owners.
/// No activation or method invocation occurs during discovery.
pub struct DesktopServices;
impl Provider for DesktopServices {
    fn id(&self) -> &str {
        "desktop-services"
    }
    fn discover(&mut self, ctx: &SearchContext<'_>) -> Result<Vec<Candidate>> {
        let Some(rest) = ctx.query.strip_prefix("service ") else {
            return Ok(vec![]);
        };
        if rest.trim().is_empty() {
            return Ok(vec![]);
        }
        let mut cmd = std::process::Command::new("systemctl");
        cmd.args(["--user", "list-units", "--type=service", "--all", "--no-pager", "--plain", "--no-legend"]);
        let output = crate::process::capture_cancel(cmd, vec![], std::time::Duration::from_secs(2), 1024 * 1024, ctx.cancelled)?;
        Ok(String::from_utf8(output)?
            .lines()
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                let unit = words.next()?;
                if !unit.ends_with(".service") || !unit.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.@:".contains(&b)) {
                    return None;
                }
                Some(Candidate {
                    id: format!("service:{unit}"),
                    title: format!("Service {unit}"),
                    subtitle: words.collect::<Vec<_>>().join(" "),
                    keywords: String::new(),
                    target: Target::Service(unit.into()),
                })
            })
            .collect())
    }
}

#[derive(Clone, Debug)]
pub enum Effect {
    LaunchApp(String, Option<String>),
    OpenFile(std::path::PathBuf, bool),
    Copy(String),
    Command(Vec<String>, bool),
    Plugin(plugins::Action),
}
pub fn actions(target: &Target) -> Vec<ContextAction> {
    let entries: &[(&str, &str)] = match target {
        Target::File(_) => &[("open", "Open"), ("reveal", "Reveal in Files"), ("copy-path", "Copy path")],
        Target::App(_) => &[("open", "Open")],
        Target::Omarchy(_) => &[("execute", "Execute"), ("details", "View details")],
        Target::Service(_) => &[("details", "View status")],
        Target::Plugin(_, _, _) => &[("open", "Open")],
        _ => &[],
    };
    entries.iter().map(|(id, title)| ContextAction { id: (*id).into(), title: (*title).into() }).collect()
}
/// Resolve an explicit request into an effect; discovery itself cannot execute it.
/// The host performs GTK/clipboard effects on the main thread.
pub fn resolve(paths: &Paths, target: &Target, action: &str) -> Result<Effect> {
    match target {
        Target::File(path) => {
            if !crate::config::allowed(&paths.load()?, path) {
                bail!("Source permission revoked");
            }
            match action {
                "open" => Ok(Effect::OpenFile(path.clone(), false)),
                "reveal" => Ok(Effect::OpenFile(path.clone(), true)),
                "copy-path" => Ok(Effect::Copy(path.display().to_string())),
                _ => bail!("Unsupported file action"),
            }
        }
        Target::App(id) => {
            let app = gio_unix::DesktopAppInfo::new(id).ok_or_else(|| anyhow::anyhow!("Application unavailable"))?;
            if action == "open" {
                return Ok(Effect::LaunchApp(id.clone(), None));
            }
            let desktop = action.strip_prefix("desktop:").ok_or_else(|| anyhow::anyhow!("Unsupported app action"))?;
            if !app.list_actions().iter().any(|a| a == desktop) {
                bail!("Desktop action unavailable");
            }
            Ok(Effect::LaunchApp(id.clone(), Some(desktop.into())))
        }
        Target::Omarchy(id) if action == "execute" => {
            let entry = crate::omarchy::Omarchy::resolve(id)?;
            Ok(match entry.argv {
                Some(args) => Effect::Command(args, entry.confirm),
                None => Effect::Command(vec!["omarchy".into(), "menu".into(), "summon".into(), entry.menu], false),
            })
        }
        Target::Plugin(id, result_id, _) if action.starts_with("context:") => {
            Ok(Effect::Plugin(plugins::contextual_activate(paths, id, result_id, &action[8..])?))
        }
        _ => bail!("Unsupported provider action"),
    }
}
pub struct Extensions;
impl Provider for Extensions {
    fn id(&self) -> &str {
        "plugins"
    }
    fn discover(&mut self, ctx: &SearchContext<'_>) -> Result<Vec<Candidate>> {
        let mut hits = plugins::search_cancel(ctx.paths, ctx.query, ctx.cancelled);
        // Explicit aliases retain the query used to find a plugin result, never its action token.
        // Resolve fresh results from that plugin so expiry and permissions remain effective.
        for (target, alias) in &ctx.config.aliases {
            if alias.to_lowercase() != ctx.query.trim().to_lowercase() {
                continue;
            }
            let Some(rest) = target.strip_prefix("plugin:") else {
                continue;
            };
            let Some((id, hit_id)) = rest.split_once(':') else {
                continue;
            };
            let Some(query) = ctx.config.alias_queries.get(target) else {
                continue;
            };
            if (ctx.cancelled)() {
                break;
            }
            for hit in plugins::search_one(ctx.paths, id, query, ctx.cancelled) {
                if hit.id == hit_id && !hits.iter().any(|(plugin, hit)| plugin == id && hit.id == hit_id) {
                    hits.push((id.into(), hit));
                }
            }
        }
        Ok(hits
            .into_iter()
            .map(|(id, hit)| Candidate {
                id: format!("plugin:{id}:{}", hit.id),
                title: hit.title,
                subtitle: hit.subtitle,
                keywords: String::new(),
                target: Target::Plugin(id, hit.id, hit.action),
            })
            .collect())
    }
}

/// Actions Nyx apps offer through Nyx AI Core's shared registry, the same ones the
/// Assistant uses. Only actions that need no arguments are offered here; AI Core and
/// the owning app check every call, and actions that ask first wait for a second Enter.
#[derive(Default)]
pub struct SharedActions {
    cached: Option<(std::time::Instant, Vec<nyx_ai_client::proto::ActionInfo>)>,
}
impl SharedActions {
    fn list(&mut self, cancelled: &dyn Fn() -> bool) -> Result<&[nyx_ai_client::proto::ActionInfo]> {
        if self.cached.as_ref().is_none_or(|(at, _)| at.elapsed() > std::time::Duration::from_secs(30)) {
            let core = crate::aicore::Core::new()?;
            let actions = if core.available() { core.actions(cancelled)? } else { Vec::new() };
            self.cached = Some((std::time::Instant::now(), actions));
        }
        Ok(&self.cached.as_ref().unwrap().1)
    }
}
/// An action's first sentence, as a title.
pub fn action_title(description: &str, id: &str) -> String {
    let first = description.split(['.', ';', '(']).next().unwrap_or("").trim();
    if first.is_empty() { id.to_string() } else { first.chars().take(70).collect() }
}
/// Whether an action can run from a search result: available, and no required arguments.
pub fn runnable(action: &nyx_ai_client::proto::ActionInfo) -> bool {
    action.available
        && action.owner != "nyx-glimpse"
        && action.spec.input_schema.get("required").and_then(|r| r.as_array()).is_none_or(|r| r.is_empty())
}
impl Provider for SharedActions {
    fn id(&self) -> &str {
        "actions"
    }
    fn discover(&mut self, ctx: &SearchContext<'_>) -> Result<Vec<Candidate>> {
        if ctx.query.trim().chars().count() < 2 {
            return Ok(vec![]);
        }
        Ok(self
            .list(ctx.cancelled)?
            .iter()
            .filter(|a| runnable(a))
            .map(|a| {
                let app = a.spec.id.split('.').next().unwrap_or("");
                Candidate {
                    id: format!("action:{}", a.spec.id),
                    title: action_title(&a.spec.description, &a.spec.id),
                    subtitle: format!("{} action", capitalized(app)),
                    keywords: format!("{} {} {}", app, a.spec.id.replace(['.', '_'], " "), a.spec.description),
                    target: Target::Shared(a.spec.id.clone()),
                }
            })
            .collect())
    }
}
fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}
/// What running a shared action gave, as one line for the status bar.
pub fn summary(value: &serde_json::Value) -> String {
    let text = match value {
        serde_json::Value::Null => return String::new(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(o) if o.len() == 1 && o.values().next().is_some_and(|v| v.is_string()) => {
            o.values().next().and_then(|v| v.as_str()).unwrap_or_default().to_string()
        }
        other => other.to_string(),
    };
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > 160 { format!("{}…", line.chars().take(159).collect::<String>()) } else { line }
}

#[cfg(test)]
mod shared_tests {
    use super::*;

    fn action(id: &str, owner: &str, schema: serde_json::Value) -> nyx_ai_client::proto::ActionInfo {
        serde_json::from_value(serde_json::json!({"id": id, "description": "What Theater is playing: title, position. More.",
            "input_schema": schema, "owner": owner, "source": "dbus", "available": true}))
        .unwrap()
    }

    #[test]
    fn only_actions_without_arguments_run_from_a_result() {
        assert!(runnable(&action("theater.now_playing", "io.github.design_nexus.nyx.Theater", serde_json::json!({"type": "object"}))));
        assert!(!runnable(&action("theater.play", "x", serde_json::json!({"type": "object", "required": ["id"]}))));
        assert!(!runnable(&action("glimpse.search", "nyx-glimpse", serde_json::json!({"type": "object"}))));
        assert_eq!(action_title("What Theater is playing: title, position. More.", "t.n"), "What Theater is playing: title, position");
        assert_eq!(summary(&serde_json::json!({"text": "Paused at\n12:04"})), "Paused at 12:04");
        assert_eq!(summary(&serde_json::Value::Null), "");
    }
}
