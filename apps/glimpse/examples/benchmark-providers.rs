//! Synthetic SQLite filename catalogue benchmark; no files/models/services are indexed.
use nyx_glimpse::{
    config::{Config, Paths},
    index,
    providers::{Files, Provider, SearchContext},
};
use serde_json::json;
use std::{fs, time::Instant};
fn rss() -> u64 {
    fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find(|l| l.starts_with("VmRSS:"))
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap()
}
fn main() {
    let mut output = vec![];
    for count in [10_000, 100_000] {
        let temp = tempfile::Builder::new().prefix("glimpse-benchmark-").tempdir().unwrap();
        let p = Paths { config: temp.path().join("config"), data: temp.path().join("data"), state: temp.path().join("state") };
        let root = temp.path().join("documents");
        fs::create_dir(&root).unwrap();
        let c = Config { roots: vec![root.clone()], learning: false, ..Config::default() };
        p.save(&c).unwrap();
        let mut conn = index::open(&p).unwrap();
        let tx = conn.transaction().unwrap();
        {
            let mut stmt = tx.prepare("INSERT INTO files(path,name,root,mtime,size) VALUES(?1,?2,?3,0,0)").unwrap();
            for i in 0..count {
                let name = format!("Project report {i} final document.txt");
                stmt.execute(rusqlite::params![root.join(&name).display().to_string(), name, root.display().to_string()])
                    .unwrap();
            }
        }
        tx.commit().unwrap();
        let before = rss();
        let mut provider = Files::default();
        let start = Instant::now();
        let cold =
            provider.discover(&SearchContext { paths: &p, config: &c, query: "project report", cancelled: &|| false }).unwrap();
        let cold_us = start.elapsed().as_micros();
        let after = rss();
        for query in ["project report", "porject report", "report 2026", "unrelated"] {
            let mut times = vec![];
            let mut results = 0;
            for _ in 0..20 {
                let start = Instant::now();
                let hits = provider.discover(&SearchContext { paths: &p, config: &c, query, cancelled: &|| false }).unwrap();
                times.push(start.elapsed().as_micros());
                results = hits.len();
            }
            times.sort_unstable();
            output.push(json!({"entries":count,"query":query,"samples":times.len(),"median_us":times[10],"p95_us":times[18],"results":results,"cold_load_us":cold_us,"cold_results":cold.len(),"rss_kib":rss(),"catalogue_rss_delta_kib":after.saturating_sub(before)}));
        }
    }
    println!("{}", json!({"kind":"synthetic SQLite filename provider; no real files or inference","measurements":output}));
}
