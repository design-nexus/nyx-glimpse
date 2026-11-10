//! The GTK panel's side of the `pw` provider (`nyx_glimpse::vault`): the window steps aside
//! while the daemon works and comes back with an error.

use gtk::{glib, prelude::*};
pub use nyx_glimpse::vault::*;
use nyx_glimpse::passwords::Verb;

/// Acts on a row: `verb` overrides the row's own (Ctrl+Enter, Alt+Enter, Ctrl+O, Ctrl+C).
/// The window hides once the daemon has done it; errors stay in the status bar. Something
/// slower (a protected item waiting for the system's sign-in dialog) hides it at once, so
/// the dialog gets the keyboard, and an error brings it back.
pub fn activate(pw: &Pw, verb: Option<Verb>, window: &gtk::ApplicationWindow, status: &gtk::Label) {
    let (pw, window, status) = (pw.clone(), window.clone(), status.clone());
    let pending = std::rc::Rc::new(std::cell::Cell::new(true));
    {
        let (pending, window) = (pending.clone(), window.clone());
        glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
            if pending.get() {
                window.set_visible(false);
            }
        });
    }
    glib::spawn_future_local(async move {
        let done = perform(&pw, verb).await;
        pending.set(false);
        match done {
            Ok(Ran::Done) => window.set_visible(false),
            // Back with the same search, now that the vault is open.
            Ok(Ran::Unlocked) => {
                window.present();
                crate::ui::rerun_search();
            }
            Ok(Ran::Nothing) => {}
            Err(e) => {
                status.set_text(&e);
                window.present();
            }
        }
    });
}
