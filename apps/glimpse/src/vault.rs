//! The `pw` provider (Nyx Passwords), for both panels: rows from the Nyx Passwords daemon and what Enter does to them.
//! Rows carry an item's id and metadata, never a secret; copies happen in the daemon,
//! which owns the clipboard (and clears it), so Glimpse can close at once.

use gtk::{gio, glib::prelude::*};
use crate::passwords::{Query, Special, Verb};
use nyx_passwords_proto::{Error as Code, client, state};

#[derive(Clone, Debug, PartialEq)]
pub enum Pw {
    Item {
        id: String,
        kind: String,
        username: String,
        host: String,
        has_totp: bool,
        verb: Verb,
    },
    Generate,
    Lock,
    /// Enter opens Passwords' Email aliases page.
    Aliases,
    /// The vault is locked: Enter opens Passwords on its unlock page.
    Unlock,
    /// The vault is locked and Nyx Biometrics can open it: Enter asks for the face.
    FaceUnlock,
    /// No account yet: Enter opens Passwords to sign in.
    SignIn,
    /// Passwords isn't installed or its service won't start.
    Unavailable,
    /// Guidance in place of results (nothing typed yet, nothing matched); Enter does nothing.
    Help,
}

impl Pw {
    pub fn key(&self) -> String {
        match self {
            Pw::Item { id, .. } => format!("pw:{id}"),
            other => format!("pw:{other:?}"),
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Pw::Item { kind, .. } => match kind.as_str() {
                "note" => "text-x-generic-symbolic",
                "identity" => "avatar-default-symbolic",
                _ => "dialog-password-symbolic",
            },
            Pw::Generate => "media-playlist-shuffle-symbolic",
            Pw::Aliases => "mail-send-symbolic",
            Pw::Lock | Pw::Unlock | Pw::FaceUnlock => "system-lock-screen-symbolic",
            Pw::SignIn => "dialog-password-symbolic",
            Pw::Unavailable => "dialog-warning-symbolic",
            Pw::Help => "system-search-symbolic",
        }
    }

    /// The status bar's keys for this row.
    pub fn hints(&self) -> Vec<(&'static [&'static str], &'static str)> {
        match self {
            Pw::Item { verb, has_totp, kind, .. } => {
                let mut list = vec![(&["Enter"][..], verb.label())];
                if kind == "login" {
                    if *verb != Verb::Password {
                        list.push((&["Ctrl", "Enter"][..], "Password"));
                    }
                    if *has_totp && *verb != Verb::Totp {
                        list.push((&["Alt", "Enter"][..], "Code"));
                    }
                }
                list
            }
            Pw::Generate => vec![(&["Enter"][..], "Generate and copy")],
            Pw::Lock => vec![(&["Enter"][..], "Lock")],
            Pw::Aliases => vec![(&["Enter"][..], "Open in Passwords")],
            Pw::Unlock => vec![(&["Enter"][..], "Unlock in Passwords")],
            Pw::FaceUnlock => vec![(&["Enter"][..], "Unlock with your face")],
            Pw::SignIn => vec![(&["Enter"][..], "Open Passwords")],
            Pw::Unavailable | Pw::Help => vec![],
        }
    }

    /// The preview pane's rows.
    pub fn details(&self) -> Vec<(String, String)> {
        match self {
            Pw::Item { kind, username, host, has_totp, .. } => {
                let mut d = vec![("Kind".to_string(), kind_name(kind).to_string())];
                if !username.is_empty() {
                    d.push(("Username".into(), username.clone()));
                }
                if !host.is_empty() {
                    d.push(("Website".into(), host.clone()));
                }
                if kind == "login" {
                    d.push(("One-time code".into(), if *has_totp { "Yes" } else { "No" }.into()));
                    if !host.is_empty() {
                        // The status bar has no room for this one.
                        d.push(("Open the site".into(), "Ctrl+O".into()));
                    }
                }
                d
            }
            _ => vec![],
        }
    }
}

fn kind_name(kind: &str) -> &'static str {
    match kind {
        "login" => "Login",
        "note" => "Secure note",
        "card" => "Card",
        "identity" => "Identity",
        "ssh" => "SSH key",
        _ => "Item",
    }
}

/// One row: title, second line, what it is.
pub type Row = (String, String, Pw);

/// "Vault locked", opened with the face where Passwords has that set up.
async fn locked_row() -> Row {
    let face = client::call("FaceUnlock", None).await.ok().and_then(|v| v.get::<(bool,)>()).is_some_and(|(on,)| on);
    if face {
        ("Vault locked".into(), "Enter to unlock with your face".into(), Pw::FaceUnlock)
    } else {
        ("Vault locked".into(), "Enter to unlock it in Passwords".into(), Pw::Unlock)
    }
}

