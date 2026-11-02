//! Optional, bounded local learning. Vault and utility queries never enter this store.
use crate::{
    config::{Config, Paths},
    passwords,
    query::{Matcher, Quality},
    utility,
};
use anyhow::Result;
use rusqlite::{Connection, params};
use std::{
    cmp::Reverse,
    collections::BTreeMap,
    time::{SystemTime, UNIX_EPOCH},
};
const RETENTION: i64 = 90 * 86400;
pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64
}
pub fn eligible_id(id: &str) -> bool {
    id.len() <= 4096 && ["app:", "file:", "system:", "omarchy:", "service:", "plugin:"].iter().any(|p| id.starts_with(p))
}
fn open(p: &Paths) -> Result<Connection> {
    p.ensure()?;
    let file = p.state.join("learning.sqlite");
    if file.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
        anyhow::bail!("Learning store cannot be a symlink");
    }
    // Explicit permissions also protect creation from library callers without the app's umask.
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(&file)?;
    let conn = Connection::open(file)?;
    conn.busy_timeout(std::time::Duration::from_millis(200))?;
    conn.execute_batch("PRAGMA secure_delete=ON; CREATE TABLE IF NOT EXISTS usage(id TEXT PRIMARY KEY, visits REAL NOT NULL, at INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS terms(id TEXT NOT NULL, term TEXT NOT NULL, at INTEGER NOT NULL, PRIMARY KEY(id,term)); CREATE TABLE IF NOT EXISTS history(query TEXT PRIMARY KEY, at INTEGER NOT NULL);")?;
    Ok(conn)
}
#[derive(Default)]
pub struct Profile {
    items: BTreeMap<String, (f64, i64)>,
    terms: BTreeMap<String, Vec<String>>,
}
pub type Rank = (Quality, u8, Reverse<u16>);
impl Profile {
    pub fn load(p: &Paths, c: &Config) -> Result<Self> {
        if !c.learning {
            return Ok(Self::default());
        }
        let conn = open(p)?;
        let cutoff = now() - RETENTION;
        let mut profile = Self::default();
        let mut stmt = conn.prepare("SELECT id,visits,at FROM usage WHERE at>=?1")?;
        for r in stmt.query_map([cutoff], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))? {
            let (id, v) = r?;
            profile.items.insert(id, v);
        }
        let mut stmt = conn.prepare("SELECT id,term FROM terms WHERE at>=?1 ORDER BY at DESC,rowid DESC")?;
        for r in stmt.query_map([cutoff], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (id, t) = r?;
            profile.terms.entry(id).or_default().push(t);
        }
        Ok(profile)
    }
    pub fn rank(&self, c: &Config, matcher: &Matcher, q: &str, id: &str, title: &str, keywords: &str) -> Option<Rank> {
        let normalized = q.trim().to_lowercase();
        let alias = c.aliases.get(id).and_then(|a| {
            let alias = a.to_lowercase();
            if alias == normalized {
                Some(Quality(0, 0))
            } else if !normalized.is_empty() && alias.starts_with(&normalized) {
                Some(Quality(3, 0))
            } else {
                None
            }
        });
        let learned = c.learning && self.terms.get(id).is_some_and(|ts| ts.contains(&normalized));
        let quality = [
            alias,
            matcher.score(title),
            learned.then_some(Quality(2, 0)),
            matcher.score(keywords).map(|r| Quality(r.0 + 8, r.1)),
        ]
        .into_iter()
        .flatten()
        .min()?;
        let boost = if c.learning {
            self.items
                .get(id)
                .map(|(visits, at)| (visits * 2f64.powf(-((now() - at).max(0) as f64) / (14. * 86400.))).ln_1p().min(10.) * 100.)
                .unwrap_or(0.) as u16
        } else {
            0
        };
        Some((quality, u8::from(!c.favorites.iter().any(|f| f == id)), Reverse(boost)))
    }
}
pub fn record(p: &Paths, id: &str, query: &str) -> Result<()> {
    // Read the current preference at execution time, not a stale worker snapshot.
    if !p.load()?.learning
        || !eligible_id(id)
        || passwords::parse(query).is_some()
        || utility::intent(query) != utility::Intent::Search
        || query.trim().is_empty()
        || query.len() > 256
    {
        return Ok(());
    }
    let mut conn = open(p)?;
    let tx = conn.transaction()?;
    let at = now();
    let old: Option<(f64, i64)> =
        tx.query_row("SELECT visits,at FROM usage WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).ok();
    let visits = old.map(|(v, t)| v * 2f64.powf(-((at - t).max(0) as f64) / 1209600.)).unwrap_or(0.) + 1.;
    tx.execute("INSERT OR REPLACE INTO usage VALUES(?1,?2,?3)", params![id, visits.min(1e6), at])?;
    tx.execute("INSERT OR REPLACE INTO terms VALUES(?1,?2,?3)", params![id, query.trim().to_lowercase(), at])?;
    tx.execute("INSERT OR REPLACE INTO history VALUES(?1,?2)", params![query.trim(), at])?;
    for table in ["usage", "terms", "history"] {
        tx.execute(&format!("DELETE FROM {table} WHERE at<?1"), [at - RETENTION])?;
    }
    tx.execute(
        "DELETE FROM terms WHERE id=?1 AND term NOT IN (SELECT term FROM terms WHERE id=?1 ORDER BY at DESC,rowid DESC LIMIT 20)",
        [id],
    )?;
    tx.execute("DELETE FROM history WHERE query NOT IN (SELECT query FROM history ORDER BY at DESC,rowid DESC LIMIT 100)", [])?;
    tx.commit()?;
    Ok(())
}
pub fn history(p: &Paths) -> Result<Vec<String>> {
    if !p.load()?.learning {
        return Ok(vec![]);
    }
    let conn = open(p)?;
    let mut stmt = conn.prepare("SELECT query FROM history WHERE at>=?1 ORDER BY at DESC,rowid DESC LIMIT 100")?;
    Ok(stmt.query_map([now() - RETENTION], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
}
pub fn clear(p: &Paths) -> Result<()> {
    let conn = open(p)?;
    conn.execute_batch("DELETE FROM usage; DELETE FROM terms; DELETE FROM history; VACUUM;")?;
    Ok(())
}
pub fn favorite(p: &Paths, id: &str) -> Result<()> {
    let _lock = crate::ai_setup::config_lock(p)?;
    let mut c = p.load()?;
    if c.favorites.iter().any(|f| f == id) {
        c.favorites.retain(|f| f != id);
    } else {
        c.favorites.push(id.into());
    }
    p.save(&c)
}
pub fn alias(p: &Paths, id: &str, alias: &str) -> Result<()> {
    alias_for_query(p, id, alias, None)
}
pub fn alias_for_query(p: &Paths, id: &str, alias: &str, query: Option<&str>) -> Result<()> {
    let _lock = crate::ai_setup::config_lock(p)?;
    let mut c = p.load()?;
    if alias.trim().is_empty() {
        c.aliases.remove(id);
        c.alias_queries.remove(id);
    } else {
        c.aliases.insert(id.into(), alias.trim().into());
        if let Some(query) = query.filter(|q| !q.trim().is_empty()) {
            c.alias_queries.insert(id.into(), query.into());
        }
    }
    p.save(&c)
}
