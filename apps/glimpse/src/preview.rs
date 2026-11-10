//! What Glimpse shows of a result beside the list, and how it opens one: shared by the
//! GTK panel (Pango markup) and `nyx-glimpse-backend` for the Quickshell panel (HTML for
//! Qt's rich text).

use crate::{config::Paths, plugins, semantic};
use gtk::{gio, glib, prelude::*};
use serde_json::{Value, json};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

/// The markup a preview's text is written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    Pango,
    Html,
}

fn escape(text: &str, flavor: Flavor) -> String {
    match flavor {
        Flavor::Pango => glib::markup_escape_text(text).to_string(),
        Flavor::Html => text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;"),
    }
}

/// Rich text keeps the passage's line breaks and spaces, as the GTK label does.
fn html_block(markup: &str) -> String {
    format!("<div style=\"white-space:pre-wrap\">{markup}</div>")
}

/// A plugin result's preview: where it came from, and the plugin's own text if it has one.
pub fn plugin_preview(id: &str, result_id: &str, title: &str, subtitle: &str, query: &str, flavor: Flavor) -> Preview {
    let p = Paths::discover();
    let plugin = plugins::get(&p, id).ok();
    let mut details = vec![("Plugin".to_string(), plugin.as_ref().map_or(id.to_string(), |p| p.manifest.name.clone()))];
    if !subtitle.is_empty() {
        details.insert(0, ("Result".into(), subtitle.to_string()));
    }
    let text = plugin
        .filter(|plugin| plugin.manifest.capabilities.contains(&"preview".into()))
        .and_then(|_| plugins::call(&p, id, "preview", "preview", json!({"result_id": result_id, "title": title, "query": query})).ok())
        .and_then(|v| v["text"].as_str().map(|s| s.chars().take(16000).collect::<String>()));
    let body = match text {
        Some(t) => {
            let markup = highlight(&t, query, flavor);
            Body::Text(if flavor == Flavor::Html { html_block(&markup) } else { markup })
        }
        None => Body::Icon,
    };
    Preview { body, details }
}

/// A file's preview, or a note when it can't be shown any more.
pub fn file_preview(path: &Path, hit: &Value, query: &str, flavor: Flavor, inline_media: bool) -> Preview {
    let note = |text: &str| Preview {
        body: Body::Text(if flavor == Flavor::Html { html_block(&escape(text, flavor)) } else { escape(text, flavor) }),
        details: vec![],
    };
    let Ok(c) = Paths::discover().load() else {
        return note("Glimpse's preferences could not be read.");
    };
    if !crate::config::allowed(&c, path) {
        return note("This folder is no longer searched.");
    }
    let Ok(meta) = path.metadata() else {
        return note("This file is no longer available.");
    };
    let mut details = file_details(path, &meta, hit);
    let body = file_body(path, hit, query, &mut details, flavor, inline_media);
    Preview { body, details }
}

/// The match's line, page or moment from a content hit, in that order.
pub fn hit_line(hit: &Value) -> Option<u64> {
    hit["info"]["line"].as_u64().or_else(|| hit["focus"].as_u64()).or_else(|| hit["loc"].as_f64().filter(|_| hit["segment"] == "lines").map(|n| n as u64))
}

/// A file's folder for display, with the home folder as "~".
pub fn folder_of(path: &Path) -> String {
    let parent = path.parent().unwrap_or(path);
    match std::env::var_os("HOME").map(PathBuf::from).and_then(|home| parent.strip_prefix(home).ok().map(Path::to_owned)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => parent.display().to_string(),
    }
}

pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.).round() as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// Where a content hit landed, in words: a page, a line or a moment.
pub fn match_place(hit: &Value) -> Option<String> {
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
pub fn match_reason(hit: &Value) -> String {
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

pub fn system_description(cmd: &str) -> &'static str {
    match cmd {
        "lock" => "Lock the screen and turn off the display.",
        "logout" => "Log out after closing application windows.",
        "reboot" => "Restart after closing application windows.",
        _ => "Shut down after closing application windows.",
    }
}

/// What the preview thread hands back: the content to frame, and the details below it.
pub enum Body {
    /// Nothing to show beyond the details: the frame shows the result's icon.
    Icon,
    /// Pango markup for the text frame.
    Text(String),
    /// An encoded image (a picture, a PDF page, a video still).
    Image(Vec<u8>),
    /// Leave the inline player that was attached on selection.
    Media,
}

pub struct Preview {
    pub body: Body,
    pub details: Vec<(String, String)>,
}

/// Kind, size, date, folder and where the match is: never the name, which the list shows.
pub fn file_details(path: &Path, meta: &std::fs::Metadata, hit: &Value) -> Vec<(String, String)> {
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
pub fn when(time: std::time::SystemTime) -> String {
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

pub fn capture_image(cmd: Command) -> Option<Vec<u8>> {
    crate::process::capture(cmd, vec![], Duration::from_secs(3), 6 * 1024 * 1024).ok().filter(|b| !b.is_empty())
}

/// `inline_media`: the panel plays audio and video itself (`Body::Media`); otherwise a
/// video shows a still.
pub fn file_body(path: &Path, hit: &Value, query: &str, details: &mut Vec<(String, String)>, flavor: Flavor, inline_media: bool) -> Body {
    let ext = path.extension().unwrap_or_default().to_string_lossy().to_lowercase();
    if let Some(is_video) = media_kind(path) {
        if let Some(seconds) = media_length(path) {
            details.insert(2, ("Length".into(), clock(seconds)));
        }
        if inline_media {
            return Body::Media;
        }
        // No inline player: a still from a few seconds in, past any black intro.
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
        if let Ok(bytes) = crate::process::capture(cmd, vec![], Duration::from_secs(3), 65536) {
            return text_body(&String::from_utf8_lossy(&bytes), path, hit, query, flavor);
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
        if let Ok(bytes) = crate::process::capture(cmd, vec![], Duration::from_secs(3), 2 * 1024 * 1024) {
            let text = semantic::strip_markup(&String::from_utf8_lossy(&bytes));
            // Office XML has no line breaks worth keeping; collapse the runs of spaces.
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            return text_body(&text, path, hit, query, flavor);
        }
        return Body::Icon;
    }
    if let Ok(f) = std::fs::File::open(path) {
        let mut bytes = Vec::new();
        if f.take(65536).read_to_end(&mut bytes).is_ok() && !bytes.is_empty() && !bytes.contains(&0) {
            return text_body(&String::from_utf8_lossy(&bytes), path, hit, query, flavor);
        }
    }
    Body::Icon
}

/// The passage that matched: the hit's line, the semantic excerpt, or the first line
/// with the search word; the start of the file when nothing points elsewhere.
pub fn text_body(text: &str, path: &Path, hit: &Value, query: &str, flavor: Flavor) -> Body {
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
    let markup = code_markup(&excerpt, path, flavor).unwrap_or_else(|| highlight(&excerpt, query, flavor));
    Body::Text(match flavor {
        Flavor::Pango => markup,
        Flavor::Html => html_block(&markup),
    })
}

pub fn media_length(path: &Path) -> Option<f64> {
    let mut probe = Command::new("ffprobe");
    probe.args(["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1"]).arg(path);
    let bytes = crate::process::capture(probe, vec![], Duration::from_secs(2), 256).ok()?;
    String::from_utf8_lossy(&bytes).trim().parse::<f64>().ok().filter(|n| n.is_finite() && *n > 0.)
}

pub fn open_file(path: &Path, hit: &Value, reveal: bool) -> anyhow::Result<()> {
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

pub fn highlight(text: &str, query: &str, flavor: Flavor) -> String {
    let keyword = query.split_whitespace().find(|s| s.chars().count() >= 3).unwrap_or("");
    if keyword.is_empty() {
        return escape(text, flavor);
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
            out.push_str(&escape(&text[plain..at], flavor));
            out.push_str(&format!("<b>{}</b>", escape(&text[at..end], flavor)));
            (plain, at) = (end, end);
        } else {
            at += ch.len_utf8();
        }
    }
    out.push_str(&escape(&text[plain..], flavor));
    out
}

/// The desktop entry for app `id`: where it was found, and its contents.
pub fn desktop_entry(id: &str) -> Option<(PathBuf, glib::KeyFile)> {
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

pub fn app_keywords(id: &str) -> String {
    desktop_entry(id)
        .and_then(|(_, key)| key.string_list("Desktop Entry", "Keywords").ok())
        .map(|v| v.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

/// What an app card shows beyond the name and description.
pub struct AppFacts {
    pub kind: String,
    pub categories: Vec<&'static str>,
    pub keywords: String,
    pub source: &'static str,
}

pub fn app_facts(id: &str) -> AppFacts {
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

/// The file's type from its name, except that a `.ts` file which isn't an MPEG transport
/// stream is TypeScript, so it neither looks like nor opens as a video.
pub fn content_type(path: &Path) -> String {
    let typescript = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("ts")) && media_kind(path).is_none();
    if typescript {
        return "application/typescript".into();
    }
    gio::content_type_guess(Some(path), None::<&[u8]>).0.to_string()
}

pub fn media_kind(path: &Path) -> Option<bool> {
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
pub fn code_markup(code: &str, path: &Path, flavor: Flavor) -> Option<String> {
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
            let (bold, italic) = (style.font_style.contains(FontStyle::BOLD), style.font_style.contains(FontStyle::ITALIC));
            let text = escape(token, flavor);
            markup.push_str(&match flavor {
                Flavor::Pango => format!(
                    "<span foreground=\"#{:02x}{:02x}{:02x}\"{}{}>{text}</span>",
                    c.r,
                    c.g,
                    c.b,
                    if bold { " weight=\"bold\"" } else { "" },
                    if italic { " style=\"italic\"" } else { "" }
                ),
                Flavor::Html => format!(
                    "<span style=\"color:#{:02x}{:02x}{:02x};{}{}\">{text}</span>",
                    c.r,
                    c.g,
                    c.b,
                    if bold { "font-weight:bold;" } else { "" },
                    if italic { "font-style:italic;" } else { "" }
                ),
            });
        }
    }
    Some(markup)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_ignores_case_and_keeps_text() {
        assert_eq!(highlight("The Report & report", "report", Flavor::Pango), "The <b>Report</b> &amp; <b>report</b>");
        assert_eq!(highlight("Ärger ÄRGER", "ärger", Flavor::Pango), "<b>Ärger</b> <b>ÄRGER</b>");
        assert_eq!(highlight("abc", "zz", Flavor::Pango), "abc");
        assert_eq!(highlight("a <b> report", "report", Flavor::Html), "a &lt;b&gt; <b>report</b>");
    }
}
