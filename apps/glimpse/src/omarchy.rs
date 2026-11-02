//! Installed Omarchy menu metadata is searchable data, never an implicit shell program.
use crate::{
    process,
    providers::{Candidate, Provider, SearchContext, Target},
};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};
#[derive(Clone, Debug)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub keywords: String,
    pub argv: Option<Vec<String>>,
    pub menu: String,
    pub confirm: bool,
    pub current: bool,
}
#[derive(Default)]
pub struct Omarchy {
    pub entries: Vec<Entry>,
    refreshed: Option<Instant>,
    stamp: String,
}
pub fn result_id(id: &str) -> String {
    match id {
        "system.lock" => "system:lock".into(),
        "system.logout" => "system:logout".into(),
        "system.reboot" => "system:reboot".into(),
        "system.shutdown" => "system:shutdown".into(),
        _ => format!("omarchy:{id}"),
    }
}
fn root() -> PathBuf {
    std::env::var_os("OMARCHY_PATH").map(PathBuf::from).unwrap_or_else(|| "/usr/share/omarchy".into())
}
fn user_menu() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"))
        .join("omarchy/extensions/omarchy-menu.jsonc")
}
/// JSONC removal understands strings, including URLs, escaped quotes and literal commas.
pub fn parse_jsonc(raw: &str) -> Result<Value> {
    let mut clean = String::new();
    let mut chars = raw.chars().peekable();
    let mut quoted = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if quoted {
            clean.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = false;
            }
            continue;
        }
        if ch == '"' {
            quoted = true;
            clean.push(ch);
        } else if ch == '/' && chars.peek() == Some(&'/') {
            for c in chars.by_ref() {
                if c == '\n' {
                    clean.push('\n');
                    break;
                }
            }
        } else if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut closed = false;
            while let Some(c) = chars.next() {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    closed = true;
                    break;
                }
            }
            if !closed {
                bail!("Unclosed JSONC comment");
            }
            clean.push(' ');
        } else {
            clean.push(ch);
        }
    }
    let chars: Vec<char> = clean.chars().collect();
    let mut out = String::new();
    quoted = false;
    escaped = false;
    for (i, ch) in chars.iter().copied().enumerate() {
        if !quoted && ch == ',' && chars[i + 1..].iter().find(|c| !c.is_whitespace()).is_some_and(|c| *c == '}' || *c == ']') {
            continue;
        }
        out.push(ch);
        if escaped {
            escaped = false;
        } else if quoted && ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            quoted = !quoted;
        }
    }
    Ok(serde_json::from_str(&out)?)
}
fn capture(argv: &[&str], cancel: &dyn Fn() -> bool) -> Result<String> {
    let mut cmd = Command::new(argv[0]);
    cmd.args(&argv[1..]);
    Ok(String::from_utf8(process::capture_cancel(cmd, vec![], Duration::from_secs(2), 2 * 1024 * 1024, cancel)?)?)
}
/// Accept shell-free argument syntax only. Query text is never part of this input.
pub fn argv(text: &str) -> Option<Vec<String>> {
    let mut result = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    for ch in text.chars() {
        if ['$', '`', ';', '|', '&', '>', '<', '\n', '\r', '\\'].contains(&ch) {
            return None;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                word.push(ch);
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if ch.is_whitespace() {
            if !word.is_empty() {
                result.push(std::mem::take(&mut word));
            }
        } else {
            word.push(ch);
        }
    }
    if quote.is_some() {
        return None;
    }
    if !word.is_empty() {
        result.push(word);
    }
    (!result.is_empty()).then_some(result)
}
fn direct(text: &str, routes: &BTreeMap<String, Vec<String>>) -> Option<Vec<String>> {
    let mut args = argv(text)?;
    if let Some(route) = routes.get(&args[0]) {
        let mut out = route.clone();
        out.extend(args.drain(1..));
        args = out;
    }
    if args.first().map(String::as_str) != Some("omarchy") {
        return None;
    }
    // Known GUI/interactive entrypoints and bounded setting changes. No generic exec.
    if !args.get(1).is_some_and(|s| {
        ["launch", "menu", "toggle", "theme", "font", "powerprofiles", "kernel", "system", "shell"].contains(&s.as_str())
    }) {
        return None;
    }
    Some(args)
}
fn parent(id: &str) -> String {
    id.rsplit_once('.').map(|(p, _)| p).unwrap_or("root").into()
}
fn supported_guard(expr: &str, memo: &mut BTreeMap<String, bool>, cancelled: &dyn Fn() -> bool) -> Option<bool> {
    if expr.trim().is_empty() {
        return Some(true);
    }
    let expr = expr.trim();
    let (neg, body) = expr.strip_prefix("! ").map(|s| (true, s)).unwrap_or((false, expr));
    let args = argv(body)?;
    if args.len() != 2 || args[0] != "omarchy-toggle-enabled" || !args[1].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return None;
    }
    let value =
        *memo.entry(body.into()).or_insert_with(|| capture(&["omarchy", "toggle", "enabled", &args[1]], cancelled).is_ok());
    Some(value != neg)
}
impl Omarchy {
    pub fn refresh(&mut self, force: bool, cancelled: &dyn Fn() -> bool) -> Result<()> {
        let default = root().join("default/omarchy/omarchy-menu.jsonc");
        let user = user_menu();
        let stamp = format!(
            "{:?}:{:?}",
            std::fs::metadata(&default).and_then(|m| m.modified()),
            std::fs::metadata(&user).and_then(|m| m.modified())
        );
        if !force && self.stamp == stamp && self.refreshed.is_some_and(|t| t.elapsed() < Duration::from_secs(5)) {
            return Ok(());
        }
        let metadata = capture(&["omarchy", "commands", "--json"], cancelled)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .unwrap_or(Value::Null);
        let mut routes = BTreeMap::new();
        for command in metadata["commands"].as_array().into_iter().flatten() {
            if let (Some(binary), Some(route)) = (command["binary"].as_str(), command["route"].as_str()) {
                routes.insert(binary.to_string(), route.split_whitespace().map(String::from).collect());
            }
        }
        let mut items = BTreeMap::<String, Value>::new();
        for path in [&default, &user] {
            match std::fs::read_to_string(path) {
                Ok(raw) => {
                    let parsed = parse_jsonc(&raw)?;
                    let obj = parsed.get("items").unwrap_or(&parsed).as_object().context("Menu must be an object")?;
                    for (id, value) in obj {
                        if value.is_object() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)) {
                            items.insert(id.clone(), value.clone());
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        let mut entries = Vec::new();
        let mut memo = BTreeMap::new();
        let mut dynamic_menus = BTreeSet::new();
        for (id, item) in &items {
            if cancelled() {
                bail!("Discovery cancelled");
            }
            let text = |key: &str| item[key].as_str().unwrap_or("");
            let label = if text("label").is_empty() { id.as_str() } else { text("label") };
            let mut chain = Vec::new();
            let mut next = id.clone();
            let mut seen = BTreeSet::new();
            let mut state = true;
            let mut known = true;
            while seen.insert(next.clone()) {
                let Some(ancestor) = items.get(&next) else {
                    break;
                };
                if next != *id {
                    chain.push(ancestor["label"].as_str().unwrap_or(&next).to_string());
                }
                match supported_guard(ancestor["when"].as_str().unwrap_or(""), &mut memo, cancelled) {
                    Some(false) => state = false,
                    None => known = false,
                    _ => {}
                }
                next = ancestor["parent"].as_str().map(String::from).unwrap_or_else(|| parent(&next));
                if next == "root" {
                    break;
                }
            }
            if !state {
                continue;
            }
            let disabled =
                if text("disabled").is_empty() { Some(false) } else { supported_guard(text("disabled"), &mut memo, cancelled) };
            if disabled == Some(true) {
                continue;
            }
            known &= disabled.is_some();
            if known {
                dynamic_menus.insert(id.clone());
            }
            chain.reverse();
            let route = if text("action").is_empty() { None } else { direct(text("action"), &routes).filter(|_| known) };
            let menu = if text("action").is_empty() && text("target").is_empty() {
                id.clone()
            } else if !text("target").is_empty() {
                text("target").to_string()
            } else {
                parent(id)
            };
            let detail = format!("{}{}", chain.join(" › "), if route.is_none() { " · Open in Omarchy menu" } else { "" });
            let current = !text("checked").is_empty() && supported_guard(text("checked"), &mut memo, cancelled) == Some(true);
            entries.push(Entry {
                id: id.clone(),
                title: label.into(),
                detail,
                keywords: format!("{} {} {} {}", chain.join(" "), text("description"), item["aliases"], item["searchTerms"]),
                argv: route,
                menu,
                confirm: id.starts_with("system.") && !["system.lock", "system.screensaver"].contains(&id.as_str()),
                current,
            });
        }
        // Dynamic choices use Omarchy's own enumeration and execution routes.
        for (menu, list, current, set) in [
            (
                "appearance.font",
                vec!["omarchy", "font", "list"],
                vec!["omarchy", "font", "current"],
                vec!["omarchy", "font", "set"],
            ),
            (
                "setup.kernel",
                vec!["omarchy", "kernel", "list"],
                vec!["omarchy", "kernel", "get"],
                vec!["omarchy", "kernel", "set"],
            ),
            (
                "setup.power-profile",
                vec!["omarchy", "powerprofiles", "list"],
                vec!["powerprofilesctl", "get"],
                vec!["omarchy", "powerprofiles", "set", "autodetect"],
            ),
        ] {
            let Some((actual, _item)) = items.iter().find(|(_, i)| {
                i["provider"].as_str()
                    == Some(match menu {
                        "appearance.font" => "fonts",
                        "setup.kernel" => "kernels",
                        _ => "power-profiles",
                    })
            }) else {
                continue;
            };
            // A guarded submenu still belongs to Omarchy if we cannot establish availability.
            if !dynamic_menus.contains(actual) {
                continue;
            }
            if let Ok(output) = capture(&list, cancelled) {
                let selected = capture(&current, cancelled).unwrap_or_default();
                for value in output.lines().map(str::trim).filter(|s| !s.is_empty() && s.len() < 256).take(256) {
                    let mut args: Vec<String> = set.iter().map(|s| s.to_string()).collect();
                    args.push(value.into());
                    entries.push(Entry {
                        id: format!("{actual}/{}", value),
                        title: value.into(),
                        detail: format!("{} › {}", actual, value),
                        keywords: actual.replace('.', " "),
                        argv: Some(args),
                        menu: actual.clone(),
                        confirm: menu == "setup.kernel",
                        current: value == selected.trim(),
                    });
                }
            }
        }
        self.entries = entries;
        self.refreshed = Some(Instant::now());
        self.stamp = stamp;
        Ok(())
    }
    pub fn resolve(id: &str) -> Result<Entry> {
        let mut source = Self::default();
        source.refresh(true, &|| false)?;
        source.entries.into_iter().find(|e| e.id == id).context("Omarchy action is no longer available")
    }
}
impl Provider for Omarchy {
    fn id(&self) -> &str {
        "omarchy"
    }
    fn discover(&mut self, ctx: &SearchContext<'_>) -> Result<Vec<Candidate>> {
        self.refresh(false, ctx.cancelled)?;
        Ok(self
            .entries
            .iter()
            .map(|e| Candidate {
                id: result_id(&e.id),
                title: e.title.clone(),
                subtitle: format!("{}{}", e.detail, if e.current { " ✓" } else { "" }),
                keywords: e.keywords.clone(),
                target: Target::Omarchy(e.id.clone()),
            })
            .collect())
    }
}
