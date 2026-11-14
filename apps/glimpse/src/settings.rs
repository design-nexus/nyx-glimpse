use crate::{
    config::{self, Config, Paths},
    integration,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    path::Path,
    process::{Command, Stdio},
};
const PAGE: &str = "glimpse";
fn preview_supported() -> bool {
    std::env::var("SETTINGS_KINDS").is_ok_and(|k| k.split(' ').any(|x| x == "preview"))
}
fn tilde(path: &Path) -> String {
    std::env::var_os("HOME")
        .and_then(|home| path.strip_prefix(home).ok().map(|rest| format!("~/{}", rest.display())))
        .unwrap_or_else(|| path.display().to_string())
}
fn button(key: &str, title: &str, label: &str, confirm: &str) -> Value {
    json!({"kind":"button","key":key,"title":title,"label":label,"confirm":confirm,"refresh":true})
}
fn switch(key: &str, title: &str, value: bool) -> Value {
    json!({"kind":"switch","key":key,"title":title,"value":value,"refresh":true})
}
fn info(title: &str, desc: &str) -> Value {
    json!({"kind":"info","title":title,"value":desc})
}
fn entry(key: &str, title: &str, value: &str, placeholder: &str) -> Value {
    json!({"kind":"entry","key":key,"title":title,"value":value,"placeholder":placeholder,"refresh":true})
}
fn group(title: &str, rows: Vec<Value>) -> Value {
    json!({"title":title,"rows":rows})
}
fn shortcuts_enabled(p: &Paths) -> bool {
    fs::read_to_string(p.state.join("hypr-block.lua")).is_ok_and(|s| s.contains("o.rebind"))
}
fn service_enabled() -> bool {
    Command::new("systemctl").args(["--user", "is-enabled", "nyx-glimpsed.service"]).output().is_ok_and(|o| o.status.success())
}
pub fn describe(p: &Paths, page: &str) -> Result<Value> {
    describe_page(p, page, preview_supported())
}
/// `style_a`: Settings draws row icons and the preview pane, so the page has both.
pub fn describe_page(p: &Paths, page: &str, style_a: bool) -> Result<Value> {
    // Accept old IDs for an already-open Settings page during an upgrade, but advertise only one page.
    if page != PAGE && !page.starts_with("glimpse-") {
        bail!("unknown Settings page");
    }
    let c = p.load()?;
    let suite_path = p.config.parent().context("config parent")?.join("nyx/theme.toml");
    let suite: nyx_theme::Suite = fs::read_to_string(&suite_path).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default();
    let theme_options: Vec<_> = nyx_theme::all().into_iter().map(|t| (t.id, t.name)).collect();
    let service_enabled = service_enabled();
    let mut sources = vec![button("add-folder", "Search folders", "Add folder", "")];
    for root in &c.roots {
        let full = root.display().to_string();
        let display = tilde(root);
        let title = root.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| full.clone());
        sources.push(json!({"kind":"disclosure","key":format!("folder/{full}"),"title":title,"desc":display,"keywords":full,"rows":[
            switch(&format!("code-root/{full}"), "Semantic code search", c.code_roots.contains(root)),
            button(&format!("remove-root/{full}"), "Remove from search", "Remove", "Remove this folder and its indexed content?")
        ]}));
    }
    if c.roots.is_empty() {
        sources.push(info("No folders selected", "Applications and utilities work without indexed folders."));
    }
    let mut excluded = vec![entry("exclude", "Add exclusion", "", "Absolute folder path")];
    for root in &c.excluded {
        excluded.push(button(&format!("remove-exclude/{}", root.display()), &root.display().to_string(), "Remove", ""));
    }
    sources.push(json!({"kind":"disclosure","key":"exclusions","title":"Excluded folders","desc":format!("{} exclusions",c.excluded.len()),"rows":excluded}));
    let mut ai = vec![
        json!({"kind":"switch","key":"semantic","title":"Enable local AI","desc":"Search file contents and media by meaning with EmbeddingGemma 2 in Nyx AI Core, on this computer. If Glimpse doesn't already have the model, AI Core downloads it once (about 865 MB). Word matches in contents work without it.","value":c.semantic_requested||c.semantic,"refresh":true}),
        json!({"kind":"info","key":"ai-status","title":"Status","value":crate::ai_setup::status(p)}),
    ];
    if crate::ai_setup::failed(p) {
        ai.push(button("ai-retry", "Setup could not finish", "Retry", ""));
    }
    // Following Omarchy's theme is a choice only where Omarchy's theme exists (not on
    // NyxOS, where it would leave every Nyx app on the fallback theme).
    let omarchy = nyx_theme::omarchy_available();
    let follow = omarchy && matches!(suite.mode, nyx_theme::ThemeMode::Omarchy);
    let mut groups = vec![
        group(
            "General",
            vec![
                switch("preview", "Show previews", c.preview),
                switch("learning", "Learn from search selections", c.learning),
                button(
                    "clear-learning",
                    "Search history and learned ranking",
                    "Clear",
                    "Delete search history and learned terms? Favorites and aliases are kept.",
                ),
                switch("paused", "Pause indexing", c.paused),
                switch("service", "Start indexing at login", service_enabled),
            ],
        ),
        group(
            "Shortcuts",
            vec![
                switch("shortcuts", "Use Glimpse shortcuts", shortcuts_enabled(p)),
                entry("action_key", "Contextual actions", &c.action_key, "<Control>k"),
                entry("favorite_key", "Toggle favorite", &c.favorite_key, "<Control><Shift>f"),
                json!({"kind":"entry","key":"search_key","title":"Search shortcut","value":c.search_key,"placeholder":"SUPER + SPACE","disabled":!shortcuts_enabled(p),"refresh":true}),
            ]
            .into_iter()
            .chain(omarchy.then(|| json!({"kind":"entry","key":"menu_key","title":"Omarchy menu shortcut","value":c.menu_key,"placeholder":"CTRL + SPACE","disabled":!shortcuts_enabled(p),"refresh":true})))
            .collect(),
        ),
        group("Sources", sources),
        group("Local AI", ai),
        group("Plugins", vec![button("plugins", "Manage plugins", "Open", "")]),
        group(
            "Appearance",
            omarchy
                .then(|| switch("follow_omarchy", "Follow Omarchy", follow))
                .into_iter()
                .chain([json!({"kind":"choice","key":"theme","title":"Theme","value":suite.theme,"options":theme_options,"disabled":follow,"refresh":true})])
                .collect(),
        ),
    ];
    for g in &mut groups {
        if g["rows"].as_array().is_some_and(|rows| rows.len() > 3) {
            g["collapsible"] = true.into();
            g["open_by_default"] = true.into();
        }
    }
    groups[2]["note"] = format!("{} folders · {} exclusions", c.roots.len(), c.excluded.len()).into();
    if !style_a {
        return Ok(json!({"poll":2,"groups":groups}));
    }
    let plugins = crate::plugins::list(p).unwrap_or_default();
    let on: Vec<&str> = plugins.iter().filter(|x| x.grant.enabled).map(|x| x.manifest.name.as_str()).collect();
    for row in groups.iter_mut().filter_map(|g| g.get_mut("rows").and_then(Value::as_array_mut)).flatten() {
        style_row(row, omarchy, on.len());
    }
    groups[3]["note"] = "EmbeddingGemma 2 in Nyx AI Core searches file contents and media by meaning, on this computer. If Glimpse doesn't already have the model, AI Core downloads it once (about 865 MB). Word matches in contents work without it.".into();
    let preview = preview(p, &c, shortcuts_enabled(p), &on, plugins.len());
    Ok(json!({"poll":2,"groups":groups,"preview":preview}))
}
/// The row's icon and a description short enough for one line beside its control.
fn style_row(row: &mut Value, omarchy: bool, plugins_on: usize) {
    for inner in row.get_mut("rows").and_then(Value::as_array_mut).into_iter().flatten() {
        style_row(inner, omarchy, plugins_on);
    }
    let key = row["key"].as_str().unwrap_or_default().to_owned();
    let s = |text: &str| Some(text.to_owned());
    let (icon, desc) = match key.as_str() {
        "preview" => ("view-reveal-symbolic", s("A preview beside the selected result")),
        "learning" => {
            row["title"] = "Learn from what you pick".into();
            ("view-sort-descending-symbolic", s("Ranks what you open often higher"))
        }
        "clear-learning" => ("document-open-recent-symbolic", s("Favorites and aliases are kept")),
        "paused" => ("media-playback-pause-symbolic", s("Folders aren't scanned while paused")),
        "service" => ("system-shutdown-symbolic", s("Runs the indexer when you sign in")),
        "shortcuts" => {
            ("input-keyboard-symbolic", s(if omarchy { "Instead of Omarchy's launcher" } else { "Use the shortcuts below" }))
        }
        "search_key" => ("system-search-symbolic", s("Opens Glimpse")),
        "menu_key" => ("open-menu-symbolic", s("Opens Omarchy's menu")),
        "action_key" => ("input-keyboard-symbolic", s("Actions for the selected result")),
        "favorite_key" => ("starred-symbolic", s("Favorites rank above other matches")),
        "add-folder" => ("folder-new-symbolic", s("Index another folder")),
        k if k.starts_with("folder/") => {
            let path = row["desc"].as_str().unwrap_or_default();
            let code = row["rows"][0]["value"] == true;
            ("folder-symbolic", Some(if code { format!("{path} · code search on") } else { path.to_owned() }))
        }
        k if k.starts_with("code-root/") => ("utilities-terminal-symbolic", s("Search code here by meaning")),
        k if k.starts_with("remove-root/") => ("user-trash-symbolic", s("Deletes what was indexed from it")),
        "exclusions" => ("view-conceal-symbolic", row["desc"].as_str().map(String::from)),
        "exclude" => ("list-add-symbolic", s("Skip a folder inside a source")),
        k if k.starts_with("remove-exclude/") => ("folder-symbolic", s("Not indexed")),
        "semantic" => {
            row["title"] = "Search by meaning".into();
            ("find-location-symbolic", s("Local AI, on this computer"))
        }
        "ai-status" => ("dialog-information-symbolic", None),
        "ai-retry" => ("view-refresh-symbolic", s("Try the setup again")),
        "plugins" => ("application-x-addon-symbolic", Some(format!("{plugins_on} turned on"))),
        "follow_omarchy" => ("preferences-desktop-appearance-symbolic", s("Use Omarchy's theme")),
        "theme" => ("color-select-symbolic", s(if omarchy { "While not following Omarchy" } else { "Shared by every Nyx app" })),
        "" => ("folder-symbolic", None),
        _ => ("emblem-system-symbolic", None),
    };
    row["icon"] = icon.into();
    if let Some(desc) = desc {
        row["desc"] = desc.into();
    }
}
fn files(n: i64) -> String {
    let digits = n.to_string();
    let mut grouped = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("{grouped} {}", if n == 1 { "file" } else { "files" })
}
fn preview(p: &Paths, c: &Config, shortcuts: bool, plugins_on: &[&str], installed: usize) -> Value {
    let counts: Option<HashMap<String, i64>> = crate::index::open(p).ok().and_then(|conn| {
        let mut query = conn.prepare("SELECT root, count(*) FROM files GROUP BY root").ok()?;
        let counts = query.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).ok()?.collect::<rusqlite::Result<_>>().ok();
        counts
    });
    let in_root = |root: &Path| counts.as_ref().map(|m| m.get(&*root.to_string_lossy()).copied().unwrap_or(0));
    // The daemon's last scan lists the folders it couldn't read.
    let unavailable: Vec<String> = fs::read(p.state.join("index-status.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v["unavailable"].as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect()))
        .unwrap_or_default();
    let mut stats = Vec::new();
    if counts.is_some() {
        let total = c.roots.iter().filter_map(|r| in_root(r)).sum();
        let folders = format!("{} folder{}", c.roots.len(), if c.roots.len() == 1 { "" } else { "s" });
        stats.push(json!({"label":"Indexed","value":files(total),"sub":if c.paused { format!("{folders} · paused") } else { folders },"tone":if c.paused { "warn" } else { "" }}));
    }
    let status = crate::ai_setup::status(p);
    let (ai, sub, tone) = match status.split_once(" · ") {
        _ if status == "Disabled" => ("Off", "Word matches only".to_owned(), ""),
        Some(("Ready", rest)) => {
            let mut rest = rest.chars();
            ("Ready", rest.next().map(|f| f.to_uppercase().chain(rest).collect()).unwrap_or_default(), "good")
        }
        _ if crate::ai_setup::failed(p) => ("Failed", "Retry under Local AI".to_owned(), "bad"),
        _ => ("Setting up", status.clone(), "accent"),
    };
    stats.push(json!({"label":"Local AI","value":ai,"sub":sub,"tone":tone}));
    let names = match plugins_on {
        [] => format!("{installed} installed"),
        [a] => (*a).to_owned(),
        [a, b] => format!("{a}, {b}"),
        [a, b, ..] => format!("{a}, {b}, …"),
    };
    stats.push(json!({"label":"Plugins","value":format!("{} on", plugins_on.len()),"sub":names}));
    let mut blocks = vec![json!({"kind":"stats","items":stats})];
    if c.roots.is_empty() {
        blocks.push(json!({"kind":"caption","text":"No folders are indexed. Applications and utilities work without them."}));
    } else {
        const SHOWN: usize = 6;
        let items: Vec<Value> = c
            .roots
            .iter()
            .take(SHOWN)
            .map(|root| {
                let name =
                    root.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
                let mut sub = tilde(root);
                if c.code_roots.contains(root) {
                    sub.push_str(" · code search");
                }
                let missing = unavailable.iter().any(|u| u.starts_with(&format!("{}: ", root.display())));
                let (value, tone) =
                    if missing { ("Unavailable".to_owned(), "bad") } else { (in_root(root).map(files).unwrap_or_default(), "") };
                json!({"icon":"folder-symbolic","label":name,"sub":sub,"value":value,"tone":tone})
            })
            .collect();
        blocks.push(json!({"kind":"list","title":"Indexed folders","items":items}));
        if let Some(more) = c.roots.len().checked_sub(SHOWN).filter(|&n| n > 0) {
            let s = if more == 1 { "" } else { "s" };
            blocks.push(json!({"kind":"caption","text":format!("{more} more folder{s} under Sources.")}));
        }
    }
    let keys: Vec<String> = c
        .search_key
        .split('+')
        .map(|k| {
            let mut k = k.trim().chars();
            k.next().map(|f| f.to_uppercase().chain(k.flat_map(char::to_lowercase)).collect()).unwrap_or_default()
        })
        .collect();
    let title = if shortcuts { format!("What {} opens", keys.join(" ")) } else { "What Glimpse searches".to_owned() };
    let line = format!(
        "Apps, files, commands, quick answers and plugins{}",
        if c.learning { "; it learns what you pick." } else { "." }
    );
    let foot = if c.battery_pause {
        format!("Indexing pauses on battery power and when a temperature sensor reaches {} °C.", c.thermal_limit)
    } else {
        format!("Indexing pauses when a temperature sensor reaches {} °C.", c.thermal_limit)
    };
    json!({"title":title,"line":line,"blocks":blocks,"foot":foot,"foot_icon":"dialog-information-symbolic"})
}
fn launch(args: &[&str]) -> Result<()> {
    Command::new(std::env::current_exe()?.with_file_name("nyx-glimpse"))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}
fn start_job(p: &Paths, op: &str) -> Result<()> {
    use std::os::fd::AsRawFd;
    p.ensure()?;
    let lock = fs::OpenOptions::new().create(true).truncate(false).write(true).open(p.state.join("settings-job.lock"))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!("Another operation is already running");
    }
    Command::new(std::env::current_exe()?.with_file_name("nyx-glimpse-settings"))
        .args(["run-job", op])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}
