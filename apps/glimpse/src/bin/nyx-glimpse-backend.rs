//! Glimpse's search for its Quickshell interface (see `nyx_glimpse::backend`).
fn main() {
    unsafe {
        libc::umask(0o077);
    }
    nyx_core::app::init(nyx_core::app::App { slug: "nyx-glimpse", name: "Glimpse" });
    if let Err(e) = nyx_glimpse::backend::serve(nyx_glimpse::config::Paths::discover()) {
        eprintln!("nyx-glimpse-backend: {e:#}");
        std::process::exit(1);
    }
}
