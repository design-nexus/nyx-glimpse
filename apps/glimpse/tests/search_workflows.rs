use nyx_glimpse::{
    config::{Config, Paths},
    learning, omarchy, plugins,
    providers::{Applications, Files, Provider, Registry, SearchContext},
    query::{Matcher, Quality},
};
use std::{fs, os::unix::fs::PermissionsExt};
fn fixture() -> (tempfile::TempDir, Paths, Config) {
    let temp = tempfile::Builder::new().prefix("glimpse-search-test-").tempdir().unwrap();
    let paths = Paths { config: temp.path().join("config"), data: temp.path().join("data"), state: temp.path().join("state") };
    paths.ensure().unwrap();
    let root = temp.path().join("documents");
    fs::create_dir(&root).unwrap();
    let config = Config { roots: vec![root], ..Config::default() };
    paths.save(&config).unwrap();
    (temp, paths, config)
}
#[test]
fn fuzzy_matching_preserves_numeric_phrases_and_all_terms() {
    for q in ["firefox", "fire", "frfx", "firefx", "fireofx"] {
        assert!(Matcher::new(q).score("Firefox").is_some(), "{q}");
    }
    assert!(Matcher::new("vsc").score("Visual Studio Code").is_some());
    assert!(Matcher::new("2").score("Report 2026").is_none());
    assert!(Matcher::new("report final").score("Draft final").is_none());
    assert!(Matcher::new("\"report final\"").score("report draft final").is_none());
    assert!(Matcher::new("fire zzz").score("Firefox").is_none());
    assert!(Matcher::new("файл").score("Файл").is_some());
    assert!(Matcher::new("reoprt 2").score("Report 2.txt").is_some());
    assert!(Matcher::new("reoprt 2").score("Report 2026.txt").is_none());
    assert_eq!(Matcher::new("Firefox").score("Firefox"), Some(Quality(1, 0)));
}
#[test]
fn learned_aliases_and_usage_never_override_exact_titles_or_admit_unrelated_results() {
    let (_t, p, mut c) = fixture();
    for _ in 0..12 {
        learning::record(&p, "app:other.desktop", "fire").unwrap();
    }
    let profile = learning::Profile::load(&p, &c).unwrap();
    let matcher = Matcher::new("fire");
    let exact = profile.rank(&c, &matcher, "fire", "app:exact.desktop", "Fire", "").unwrap();
    let learned = profile.rank(&c, &matcher, "fire", "app:other.desktop", "Other", "").unwrap();
    assert!(exact < learned);
    assert!(profile.rank(&c, &Matcher::new("unrelated"), "unrelated", "app:other.desktop", "Other", "").is_none());
    c.favorites.push("app:other.desktop".into());
    assert!(profile.rank(&c, &Matcher::new("unrelated"), "unrelated", "app:other.desktop", "Other", "").is_none());
    learning::alias(&p, "app:other.desktop", "ff").unwrap();
    c = p.load().unwrap();
    assert_eq!(profile.rank(&c, &Matcher::new("ff"), "ff", "app:other.desktop", "Other", "").unwrap().0, Quality(0, 0));
    assert!(learning::alias(&p, "app:other.desktop", "pw github").is_err());
}
#[test]
fn privacy_disable_clear_retention_and_sensitive_queries() {
    let (_t, p, mut c) = fixture();
    learning::record(&p, "app:firefox.desktop", "firefox").unwrap();
    learning::record(&p, "app:firefox.desktop", "pw github").unwrap();
    learning::record(&p, "password:entry", "github").unwrap();
    learning::record(&p, "app:firefox.desktop", "2+2").unwrap();
    assert_eq!(learning::history(&p).unwrap(), vec!["firefox"]);
    assert_eq!(fs::metadata(p.state.join("learning.sqlite")).unwrap().permissions().mode() & 0o777, 0o600);
    c.learning = false;
    p.save(&c).unwrap();
    learning::record(&p, "app:firefox.desktop", "ignored").unwrap();
    assert!(learning::history(&p).unwrap().is_empty());
    let profile = learning::Profile::load(&p, &c).unwrap();
    assert!(profile.rank(&c, &Matcher::new("ff"), "ff", "app:firefox.desktop", "Other", "").is_none());
    c.learning = true;
    p.save(&c).unwrap();
    assert_eq!(learning::history(&p).unwrap(), vec!["firefox"]);
    learning::favorite(&p, "app:firefox.desktop").unwrap();
    learning::alias(&p, "app:firefox.desktop", "browser").unwrap();
    learning::clear(&p).unwrap();
    assert!(learning::history(&p).unwrap().is_empty());
    assert_eq!(p.load().unwrap().aliases["app:firefox.desktop"], "browser");
    assert_eq!(p.load().unwrap().favorites.len(), 1);
    let conn = rusqlite::Connection::open(p.state.join("learning.sqlite")).unwrap();
    conn.execute("INSERT INTO history VALUES('expired',0)", []).unwrap();
    assert!(learning::history(&p).unwrap().is_empty());
}
#[test]
fn providers_reject_duplicates_and_files_refresh_after_edits_and_revocation() {
    let (_t, p, mut c) = fixture();
    let mut registry = Registry::default();
    registry.register(Applications(vec![])).unwrap();
    assert!(registry.register(Applications(vec![])).is_err());
    fs::write(c.roots[0].join("Report 2.txt"), "x").unwrap();
    nyx_glimpse::index::scan(&p, &c).unwrap();
    let mut files = Files::default();
    let cancelled = || false;
    assert_eq!(
        files.discover(&SearchContext { paths: &p, config: &c, query: "reoprt 2", cancelled: &cancelled }).unwrap().len(),
        1
    );
    fs::rename(c.roots[0].join("Report 2.txt"), c.roots[0].join("Notes 2.txt")).unwrap();
    nyx_glimpse::index::scan(&p, &c).unwrap();
    assert!(
        files.discover(&SearchContext { paths: &p, config: &c, query: "reoprt 2", cancelled: &cancelled }).unwrap().is_empty()
    );
    assert_eq!(
        files.discover(&SearchContext { paths: &p, config: &c, query: "notes 2", cancelled: &cancelled }).unwrap().len(),
        1
    );
    c.excluded = c.roots.clone();
    assert!(
        files.discover(&SearchContext { paths: &p, config: &c, query: "notes 2", cancelled: &cancelled }).unwrap().is_empty()
    );
}
#[test]
fn shortcuts_and_jsonc_reject_injection_without_breaking_existing_configuration() {
    assert!(nyx_glimpse::config::validate(&Config::default()).is_ok());
    let mut c = Config { action_key: "<Control>p".into(), ..Config::default() };
    assert!(nyx_glimpse::config::validate(&c).is_err());
    c.action_key = "<Alt>k".into();
    c.favorite_key = "<Alt>k".into();
    assert!(nyx_glimpse::config::validate(&c).is_err());
    let parsed = omarchy::parse_jsonc("{\n// Comment\n\"url\":\"https://example.org/a,}\",/*x*/\"items\":[1,],}").unwrap();
    assert_eq!(parsed["url"], "https://example.org/a,}");
    assert_eq!(parsed["items"][0], 1);
    assert!(omarchy::parse_jsonc("{ /* unclosed").is_err());
    assert!(omarchy::argv("omarchy toggle nightlight; echo unsafe").is_none());
    assert!(omarchy::argv("omarchy launch $(unsafe)").is_none());
    assert_eq!(omarchy::argv("omarchy font set 'Inter Variable'").unwrap(), vec!["omarchy", "font", "set", "Inter Variable"]);
}
#[test]
fn plugin_contextual_capability_requires_renewed_grant_and_legacy_hits_still_parse() {
    let (temp, p, _) = fixture();
    let source = temp.path().join("plugin");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("plugin.toml"),"id='test-context'\nname='Context'\nversion='1'\napi_version=1\nexec=['python3','main.py']\ncapabilities=['search','contextual_actions']\n").unwrap();
    fs::write(source.join("main.py"), "raise RuntimeError('must not execute before grant')").unwrap();
    plugins::install_local(&p, &source, "local", "", false).unwrap();
    assert!(plugins::contextual_actions(&p, "test-context", "item").is_err());
    let legacy: glimpse_plugin_sdk::Hit =
        serde_json::from_str(r#"{"id":"a","title":"Hello","action":{"kind":"copy","text":"hello"}}"#).unwrap();
    assert_eq!(legacy.id, "a");
    let mut grant: serde_json::Value = serde_json::from_slice(&fs::read(p.config.join("plugins.json")).unwrap()).unwrap();
    grant["test-context"]["enabled"] = true.into();
    grant["test-context"]["capabilities"] = serde_json::json!([]);
    fs::write(p.config.join("plugins.json"), serde_json::to_vec(&grant).unwrap()).unwrap();
    let e = plugins::contextual_actions(&p, "test-context", "item").unwrap_err();
    assert!(e.to_string().contains("renewed"));
}
#[test]
fn action_dispatch_rechecks_file_scope_and_rejects_unknown_verbs() {
    use nyx_glimpse::providers::{Effect, Target, resolve};
    let (_t, p, mut c) = fixture();
    let path = c.roots[0].join("file.txt");
    fs::write(&path, "hello").unwrap();
    let target = Target::File(path.clone());
    assert!(matches!(resolve(&p, &target, "copy-path").unwrap(), Effect::Copy(_)));
    assert!(matches!(resolve(&p, &target, "reveal").unwrap(), Effect::OpenFile(_, true)));
    assert!(resolve(&p, &target, "execute").is_err());
    c.excluded = c.roots.clone();
    p.save(&c).unwrap();
    assert!(resolve(&p, &target, "open").is_err());
}
#[test]
fn contextual_plugin_actions_run_only_inside_current_grants() {
    let (temp, p, _) = fixture();
    let source = temp.path().join("context-plugin");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("plugin.toml"),"id='test-dispatch'\nname='Dispatch'\nversion='1'\napi_version=1\nexec=['python3','main.py']\ncapabilities=['search','contextual_actions']\n").unwrap();
    fs::write(
        source.join("main.py"),
        r#"import json,sys
for line in sys.stdin:
 r=json.loads(line); op=r['op']; answer={'id':r['id']}
 if op=='hello': answer.update(api_version=1,capabilities=['search','contextual_actions'])
 elif op=='actions': answer['result']=[{'id':'copy','title':'Copy greeting','action':{'kind':'activate','token':'greeting'}}]
 elif op=='activate': answer['result']={'kind':'copy','text':'hello'}
 elif op=='shutdown': break
 print(json.dumps(answer),flush=True)
"#,
    )
    .unwrap();
    plugins::install_local(&p, &source, "local", "", false).unwrap();
    plugins::enable(&p, "test-dispatch", true).unwrap();
    assert_eq!(plugins::contextual_actions(&p, "test-dispatch", "row").unwrap().len(), 1);
    assert!(
        matches!(plugins::contextual_activate(&p,"test-dispatch","row","copy").unwrap(),glimpse_plugin_sdk::Action::Copy{text} if text=="hello")
    );
    assert!(plugins::contextual_activate(&p, "test-dispatch", "row", "unknown").is_err());
    plugins::disable(&p, "test-dispatch").unwrap();
    assert!(plugins::contextual_activate(&p, "test-dispatch", "row", "copy").is_err());
}
#[test]
fn relevance_corpus_covers_typos_abbreviations_and_no_match() {
    let corpus = nyx_glimpse::metrics::relevance();
    assert_eq!(corpus["new_correct"], corpus["total"]);
}