fn run_job(p: &Paths, op: &str) -> Result<()> {
    use std::os::fd::AsRawFd;
    p.ensure()?;
    let lock = fs::OpenOptions::new().create(true).truncate(false).write(true).open(p.state.join("settings-job.lock"))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!("Could not lock background operation");
    }
    let args: &[&str] = match op {
        "runtime" | "models" => &["models", "setup"],
        "rescan" => &["index", "scan"],
        "purge" => &["index", "purge"],
        _ => bail!("unknown background operation"),
    };
    let file = p.state.join("settings-job.json");
    let message = match op {
        "runtime" | "models" => "Making EmbeddingGemma 2 available in Nyx AI Core…",
        "rescan" => "Refreshing indexed folders…",
        _ => "Deleting search indexes…",
    };
    config::atomic_write(&file, &serde_json::to_vec(&json!({"running":true,"pid":std::process::id(),"message":message}))?)?;
    let result = crate::cli::dispatch(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    let message = match &result {
        Ok(_) => format!(
            "{} complete",
            match op {
                "runtime" => "Runtime setup",
                "models" => "Model download",
                "rescan" => "Folder refresh",
                _ => "Index deletion",
            }
        ),
        Err(e) => format!("Operation failed: {e:#}"),
    };
    config::atomic_write(&file, &serde_json::to_vec(&json!({"running":false,"message":message}))?)?;
    result.map(|_| ())
}
pub fn dispatch(p: &Paths, args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("pages") => println!(
            "{}",
            json!([{"id":PAGE,"title":"Glimpse","icon":"system-search-symbolic","description":"Search, indexing, local AI, and plugins.","keywords":"glimpse search launcher plugins"}])
        ),
        Some("describe") => println!("{}", describe(p, args.get(1).context("page missing")?)?),
        Some("theme-changed") => {}
        Some("run-ai-setup") => crate::ai_setup::run(p)?,
        Some("run-job") => run_job(p, args.get(1).context("operation missing")?)?,
        Some("set") => {
            let page = args.get(1).context("page missing")?;
            if page != PAGE && !page.starts_with("glimpse-") {
                bail!("unknown page");
            }
            let key = args.get(2).context("setting missing")?;
            let value = args.get(3).map(String::as_str).unwrap_or("");
            if key == "semantic" || key == "ai-retry" {
                crate::ai_setup::request(p, if key == "ai-retry" { true } else { value.parse()? })?;
                println!("{}", json!({"refresh":true}));
                return Ok(());
            }
            let _lock = crate::ai_setup::config_lock(p)?;
            let mut c = p.load()?;
            match key.as_str() {
                "learning" => {
                    c.learning = value.parse()?;
                    p.save(&c)?;
                }
                "clear-learning" => {
                    crate::learning::clear(p)?;
                }
                "action_key" => {
                    c.action_key = value.into();
                    p.save(&c)?;
                }
                "favorite_key" => {
                    c.favorite_key = value.into();
                    p.save(&c)?;
                }
                "paused" => {
                    c.paused = value.parse()?;
                    p.save(&c)?;
                }
                "battery_pause" => {
                    c.battery_pause = value.parse()?;
                    p.save(&c)?;
                }
                "preview" => {
                    c.preview = value.parse()?;
                    p.save(&c)?;
                }
                "thermal_limit" => {
                    let n: f64 = value.parse()?;
                    if !n.is_finite() || n.fract() != 0. {
                        bail!("Temperature must be a whole number");
                    }
                    c.thermal_limit = n as u32;
                    p.save(&c)?;
                }
                "device" => {
                    c.device = value.into();
                    p.save(&c)?;
                }
                "mode" | "theme" | "follow_omarchy" => {
                    let file = p.config.parent().context("config parent")?.join("nyx/theme.toml");
                    let mut suite: nyx_theme::Suite =
                        fs::read_to_string(&file).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default();
                    if key == "mode" || key == "follow_omarchy" {
                        let value = if key == "follow_omarchy" {
                            if value.parse::<bool>()? { "omarchy" } else { "theme" }
                        } else {
                            value
                        };
                        suite.mode = match value {
                            "omarchy" => nyx_theme::ThemeMode::Omarchy,
                            "theme" => nyx_theme::ThemeMode::Theme,
                            _ => bail!("Unknown theme source"),
                        };
                        c.mode = value.into();
                    } else {
                        if !nyx_theme::all().iter().any(|t| t.id == value) {
                            bail!("Unknown Nyx theme");
                        }
                        suite.theme = value.into();
                        c.theme = value.into();
                    }
                    p.save(&c)?;
                    config::atomic_write(&file, toml::to_string_pretty(&suite)?.as_bytes())?;
                }
                "search_key" | "menu_key" => {
                    let original = c.clone();
                    if key == "search_key" {
                        c.search_key = value.into();
                    } else {
                        c.menu_key = value.into();
                    }
                    p.save(&c)?;
                    if shortcuts_enabled(p)
                        && let Err(e) = integration::apply(p, Some(true))
                    {
                        p.save(&original)?;
                        return Err(e);
                    }
                }
                "rates_date" => {
                    if !value.trim().is_empty() && !config::valid_date(value.trim()) {
                        bail!("Use a valid YYYY-MM-DD date");
                    }
                    c.rates_date = value.trim().into();
                    p.save(&c)?;
                }
                "rates" => {
                    c.rates.clear();
                    if !value.trim().is_empty() {
                        for pair in value.split(',') {
                            let (name, rate) =
                                pair.trim().split_once('=').context("Use currency rates such as USD=1, EUR=0.92")?;
                            let name = name.trim().to_uppercase();
                            if name.len() != 3 || !name.bytes().all(|b| b.is_ascii_uppercase()) {
                                bail!("Currency codes must contain three letters");
                            }
                            if c.rates.insert(name, rate.trim().parse()?).is_some() {
                                bail!("Duplicate currency code");
                            }
                        }
                    }
                    p.save(&c)?;
                }
                "service" => integration::user_service(p, value.parse()?)?,
                "shortcuts" => integration::apply(p, Some(value.parse()?))?,
                "shortcuts-on" => integration::apply(p, Some(true))?,
                "shortcuts-off" => integration::apply(p, Some(false))?,
                "add-folder" => launch(&["--add-folder"])?,
                "plugins" => launch(&["--plugins"])?,
                "runtime" | "models" | "rescan" | "purge" => start_job(p, key)?,
                "exclude" => {
                    let root = std::path::PathBuf::from(value);
                    if !c.excluded.contains(&root) {
                        c.excluded.push(root);
                    }
                    p.save(&c)?;
                    crate::index::purge_outside(&crate::index::open(p)?, &c)?;
                    crate::cli::clear_semantic(p)?;
                }
                k if k.starts_with("remove-exclude/") => {
                    let suffix = &k[15..];
                    let i: usize = if suffix.starts_with('/') {
                        c.excluded
                            .iter()
                            .position(|p| p == std::path::Path::new(suffix))
                            .context("Exclusion changed; refresh page")?
                    } else {
                        suffix.parse()?
                    };
                    if i >= c.excluded.len() {
                        bail!("Exclusions changed; refresh page");
                    }
                    c.excluded.remove(i);
                    p.save(&c)?;
                }
                k if k.starts_with("remove-root/") => {
                    let suffix = &k[12..];
                    let i: usize = if suffix.starts_with('/') {
                        c.roots.iter().position(|p| p == std::path::Path::new(suffix)).context("Source changed; refresh page")?
                    } else {
                        suffix.parse()?
                    };
                    if i >= c.roots.len() {
                        bail!("Sources changed; refresh page");
                    }
                    c.roots.remove(i);
                    c.code_roots.retain(|r| c.roots.iter().any(|p| r.starts_with(p)));
                    p.save(&c)?;
                    crate::index::purge_outside(&crate::index::open(p)?, &c)?;
                    crate::cli::clear_semantic(p)?;
                }
                k if k.starts_with("code-root/") => {
                    let suffix = &k[10..];
                    let i: usize = if suffix.starts_with('/') {
                        c.roots.iter().position(|p| p == std::path::Path::new(suffix)).context("Source changed; refresh page")?
                    } else {
                        suffix.parse()?
                    };
                    let root = c.roots.get(i).context("Sources changed")?.clone();
                    c.code_roots.retain(|r| r != &root);
                    if value.parse::<bool>()? {
                        c.code_roots.push(root);
                    }
                    p.save(&c)?;
                }
                _ => bail!("unknown preference"),
            }
            println!(
                "{}",
                json!({"refresh":true,"toast":if matches!(key.as_str(),"runtime"|"models"|"rescan"|"purge") {"Operation started. Progress appears under Diagnostics."} else {"Glimpse updated"}})
            );
        }
        _ => bail!("unknown Settings operation"),
    }
    Ok(())
}