/// The rows for a `pw` query, and a status message when there's something to say.
pub async fn lookup(q: &Query) -> (Vec<Row>, String) {
    let st = match client::status().await {
        Ok((st, _, _)) => st,
        Err(e) => {
            let title = if client::installed() { "Passwords isn't answering" } else { "Passwords isn't installed" };
            return (vec![(title.into(), e.to_string(), Pw::Unavailable)], String::new());
        }
    };
    if st == state::LOGGED_OUT {
        return (vec![("Sign in to Vaultwarden".into(), "Opens Passwords".into(), Pw::SignIn)], String::new());
    }
    if st == state::LOCKED {
        let rows = match q.special {
            // Already locked: nothing to do, but say so rather than offering to unlock.
            Some(Special::Lock) => return (vec![], "The vault is already locked".into()),
            _ => vec![locked_row().await],
        };
        return (rows, String::new());
    }
    let mut rows = Vec::new();
    match q.special {
        Some(Special::Generate) => rows.push((
            "Generate a password".to_string(),
            "Copies a new one, made with Passwords' generator settings".to_string(),
            Pw::Generate,
        )),
        Some(Special::Lock) => rows.push(("Lock the vault".to_string(), String::new(), Pw::Lock)),
        Some(Special::Aliases) => rows.push((
            "Email aliases".to_string(),
            "See your aliases and the logins that use them, in Passwords".to_string(),
            Pw::Aliases,
        )),
        None => {}
    }
    match client::search(&q.terms, 10).await {
        Ok((_origin, items)) => {
            for (id, name, username, host, has_totp, kind) in items {
                let second =
                    [username.as_str(), host.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
                rows.push((name, second, Pw::Item { id, kind, username, host, has_totp, verb: q.verb }));
            }
        }
        Err(e) if e.is(Code::Denied) => return (rows, "Glimpse is turned off in Passwords' settings".into()),
        Err(e) if e.is(Code::Locked) => {
            return (vec![locked_row().await], String::new());
        }
        Err(e) => return (rows, e.to_string()),
    }
    if rows.is_empty() {
        let help = if q.terms.is_empty() {
            ("Type a name after pw".to_string(), "pw github · pw github pass · pw generate · pw aliases · pw lock".to_string())
        } else {
            (format!("Nothing in the vault matches “{}”", q.terms), "Try fewer or other words".to_string())
        };
        rows.push((help.0, help.1, Pw::Help));
    }
    (rows, String::new())
}

/// What acting on a row came to.
pub enum Ran {
    /// Done: the panel closes.
    Done,
    /// The vault opened with the face: the panel comes back with the same search.
    Unlocked,
    /// Nothing to do (a help row).
    Nothing,
}

/// Acts on a row: `verb` overrides the row's own (Ctrl+Enter, Alt+Enter, Ctrl+O, Ctrl+C).
/// Copies happen in the daemon. A protected item can wait on the system's sign-in dialog,
/// so a panel should step aside if this takes long.
pub async fn perform(pw: &Pw, verb: Option<Verb>) -> Result<Ran, String> {
    match pw {
        Pw::Item { id, verb: own, kind, has_totp, .. } => {
            let verb = verb.unwrap_or(*own);
            if verb == Verb::Totp && !has_totp {
                Err("This item has no one-time code".to_string())
            } else if kind != "login" && verb != Verb::Username {
                Err(format!("This is a {}: open it in Passwords", kind_name(kind).to_lowercase()))
            } else {
                match verb.field() {
                    Some(field) => client::copy(id, field).await.map(|_| Ran::Done).map_err(message),
                    None => match client::text("OpenUrl", Some((id.as_str(),).to_variant())).await {
                        Ok(url) => gio::AppInfo::launch_default_for_uri(&url, None::<&gio::AppLaunchContext>)
                            .map(|_| Ran::Done)
                            .map_err(|e| e.to_string()),
                        Err(e) => Err(message(e)),
                    },
                }
            }
        }
        Pw::Generate => client::call("Generate", None).await.map(|_| Ran::Done).map_err(message),
        Pw::Lock => client::call("Lock", None).await.map(|_| Ran::Done).map_err(message),
        Pw::Aliases => open_passwords(&["--section", "aliases"]),
        // Anything but a cancel goes on in Passwords' own unlock page.
        Pw::FaceUnlock => match client::call("UnlockWithFace", None).await {
            Ok(_) => Ok(Ran::Unlocked),
            Err(e) if e.to_string().contains("not confirmed") => Err("Not confirmed. Enter to try again, or unlock in Passwords".to_string()),
            Err(_) => open_passwords(&["--unlock"]),
        },
        Pw::Unlock | Pw::SignIn => open_passwords(&["--unlock"]),
        Pw::Unavailable => Err("Install Passwords with: nyx install passwords".to_string()),
        Pw::Help => Ok(Ran::Nothing),
    }
}

fn open_passwords(args: &[&str]) -> Result<Ran, String> {
    std::process::Command::new("nyx-passwords").args(args).spawn().map(|_| Ran::Done).map_err(|_| "Couldn't open Passwords".to_string())
}

fn message(e: client::Fail) -> String {
    match e.code {
        Some(Code::Reprompt) => {
            "This item asks for the master password: open it in Passwords, or turn on “Protected items in Glimpse” there".into()
        }
        Some(Code::NoField) => "This item doesn't have that".into(),
        Some(Code::NoUrl) => "This item has no web address".into(),
        Some(Code::RateLimited) => "Too many copies in a row; wait a minute".into(),
        Some(Code::Clipboard) => "Couldn't reach the clipboard".into(),
        _ => e.to_string(),
    }
}
