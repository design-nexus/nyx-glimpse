//! Explicit diagnostics only: timings contain no queries, paths, result IDs or secrets.
use crate::query::{Matcher, name_rank};
use serde_json::json;
use std::time::{Duration, Instant};
static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
pub fn start() {
    START.get_or_init(Instant::now);
}
pub fn started() -> Instant {
    *START.get_or_init(Instant::now)
}
pub fn enabled() -> bool {
    std::env::var_os("GLIMPSE_TRACE").is_some()
}
pub fn timing(event: &str, elapsed: Duration) {
    if enabled() {
        eprintln!("glimpse-metric {}", json!({"event":event,"elapsed_us":elapsed.as_micros()}));
    }
}
fn rss() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|n| n.parse().ok())
        })
        .unwrap_or(0)
}
pub fn benchmark() -> serde_json::Value {
    let mut measurements = Vec::new();
    for count in [10_000, 100_000] {
        let before = rss();
        let start = Instant::now();
        let names: Vec<_> = (0..count).map(|i| format!("Project report {i} final document.txt").to_lowercase()).collect();
        let setup = start.elapsed();
        let after = rss();
        for query in ["project report", "porject report", "final", "report 2026", "unrelated"] {
            let matcher = Matcher::new(query);
            for baseline in [true, false] {
                let mut samples = Vec::new();
                let mut matches = 0;
                for _ in 0..20 {
                    let start = Instant::now();
                    matches = names
                        .iter()
                        .filter(
                            |name| if baseline { name_rank(name, query).is_some() } else { matcher.score_lower(name).is_some() },
                        )
                        .count();
                    samples.push(start.elapsed().as_micros());
                    std::hint::black_box(matches);
                }
                samples.sort_unstable();
                measurements.push(json!({"entries":count,"query":query,"matcher":if baseline {"previous lexical"}else{"fuzzy"},"samples":samples.len(),"median_us":samples[10],"p95_us":samples[18],"matches":matches,"catalogue_setup_us":setup.as_micros(),"rss_delta_kib":after.saturating_sub(before),"rss_kib":rss()}));
            }
        }
    }
    json!({"kind":"synthetic matcher scan; not end-to-end GUI latency","version":env!("CARGO_PKG_VERSION"),"measurements":measurements,"relevance":relevance()})
}

/// Small reproducible regression corpus, not a broad relevance evaluation.
pub fn relevance() -> serde_json::Value {
    let names = ["Firefox", "Files", "Visual Studio Code", "Report 2.txt", "Report 2026.txt"];
    let cases = [
        ("firefox", "Firefox"),
        ("fireofx", "Firefox"),
        ("frfx", "Firefox"),
        ("vsc", "Visual Studio Code"),
        ("code", "Visual Studio Code"),
        ("reoprt 2", "Report 2.txt"),
        ("2026", "Report 2026.txt"),
        ("unrelated", ""),
    ];
    let mut rows = Vec::new();
    let mut old_correct = 0;
    let mut new_correct = 0;
    for (query, expected) in cases {
        let matcher = Matcher::new(query);
        let old = names.iter().filter_map(|n| name_rank(n, query).map(|q| (q, *n))).min().map(|(_, n)| n).unwrap_or("");
        let new = names.iter().filter_map(|n| matcher.score(n).map(|q| (q, *n))).min().map(|(_, n)| n).unwrap_or("");
        old_correct += usize::from(old == expected);
        new_correct += usize::from(new == expected);
        rows.push(json!({"query":query,"expected":expected,"previous_top":old,"new_top":new}));
    }
    json!({"cases":rows,"previous_correct":old_correct,"new_correct":new_correct,"total":cases.len()})
}
