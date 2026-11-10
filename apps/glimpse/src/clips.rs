//! The `cb` provider (Nyx Clipboard), for both panels: rows from the Nyx Clipboard service (`Clipboard1`) and what Enter
//! does to them. Pasting and copying happen in the service, which owns the clipboard and
//! sends the paste once Glimpse has closed. Verification codes never come through here.

use gtk::{gio, glib, prelude::*};

const BUS: &str = "io.github.design_nexus.nyx.Clipboard1";
const PATH: &str = "/io/github/design_nexus/nyx/Clipboard1";
const IFACE: &str = "io.github.design_nexus.nyx.Clipboard1";

#[derive(Clone, Debug, PartialEq)]
pub enum Cb {
    Entry { id: i64, kind: String, pinned: bool, size: u64, app: String },
    /// Clipboard isn't installed or its service isn't answering.
    Unavailable,
    /// Guidance in place of results; Enter does nothing.
    Help,
}

/// What a row is asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    Paste,
    PastePlain,
    Copy,
    Pin,
    Delete,
    /// Open the Clipboard popup.
    Open,
}

impl Cb {
    pub fn key(&self) -> String {
        match self {
            Cb::Entry { id, .. } => format!("cb:{id}"),
            other => format!("cb:{other:?}"),
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Cb::Entry { kind, .. } => match kind.as_str() {
                "image" => "image-x-generic-symbolic",
                "url" => "web-browser-symbolic",
                "files" => "folder-documents-symbolic",
                "code" => "applications-engineering-symbolic",
                "rich" | "html" => "format-text-rich-symbolic",
                _ => "edit-paste-symbolic",
            },
            Cb::Unavailable => "dialog-warning-symbolic",
            Cb::Help => "system-search-symbolic",
        }
    }

    pub fn hints(&self) -> Vec<(&'static [&'static str], &'static str)> {
        match self {
            Cb::Entry { kind, .. } => {
                let mut v = vec![(&["Enter"][..], "Paste")];
                if kind != "image" {
                    v.push((&["Ctrl", "Enter"][..], "Plain text"));
                }
                v.push((&["Alt", "Enter"][..], "Copy"));
                v
            }
            Cb::Unavailable | Cb::Help => vec![],
        }
    }

    pub fn details(&self) -> Vec<(String, String)> {
        match self {
            Cb::Entry { kind, pinned, size, app, .. } => {
                let mut d = vec![("Kind".to_string(), kind_name(kind).to_string())];
                if !app.is_empty() {
                    d.push(("Copied in".into(), app.clone()));
                }
                d.push(("Size".into(), human(*size)));
                if *pinned {
                    d.push(("Pinned".into(), "Yes".into()));
                }
                d
            }
            _ => vec![],
        }
    }
}

fn kind_name(kind: &str) -> &'static str {
    match kind {
        "url" => "Link",
        "image" => "Image",
        "files" => "Files",
        "code" => "Code",
        "markdown" => "Markdown",
        "html" => "HTML",
        "rich" => "Formatted text",
        _ => "Text",
    }
}

fn human(b: u64) -> String {
    match b {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{:.1} KB", b as f64 / 1024.0),
        b => format!("{:.1} MB", b as f64 / 1_048_576.0),
    }
}

async fn call(method: &str, args: Option<glib::Variant>) -> Result<glib::Variant, String> {
    let bus = gio::bus_get_future(gio::BusType::Session).await.map_err(|e| e.to_string())?;
    bus.call_future(Some(BUS), PATH, IFACE, method, args.as_ref(), None, gio::DBusCallFlags::NONE, 10_000).await.map_err(|mut e| {
        gio::DBusError::strip_remote_error(&mut e);
        e.message().to_string()
    })
}

/// One row: title, second line, what it is.
pub type Row = (String, String, Cb);

fn ago(ts: f64) -> String {
    let now = glib::real_time() as f64 / 1e6;
    let s = (now - ts).max(0.0);
    match s {
        s if s < 60.0 => "now".into(),
        s if s < 3600.0 => format!("{} min ago", (s / 60.0) as i64),
        s if s < 86_400.0 => format!("{} h ago", (s / 3600.0) as i64),
        s => format!("{} days ago", (s / 86_400.0) as i64),
    }
}

/// The rows for a `cb` query.
pub async fn lookup(text: &str) -> (Vec<Row>, String) {
    let query = serde_json::json!({ "text": text, "limit": 20 }).to_string();
    let reply = match call("Search", Some((query,).to_variant())).await {
        Ok(v) => v,
        Err(e) => {
            let row = ("Clipboard isn't answering".to_string(), e, Cb::Unavailable);
            return (vec![row], String::new());
        }
    };
    let json = reply.child_value(0).str().unwrap_or("[]").to_string();
    let entries: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap_or_default();
    let mut rows: Vec<Row> = entries
        .iter()
        .filter_map(|e| {
            let id = e["id"].as_i64()?;
            let kind = e["kind"].as_str().unwrap_or("text").to_string();
            let app = e["app"].as_str().unwrap_or("").to_string();
            let short_app = app.rsplit('.').next().unwrap_or(&app).to_string();
            let mut second = vec![kind_name(&kind).to_string(), ago(e["last_used"].as_f64().unwrap_or(0.0))];
            if !short_app.is_empty() {
                second.insert(1, short_app);
            }
            Some((
                e["preview"].as_str().unwrap_or("").to_string(),
                second.join(" · "),
                Cb::Entry { id, kind, pinned: e["pinned"] == true, size: e["size"].as_u64().unwrap_or(0), app },
            ))
        })
        .collect();
    if rows.is_empty() {
        let help = if text.is_empty() {
            ("Nothing copied yet".to_string(), "cb finds what you copied · cb img: · cb url: · Super + Ctrl + V opens Clipboard".to_string())
        } else {
            (format!("Nothing copied matches “{text}”"), "Try fewer or other words, or a kind: cb img: · cb url: · cb code:".to_string())
        };
        rows.push((help.0, help.1, Cb::Help));
    }
    (rows, String::new())
}

/// Whether `how` hides the panel before it runs, so a paste reaches the window under it.
pub fn hides_first(how: How) -> bool {
    matches!(how, How::Paste | How::PastePlain | How::Copy | How::Open)
}

/// Acts on an entry in the service; what to say about it when the panel stays (Pin, Delete).
pub async fn perform(id: i64, pinned: bool, how: How) -> Result<&'static str, String> {
    match how {
        How::Paste | How::PastePlain => call("Paste", Some((vec![id], how == How::PastePlain).to_variant())).await.map(|_| "Pasted"),
        How::Copy => call("Copy", Some((id, false).to_variant())).await.map(|_| "Copied"),
        How::Pin => call("SetPinned", Some((id, !pinned).to_variant())).await.map(|_| if pinned { "Unpinned" } else { "Pinned" }),
        How::Delete => call("Delete", Some((vec![id],).to_variant())).await.map(|_| "Deleted"),
        How::Open => std::process::Command::new("nyx-clipboard")
            .arg("--show")
            .spawn()
            .map(|_| "Opened")
            .map_err(|_| "Couldn't open Clipboard".to_string()),
    }
}
