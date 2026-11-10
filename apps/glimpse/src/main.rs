mod cb;
mod player;
mod pw;
mod ui;
use gtk::{gio, glib, prelude::*};
fn main() -> glib::ExitCode {
    nyx_glimpse::metrics::start();
    unsafe {
        libc::umask(0o077);
    }
    nyx_core::app::init(nyx_core::app::App { slug: "nyx-glimpse", name: "Glimpse" });
    if let Err(e) = nyx_glimpse::plugins::migrate_nixe(&nyx_glimpse::config::Paths::discover()) {
        eprintln!("nyx-glimpse: moving plugins from their Nixe names: {e:#}");
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    match nyx_glimpse::cli::dispatch(&args) {
        Ok(true) => return glib::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e:#}");
            return glib::ExitCode::FAILURE;
        }
        Ok(false) => {}
    }
    // Where the Quickshell interface runs (nyx-shell-qs), it is Glimpse's panel; the GTK
    // one answers when it doesn't.
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let query = args.iter().position(|a| a == "--query").and_then(|i| args.get(i + 1));
    let asked = if has("--hide") {
        nyx_glimpse::backend::ask_interface("Hide", serde_json::json!({}))
    } else if has("--add-folder") {
        nyx_glimpse::backend::ask_interface("AddFolder", serde_json::json!({}))
    } else if has("--plugins") {
        nyx_glimpse::backend::ask_interface("OpenPlugins", serde_json::json!({}))
    } else if has("--add-plugin") {
        nyx_glimpse::backend::ask_interface("PluginOp", serde_json::json!({"op": "add_local"}))
    } else if let Some(query) = query {
        nyx_glimpse::backend::ask_interface("Show", serde_json::json!({"query": query}))
    } else if has("--toggle") {
        nyx_glimpse::backend::ask_interface("Toggle", serde_json::json!({}))
    } else {
        nyx_glimpse::backend::ask_interface("Show", serde_json::json!({}))
    };
    if asked.is_ok() {
        return glib::ExitCode::SUCCESS;
    }
    if std::env::var_os("GSK_RENDERER").is_none() {
        unsafe {
            std::env::set_var("GSK_RENDERER", "ngl");
        }
    }
    let id = std::env::var("GLIMPSE_APP_ID").unwrap_or_else(|_| "io.github.design_nexus.nyx.Glimpse".into());
    let app = gtk::Application::builder().application_id(id).flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE).build();
    app.connect_startup(|_| ui::theme());
    app.connect_command_line(|app, cmd| {
        let args = cmd.arguments();
        let has = |s: &str| args.iter().any(|v| v == s);
        if has("--hide") {
            ui::dismiss();
        } else if has("--add-folder") {
            ui::add_folder(app);
        } else if has("--plugins") {
            ui::plugin_manager(app);
        } else if has("--add-plugin") {
            ui::add_plugin(app);
        } else {
            let query =
                args.iter().position(|s| s == "--query").and_then(|i| args.get(i + 1)).map(|s| s.to_string_lossy().into_owned());
            ui::present(app, has("--toggle"), query.as_deref());
        }
        glib::ExitCode::SUCCESS
    });
    app.run()
}
