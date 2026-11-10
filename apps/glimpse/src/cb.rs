//! The GTK panel's side of the `cb` provider (`nyx_glimpse::clips`).

use gtk::{glib, prelude::*};
pub use nyx_glimpse::clips::*;

/// Acts on a row. Pasting hides Glimpse first so the paste reaches the window under it.
pub fn activate(cb: &Cb, how: How, window: &gtk::ApplicationWindow, status: &gtk::Label) {
    let Cb::Entry { id, pinned, .. } = cb.clone() else {
        if *cb == Cb::Unavailable {
            status.set_text("Install Clipboard with: nyx install clipboard");
        }
        return;
    };
    if hides_first(how) {
        window.set_visible(false);
    }
    let (window, status) = (window.clone(), status.clone());
    glib::spawn_future_local(async move {
        match perform(id, pinned, how).await {
            Ok(m) => {
                if matches!(how, How::Pin | How::Delete) {
                    status.set_text(m);
                    crate::ui::rerun_search();
                }
            }
            Err(e) => {
                status.set_text(&e);
                window.present();
            }
        }
    });
}
