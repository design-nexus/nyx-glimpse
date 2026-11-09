use crate::{
    aicore::{self, Core, Input},
    config::{self, Paths},
    process,
};
use anyhow::{Context, Result, bail};
use nyx_ai_client::proto::Priority;
use serde_json::{Value, json};
use std::{fs, io::Read, path::PathBuf, process::Command, time::Duration};

/// For Settings and the command line: whether Nyx AI Core answers, the model
/// Glimpse embeds with, and whether content search is on.
pub fn status(p: &Paths) -> Value {
    let mut s = aicore::status(p);
    s["enabled"] = p.load().is_ok_and(|c| c.semantic).into();
    s["rebuilding"] = next_index(p).exists().into();
    s
}

/// The index searches use.
fn live_index(p: &Paths) -> PathBuf {
    p.data.join("semantic-index/glimpse.sqlite")
}
/// The index built beside the live one after the embedding model changed; it
/// replaces the live one once every file is in it.
fn next_index(p: &Paths) -> PathBuf {
    p.data.join("semantic-index/glimpse.next.sqlite")
}
/// A search saw AI Core's model differ from the index's: the identity it saw, for
/// the indexer to rebuild for even when no file has changed.
fn model_changed(p: &Paths) -> PathBuf {
    p.state.join("semantic-model-changed")
}

/// A query's vector, and the generic probes' vectors it's measured against.
type QueryVectors = (Vec<f32>, Vec<Vec<f32>>);
pub struct Session {
    core: Option<Core>,
    last_use: std::time::Instant,
    /// Generic queries' vectors per task (search, code) and model, for measuring each item's hubness.
    probes: std::collections::HashMap<(bool, String), Vec<Vec<f32>>>,
    /// Why the last search matched contents by words only, if it did.
    note: Option<String>,
}
/// Queries that ask for nothing in particular. An item that scores well against these
/// scores well against everything (a near-blank image, a boilerplate page), so its
/// similarity to them is subtracted before it can count as related.
/// They name no kind of file, so a query that asks for pictures or recordings isn't
/// marked down for it.
const PROBES: [&str; 8] = ["something", "anything", "information", "a file", "an item", "data", "stuff", "content"];
pub fn validate_vector(vector: &[f32]) -> Result<()> {
    if vector.len() != aicore::DIMENSIONS || vector.iter().any(|n| !n.is_finite()) {
        bail!("invalid embedding shape or values");
    }
    let norm: f32 = vector.iter().map(|n| n * n).sum();
    if !(0.99..=1.01).contains(&norm) {
        bail!("embedding is not normalized");
    }
    Ok(())
}
fn database(p: &Paths) -> Result<rusqlite::Connection> {
    open_index(p, &live_index(p))
}
fn open_index(p: &Paths, file: &std::path::Path) -> Result<rusqlite::Connection> {
    use std::os::unix::fs::PermissionsExt;
    p.ensure()?;
    let dir = p.data.join("semantic-index");
    config::private_dir(&dir)?;
    if file.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
        bail!("semantic index is a symlink");
    }
    let db = rusqlite::Connection::open(file)?;
    fs::set_permissions(file, fs::Permissions::from_mode(0o600))?;
    db.busy_timeout(Duration::from_secs(3))?;
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA recursive_triggers=ON; CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value TEXT); CREATE TABLE IF NOT EXISTS documents(path TEXT PRIMARY KEY,stamp TEXT NOT NULL,status TEXT NOT NULL,cursor INTEGER NOT NULL DEFAULT 0,error TEXT); CREATE TABLE IF NOT EXISTS segments(id INTEGER PRIMARY KEY,path TEXT NOT NULL,ordinal INTEGER NOT NULL,kind TEXT NOT NULL,loc REAL,text TEXT NOT NULL,vector BLOB NOT NULL,code INTEGER NOT NULL DEFAULT 0,reader TEXT, UNIQUE(path,ordinal)); CREATE VIRTUAL TABLE IF NOT EXISTS words USING fts5(text); CREATE TRIGGER IF NOT EXISTS segment_add AFTER INSERT ON segments BEGIN INSERT INTO words(rowid,text) VALUES(new.id,new.text); END; CREATE TRIGGER IF NOT EXISTS segment_del AFTER DELETE ON segments BEGIN DELETE FROM words WHERE rowid=old.id; END;")?;
    Ok(db)
}
/// The model the index's vectors came from (`None` while it's empty).
fn identity(db: &rusqlite::Connection) -> Option<String> {
    db.query_row("SELECT value FROM meta WHERE key='identity'", [], |r| r.get(0)).ok()
}
fn set_identity(db: &rusqlite::Connection, identity: &str) -> Result<()> {
    db.execute("INSERT OR REPLACE INTO meta(key,value) VALUES('identity',?1)", [identity])?;
    Ok(())
}
fn pack(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|n| n.to_le_bytes()).collect()
}
fn unpack(bytes: &[u8]) -> Result<Vec<f32>> {
    if bytes.len() != aicore::DIMENSIONS * 4 {
        bail!("corrupt stored vector");
    }
    let v = bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect::<Vec<_>>();
    validate_vector(&v)?;
    Ok(v)
}
impl Session {
    pub fn start(_p: &Paths) -> Result<Self> {
        Ok(Self { core: None, last_use: std::time::Instant::now(), probes: Default::default(), note: None })
    }
    pub fn release_idle(&mut self) {
        if self.last_use.elapsed() > Duration::from_secs(30) {
            self.core = None;
        }
    }
    /// Why the last search matched contents by words only (AI Core down, the index
    /// being rebuilt for a new model), for the status line.
    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }
    /// The query's vector and the probes', when AI Core answers and its model is
    /// the one the index was built with.
    fn vectors(
        &mut self,
        p: &Paths,
        db: &rusqlite::Connection,
        code: bool,
        query: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<QueryVectors>> {
        let Some(stored) = identity(db) else { return Ok(None) };
        let task = if code { "code retrieval" } else { "search result" };
        let mut prompts = vec![format!("task: {task} | query: {query}")];
        let cached = self.probes.keys().find(|(c, id)| *c == code && aicore::compatible(&stored, id)).cloned();
        if cached.is_none() {
            prompts.extend(PROBES.iter().map(|probe| format!("task: {task} | query: {probe}")));
        }
        if self.core.is_none() {
            self.core = Some(Core::new()?);
        }
        let inputs: Vec<Input> = prompts.iter().map(|s| Input::Text(s)).collect();
        let result = self.core.as_mut().unwrap().embed(p, &inputs, Priority::Interactive, cancelled, Duration::from_secs(30));
        let (mut vectors, current) = match result {
            Ok(v) => v,
            Err(e) => {
                self.core = None;
                return Err(e);
            }
        };
        if !aicore::compatible(&stored, &current) {
            if fs::read_to_string(model_changed(p)).ok().as_deref() != Some(current.as_str()) {
                let _ = config::atomic_write(&model_changed(p), current.as_bytes());
            }
            return Ok(None);
        }
        for v in &vectors {
            validate_vector(v)?;
        }
        let vector = vectors.remove(0);
        let probes = match cached {
            Some(key) => self.probes[&key].clone(),
            None => {
                self.probes.insert((code, current), vectors.clone());
                vectors
            }
        };
        Ok(Some((vector, probes)))
    }
    /// Files whose contents match `q`: by meaning through Nyx AI Core, and by the
    /// words themselves, which needs no AI and keeps working when AI Core doesn't.
    pub fn search(&mut self, p: &Paths, q: &str, cancelled: impl Fn() -> bool) -> Result<Vec<Value>> {
        let cfg = p.load()?;
        self.note = None;
        if !cfg.semantic || q.trim().is_empty() || cfg.roots.is_empty() {
            return Ok(vec![]);
        }
        let db = database(p)?;
        let count: i64 = db.query_row("SELECT count(*) FROM segments", [], |r| r.get(0))?;
        if count == 0 {
            return Ok(vec![]);
        }
        self.release_idle();
        let code = q.trim().starts_with("code:");
        let query = if code { q.trim()[5..].trim() } else { q.trim().strip_prefix("semantic:").unwrap_or(q.trim()).trim() };
        let found = match self.vectors(p, &db, code, query, &cancelled) {
            Ok(Some(v)) => Some(v),
            Ok(None) => {
                self.note = Some("Rebuilding the content index for a new model; contents match by words for now".into());
                None
            }
            Err(_) if cancelled() => bail!("search cancelled"),
            Err(e) => {
                self.note = Some(format!("Contents match by words only: {}", aicore::explain(&e)));
                None
            }
        };
        self.last_use = std::time::Instant::now();
        let active = crate::plugins::list(p)?;
        let permitted = |path: &str, reader: &Option<String>, is_code: bool| {
            config::allowed(&cfg, std::path::Path::new(path))
                && (!is_code || cfg.code_roots.iter().any(|r| std::path::Path::new(path).starts_with(r)))
                && reader.as_ref().is_none_or(|id| {
                    active.iter().any(|plugin| {
                        plugin.manifest.id == *id
                            && plugin.grant.enabled
                            && plugin.grant.permissions.read.iter().any(|r| std::path::Path::new(path).starts_with(r))
                    })
                })
        };
        let mut fused = std::collections::HashMap::<String, (f64, Value)>::new();
        if let Some((vector, probes)) = found {
            let mut stmt = db.prepare("SELECT id,path,kind,loc,text,vector,code,reader FROM segments WHERE code=?1")?;
            // Best segment per file by lift: its similarity to the query minus its similarity to
            // queries for nothing in particular. Raw cosine favours "hubs" (tiny theme images,
            // boilerplate pages) that sit close to every query and turned up for everything.
            let mut best = std::collections::HashMap::<String, (f32, Value)>::new();
            for row in stmt.query_map([code], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<f64>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Vec<u8>>(5)?,
                    r.get::<_, bool>(6)?,
                    r.get::<_, Option<String>>(7)?,
                ))
            })? {
                if cancelled() {
                    bail!("search cancelled");
                }
                let (id, path, kind, loc, text, bytes, is_code, reader) = row?;
                if !permitted(&path, &reader, is_code) {
                    continue;
                }
                let stored = unpack(&bytes)?;
                let dot = |q: &[f32]| q.iter().zip(&stored).map(|(a, b)| a * b).sum::<f32>();
                let cosine = dot(&vector);
                let baseline = probes.iter().map(|p| dot(p)).sum::<f32>() / probes.len() as f32;
                let lift = cosine - baseline;
                if best.get(&path).is_none_or(|(score, _)| lift > *score) {
                    let file_kind = match kind.as_str() {
                        "page" | "page_image" => "pdf",
                        "image" => "image",
                        "audio" => "audio",
                        "frame" => "video",
                        _ => "doc",
                    };
                    best.insert(path.clone(),(lift,json!({"path":path,"kind":file_kind,"segment":kind,"loc":loc,"cosine":cosine,"lift":lift,"segment_id":id,"excerpt":text.chars().take(1000).collect::<String>(),"reader":reader})));
                }
            }
            // Clear standouts only. On this model related files lift 0.09–0.2 above the generic
            // baseline and unrelated ones stay under about 0.07. Sound matches text weakly, so a
            // single result far ahead of every other (0.06 or more) also counts.
            let mut ranked: Vec<_> = best.into_values().collect();
            ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
            let standout = match ranked.as_slice() {
                [(top, _), (next, _), ..] => *top > 0. && top - next >= 0.06,
                [(top, _)] => *top > 0.,
                [] => false,
            };
            let mut semantic: Vec<_> = ranked
                .into_iter()
                .enumerate()
                .filter(|(i, (lift, _))| *lift >= 0.09 || (*i == 0 && standout))
                .map(|(_, hit)| hit)
                .collect();
            if let Some((top, _)) = semantic.first() {
                let minimum = *top - 0.05;
                semantic.retain(|(lift, _)| *lift >= minimum);
            }
            for (rank, (_, mut hit)) in semantic.into_iter().take(5).enumerate() {
                hit["related"] = true.into();
                fused.insert(hit["path"].as_str().unwrap().into(), (1. / (60. + rank as f64), hit));
            }
        }
        // Words in a file's contents count only when they appear together, as typed, not
        // scattered across a 1,500-character chunk; prefixes are for names, not contents.
        let expression = crate::query::fts_near(query);
        if !expression.is_empty() {
            let mut words=db.prepare("SELECT s.path,s.kind,s.loc,s.text,s.reader,s.code FROM words JOIN segments s ON s.id=words.rowid WHERE words MATCH ?1 AND s.code=?2 ORDER BY bm25(words) LIMIT 100")?;
            let mut seen = std::collections::HashSet::new();
            let mut rank = 0;
            for row in words.query_map(rusqlite::params![expression, code], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<f64>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, bool>(5)?,
                ))
            })? {
                let (path, kind, loc, text, reader, is_code) = row?;
                if !permitted(&path, &reader, is_code) || !seen.insert(path.clone()) {
                    continue;
                }
                if seen.len() > 8 {
                    break;
                }
                let entry=fused.entry(path.clone()).or_insert_with(||(0.,json!({"path":path,"kind":if kind=="page"{"pdf"}else{"doc"},"segment":kind,"loc":loc,"excerpt":text.chars().take(1000).collect::<String>(),"reader":reader})));
                entry.0 += 1. / (60. + rank as f64);
                entry.1["keyword_rank"] = rank.into();
                entry.1["related"] = false.into();
                if rank == 0 && (kind == "page" || kind == "lines") {
                    entry.1["loc"] = json!(loc);
                }
                rank += 1;
            }
        }
        let current = p.load()?;
        let mut hits: Vec<_> = fused
            .into_values()
            .filter(|(_, v)| v["path"].as_str().is_some_and(|path| config::allowed(&current, std::path::Path::new(path))))
            .collect();
        hits.sort_by(|a, b| b.0.total_cmp(&a.0));
        Ok(hits
            .into_iter()
            .take(20)
            .map(|(score, mut v)| {
                v["score"] = score.into();
                v
            })
            .collect())
    }
}
pub fn search(p: &Paths, q: &str) -> Result<Vec<Value>> {
    Session::start(p)?.search(p, q, || false)
}
struct Piece {
    ordinal: usize,
    op: &'static str,
    input: String,
    text: String,
    kind: &'static str,
    loc: Option<f64>,
    reader: Option<String>,
}
fn extract(
    p: &Paths,
    path: &std::path::Path,
    c: &crate::config::Config,
    cursor: usize,
) -> Result<(Vec<Piece>, bool, bool, usize)> {
    let ext = path.extension().unwrap_or_default().to_string_lossy().to_lowercase();
    let code = c.code_roots.iter().any(|r| path.starts_with(r))
        && ["rs", "py", "js", "ts", "tsx", "jsx", "c", "cpp", "h", "go", "java", "sh", "swift", "qml", "lua", "rb"]
            .contains(&ext.as_str());
    let mut pieces = Vec::new();
    let scratch = p.state.join("extract");
    config::private_dir(&scratch)?;
    let capture = |program: &str, args: Vec<std::ffi::OsString>, limit: usize| -> Result<Vec<u8>> {
        let mut command = Command::new(program);
        command.args(args);
        let stop = process::throttled(Duration::from_millis(250), || {
            p.load().map_or(true, |now| now.paused || !config::allowed(&now, path))
        });
        process::capture_cancel(command, vec![], Duration::from_secs(10), limit, stop)
    };
    if ext == "pdf" {
        let bytes = capture("pdfinfo", vec![path.as_os_str().to_owned()], 65536)?;
        let info = String::from_utf8_lossy(&bytes);
        let total = info
            .lines()
            .find_map(|s| s.strip_prefix("Pages:").and_then(|s| s.trim().parse::<usize>().ok()))
            .context("PDF page count unavailable")?
            .min(5000);
        for page in (cursor + 1)..=(cursor + 8).min(total) {
            let text = String::from_utf8(capture(
                "pdftotext",
                vec![
                    "-f".into(),
                    page.to_string().into(),
                    "-l".into(),
                    page.to_string().into(),
                    path.as_os_str().to_owned(),
                    "-".into(),
                ],
                256 * 1024,
            )?)?;
            if text.chars().filter(|x| !x.is_whitespace()).count() >= 40 {
                let chars: Vec<char> = text.chars().collect();
                for (chunk_number, chunk) in chars.chunks(1500).enumerate() {
                    let text: String = chunk.iter().collect();
                    pieces.push(Piece {
                        ordinal: page * 10000 + chunk_number,
                        op: "text",
                        input: format!("title: none | text: {text}"),
                        text,
                        kind: "page",
                        loc: Some(page as f64),
                        reader: None,
                    });
                }
            } else {
                let output = scratch.join(format!("page-{page}"));
                capture(
                    "pdftoppm",
                    vec![
                        "-f".into(),
                        page.to_string().into(),
                        "-l".into(),
                        page.to_string().into(),
                        "-singlefile".into(),
                        "-scale-to".into(),
                        "768".into(),
                        "-png".into(),
                        path.as_os_str().to_owned(),
                        output.as_os_str().to_owned(),
                    ],
                    65536,
                )?;
                pieces.push(Piece {
                    ordinal: page * 10000,
                    op: "image",
                    input: output.with_extension("png").to_string_lossy().into_owned(),
                    text,
                    kind: "page_image",
                    loc: Some(page as f64),
                    reader: None,
                });
            }
        }
        return Ok((pieces, cursor + 8 >= total, false, (cursor + 8).min(total)));
    }
    if ["jpg", "jpeg", "png", "webp", "heic", "gif", "bmp", "tiff", "tif"].contains(&ext.as_str()) {
        let output = scratch.join("image.png");
        capture(
            "ffmpeg",
            vec![
                "-v".into(),
                "error".into(),
                "-protocol_whitelist".into(),
                "file,pipe".into(),
                "-i".into(),
                path.as_os_str().to_owned(),
                "-frames:v".into(),
                "1".into(),
                "-vf".into(),
                "scale=768:768:force_original_aspect_ratio=decrease".into(),
                "-y".into(),
                output.as_os_str().to_owned(),
            ],
            65536,
        )?;
        pieces.push(Piece {
            ordinal: 0,
            op: "image",
            input: output.to_string_lossy().into_owned(),
            text: String::new(),
            kind: "image",
            loc: None,
            reader: None,
        });
        if crate::plugins::on_path("tesseract")
            && let Ok(bytes) = capture("tesseract", vec![output.as_os_str().to_owned(), "stdout".into()], 65536)
        {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if !text.trim().is_empty() {
                pieces.push(Piece {
                    ordinal: 1,
                    op: "text",
                    input: format!("title: none | text: {}", text.chars().take(1500).collect::<String>()),
                    text,
                    kind: "ocr",
                    loc: None,
                    reader: None,
                });
            }
        }
        return Ok((pieces, true, false, 1));
    }
    if ["wav", "mp3", "flac", "ogg", "m4a", "aac", "mp4", "mkv", "webm", "mov", "avi"].contains(&ext.as_str()) {
        let video = ["mp4", "mkv", "webm", "mov", "avi"].contains(&ext.as_str());
        let bytes = capture(
            "ffprobe",
            vec![
                "-v".into(),
                "error".into(),
                "-protocol_whitelist".into(),
                "file,pipe".into(),
                "-show_format".into(),
                "-show_streams".into(),
                "-of".into(),
                "json".into(),
                path.as_os_str().to_owned(),
            ],
            65536,
        )?;
        let meta: Value = serde_json::from_slice(&bytes)?;
        let duration = meta["format"]["duration"]
            .as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|n| n.is_finite() && *n > 0.)
            .context("media duration unavailable")?;
        let has_audio = meta["streams"].as_array().is_some_and(|s| s.iter().any(|s| s["codec_type"] == "audio"));
        let step = if video { 3. } else { 25. };
        let total = (duration / step).ceil() as usize;
        for part in cursor..(cursor + 60).min(total) {
            let seconds = part as f64 * step;
            let output = scratch.join(if video { format!("frame-{part}.png") } else { format!("audio-{part}.wav") });
            let mut args: Vec<std::ffi::OsString> = vec![
                "-v".into(),
                "error".into(),
                "-protocol_whitelist".into(),
                "file,pipe".into(),
                "-ss".into(),
                seconds.to_string().into(),
                "-i".into(),
                path.as_os_str().to_owned(),
            ];
            if video {
                args.extend([
                    "-frames:v".into(),
                    "1".into(),
                    "-vf".into(),
                    "scale=768:768:force_original_aspect_ratio=decrease".into(),
                    "-f".into(),
                    "image2".into(),
                    "-vcodec".into(),
                    "png".into(),
                ]);
            } else {
                args.extend([
                    "-t".into(),
                    "30".into(),
                    "-ac".into(),
                    "1".into(),
                    "-ar".into(),
                    "16000".into(),
                    "-f".into(),
                    "wav".into(),
                ]);
            }
            args.extend(["-y".into(), output.as_os_str().to_owned()]);
            capture("ffmpeg", args, 65536)?;
            pieces.push(Piece {
                ordinal: part * 2,
                op: if video { "image" } else { "audio" },
                input: output.to_string_lossy().into_owned(),
                text: String::new(),
                kind: if video { "frame" } else { "audio" },
                loc: Some(seconds),
                reader: None,
            });
            if video && has_audio && part % 8 == 0 {
                let output = scratch.join(format!("track-{part}.wav"));
                capture(
                    "ffmpeg",
                    vec![
                        "-v".into(),
                        "error".into(),
                        "-protocol_whitelist".into(),
                        "file,pipe".into(),
                        "-ss".into(),
                        seconds.to_string().into(),
                        "-i".into(),
                        path.as_os_str().to_owned(),
                        "-t".into(),
                        "30".into(),
                        "-ac".into(),
                        "1".into(),
                        "-ar".into(),
                        "16000".into(),
                        "-y".into(),
                        output.as_os_str().to_owned(),
                    ],
                    65536,
                )?;
                pieces.push(Piece {
                    ordinal: part * 2 + 1,
                    op: "audio",
                    input: output.to_string_lossy().into_owned(),
                    text: String::new(),
                    kind: "audio",
                    loc: Some(seconds),
                    reader: None,
                });
            }
        }
        return Ok((pieces, cursor + 60 >= total, false, (cursor + 60).min(total)));
    }
    if code || ["txt", "md", "rst", "csv", "html", "htm", "rtf", "docx", "odt", "pptx", "xlsx"].contains(&ext.as_str()) {
        let text = if ["docx", "odt", "pptx", "xlsx"].contains(&ext.as_str()) {
            let member = match ext.as_str() {
                "docx" => "word/document.xml",
                "odt" => "content.xml",
                "pptx" => "ppt/slides/slide*.xml",
                _ => "xl/sharedStrings.xml",
            };
            let bytes = capture("unzip", vec!["-p".into(), path.as_os_str().to_owned(), member.into()], 2 * 1024 * 1024)?;
            strip_markup(&String::from_utf8_lossy(&bytes))
        } else {
            let mut bytes = Vec::new();
            fs::File::open(path)?.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
            if bytes.len() > 4 * 1024 * 1024 {
                bail!("text file exceeds extraction limit; use a reader plugin");
            }
            // Older text files are often Latin-1; index what can be read rather than failing.
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if ["html", "htm"].contains(&ext.as_str()) { strip_markup(&text) } else { text }
        };
        let chars: Vec<char> = text.chars().collect();
        let total = chars.len().div_ceil(1500);
        for part in cursor..(cursor + 128).min(total) {
            let start = part * 1500;
            let chunk: String = chars[start..(start + 1500).min(chars.len())].iter().collect();
            let line = chars[..start].iter().filter(|c| **c == '\n').count() + 1;
            pieces.push(Piece {
                ordinal: part,
                op: "text",
                input: format!("title: none | text: {chunk}"),
                text: chunk,
                kind: if code { "lines" } else { "chunk" },
                loc: if code { Some(line as f64) } else { None },
                reader: None,
            });
        }
        return Ok((pieces, cursor + 128 >= total, code, (cursor + 128).min(total)));
    }
    // Plugins add formats without changing the extractor.
    let content = crate::index::open(p)?;
    let mut rows = content.prepare("SELECT plugin,info,text FROM extracted WHERE path=?1")?;
    for row in rows.query_map([path.to_string_lossy().as_ref()], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
    })? {
        let (id, info, text) = row?;
        let plugin = crate::plugins::get(p, &id)?;
        if !plugin.grant.enabled {
            continue;
        }
        let info: Value = serde_json::from_str(&info)?;
        pieces.push(Piece {
            ordinal: pieces.len(),
            op: "text",
            input: format!("title: none | text: {}", text.chars().take(1500).collect::<String>()),
            text,
            kind: if info["page"].is_number() {
                "page"
            } else if info["line"].is_number() {
                "lines"
            } else {
                "chunk"
            },
            loc: info["page"].as_f64().or_else(|| info["line"].as_f64()).or_else(|| info["seconds"].as_f64()),
            reader: Some(id),
        });
    }
    Ok((pieces, true, false, 1))
}
/// Text of an XML/HTML document: tags become spaces and entities are decoded.
pub fn strip_markup(text: &str) -> String {
    let mut inside = false;
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '<' => {
                inside = true;
                out.push(' ');
            }
            '>' => inside = false,
            _ if !inside => out.push(ch),
            _ => {}
        }
    }
    decode_entities(&out)
}
/// Decodes named XML entities and numeric references in one pass, so "&amp;lt;" stays "&lt;".
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let decoded = rest[1..].find(';').filter(|end| *end <= 10).and_then(|end| {
            let name = &rest[1..=end];
            let ch = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                _ => name
                    .strip_prefix("#x")
                    .or_else(|| name.strip_prefix("#X"))
                    .map(|hex| u32::from_str_radix(hex, 16))
                    .or_else(|| name.strip_prefix('#').map(str::parse::<u32>))
                    .and_then(|n| n.ok())
                    .and_then(char::from_u32),
            }?;
            Some((ch, end + 2))
        });
        match decoded {
            Some((ch, len)) => {
                out.push(ch);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}
/// Scan the sources, then index their contents.
pub fn index(p: &Paths) -> Result<()> {
    let cfg = p.load()?;
    if cfg.semantic && !cfg.paused && !cfg.roots.is_empty() {
        crate::index::scan(p, &cfg)?;
    }
    index_scanned(p).map(|_| ())
}
/// Index contents of files the filename index already lists; the daemon scans first.
/// Returns true when work is left over (time budget used up, files part-done, a
/// rebuild under way), so the caller should run another pass soon.
///
/// Vectors come from Nyx AI Core. When its model's identity no longer matches the
/// index (another file, revision, projector or settings), the old index stays in use
/// for word matches while a new one is built beside it; the new one replaces it once
/// every file is in it, so vectors from two models never mix.
pub fn index_scanned(p: &Paths) -> Result<bool> {
    index_with(p, Core::new()?)
}
fn index_with(p: &Paths, mut core: Core) -> Result<bool> {
    use std::os::fd::AsRawFd;
    p.ensure()?;
    let lock = fs::OpenOptions::new().create(true).truncate(false).write(true).open(p.state.join("semantic.lock"))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Ok(false);
    }
    let cfg = p.load()?;
    if !cfg.semantic || cfg.paused || cfg.roots.is_empty() {
        return Ok(false);
    }
    if let Ok(seen) = fs::read_to_string(model_changed(p)) {
        let _ = fs::remove_file(model_changed(p));
        let building = if next_index(p).exists() { identity(&open_index(p, &next_index(p))?) } else { identity(&database(p)?) };
        if building.is_some_and(|b| !aicore::compatible(&b, &seen)) {
            return start_rebuild(p, &seen, next_index(p).exists());
        }
    }
    let rebuilding = next_index(p).exists();
    let mut db = if rebuilding { open_index(p, &next_index(p))? } else { database(p)? };
    let mut expected = identity(&db);
    let mut out_of_time = false;
    let mut finished = true;
    let source = crate::index::open(p)?;
    let documents: Vec<String> =
        db.prepare("SELECT path FROM documents")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    for path in documents {
        let exists: bool = source.query_row("SELECT EXISTS(SELECT 1 FROM files WHERE path=?1)", [&path], |r| r.get(0))?;
        if !exists || !config::allowed(&cfg, std::path::Path::new(&path)) {
            db.execute("DELETE FROM segments WHERE path=?1", [&path])?;
            db.execute("DELETE FROM documents WHERE path=?1", [&path])?;
        }
    }
    let files: Vec<(String, i64, i64)> = source
        .prepare("SELECT path,mtime,size FROM files ORDER BY path")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let started = std::time::Instant::now();
    let reader_version = crate::plugins::list(p)?
        .into_iter()
        .filter(|x| x.grant.enabled && x.manifest.capabilities.contains(&"reader".into()))
        .map(|x| format!("{}:{}", x.manifest.id, x.manifest.version))
        .collect::<Vec<_>>()
        .join("|");
    let mut current = cfg.clone();
    let mut loaded = std::time::Instant::now();
    // Set when AI Core's model differs from the index's: the identity to rebuild for.
    let mut changed_to: Option<String> = None;
    for (path, mtime, size) in files {
        // Settings are re-read at most once a second, not once per file.
        if loaded.elapsed() >= Duration::from_secs(1) {
            current = p.load()?;
            loaded = std::time::Instant::now();
        }
        if current.paused || !current.semantic || current.roots != cfg.roots || current.excluded != cfg.excluded {
            finished = false;
            break;
        }
        if started.elapsed() > Duration::from_secs(300) {
            out_of_time = true;
            finished = false;
            break;
        }
        let file = std::path::Path::new(&path);
        if !config::allowed(&current, file) || !file.is_file() {
            continue;
        }
        let stamp = format!("{mtime}:{size}:{}:{reader_version}", cfg.code_roots.iter().any(|r| file.starts_with(r)));
        let cached: Option<(String, String, i64)> = db
            .query_row("SELECT stamp,status,cursor FROM documents WHERE path=?1", [&path], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .ok();
        // A failed file is retried only once it changes, not on every pass.
        if cached.as_ref().is_some_and(|(s, state, _)| s == &stamp && (state == "done" || state == "error")) {
            continue;
        }
        let cursor = cached.as_ref().filter(|(s, _, _)| s == &stamp).map(|(_, _, n)| *n as usize).unwrap_or(0);
        if cursor == 0 {
            db.execute("DELETE FROM segments WHERE path=?1", [&path])?;
        }
        let cancelled = process::throttled(Duration::from_millis(250), || {
            p.load().map_or(true, |c| c.paused || !c.semantic || !config::allowed(&c, file))
        });
        // Only AI Core being down or busy stops the pass; a file that can't be read
        // or embedded waits until it changes.
        let mut core_fault = false;
        let result = (|| -> Result<(usize, bool)> {
            let (pieces, complete, code, next_cursor) = extract(p, file, &current, cursor)?;
            if pieces.is_empty() {
                return Ok((next_cursor, complete));
            }
            let inputs: Vec<Input> = pieces
                .iter()
                .map(|piece| match piece.op {
                    "image" => Input::Image(std::path::Path::new(&piece.input)),
                    "audio" => Input::Audio(std::path::Path::new(&piece.input)),
                    _ => Input::Text(&piece.input),
                })
                .collect();
            let (vectors, made_by) = core
                .embed(p, &inputs, Priority::Background, &cancelled, Duration::from_secs(600))
                .inspect_err(|e| core_fault = aicore::transient(e))?;
            match &expected {
                None => {
                    set_identity(&db, &made_by)?;
                    expected = Some(made_by);
                }
                Some(stored) if aicore::compatible(stored, &made_by) => {
                    // An index from Glimpse's own runtime takes AI Core's identical model's name.
                    if *stored != made_by {
                        set_identity(&db, &made_by)?;
                        expected = Some(made_by);
                    }
                }
                Some(_) => {
                    changed_to = Some(made_by);
                    bail!("the embedding model changed");
                }
            }
            for v in &vectors {
                validate_vector(v)?;
            }
            if cancelled() {
                bail!("source changed");
            }
            let meta = file.metadata()?;
            let now = meta.modified()?.duration_since(std::time::UNIX_EPOCH)?.as_secs();
            if now != mtime as u64 || meta.len() != size as u64 {
                bail!("file changed during extraction");
            }
            let tx = db.transaction()?;
            for (piece, vector) in pieces.into_iter().zip(vectors) {
                tx.execute("INSERT OR REPLACE INTO segments(path,ordinal,kind,loc,text,vector,code,reader) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",rusqlite::params![path,piece.ordinal as i64,piece.kind,piece.loc,piece.text,pack(&vector),code,piece.reader])?;
            }
            tx.commit()?;
            Ok((next_cursor, complete))
        })();
        match result {
            Ok((next_cursor, complete)) => {
                db.execute(
                    "INSERT OR REPLACE INTO documents(path,stamp,status,cursor,error) VALUES(?1,?2,?3,?4,NULL)",
                    rusqlite::params![path, stamp, if complete { "done" } else { "partial" }, next_cursor as i64],
                )?;
            }
            Err(e) => {
                // Paused, switched off or source removed: leave the document for a later pass.
                if cancelled() {
                    finished = false;
                    break;
                }
                if let Some(new) = changed_to.take() {
                    drop(db);
                    return start_rebuild(p, &new, rebuilding);
                }
                if core_fault {
                    write_status(p, json!({"event":"waiting","reason":aicore::explain(&e)}))?;
                    finished = false;
                    break;
                }
                db.execute(
                    "INSERT OR REPLACE INTO documents(path,stamp,status,cursor,error) VALUES(?1,?2,?3,?4,?5)",
                    rusqlite::params![path, stamp, "error", cursor as i64, e.to_string()],
                )?;
            }
        }
        write_status(
            p,
            json!({"event":if rebuilding {"rebuilding"} else {"indexing"},"files":db.query_row("SELECT count(*) FROM documents WHERE status='done'",[],|r|r.get::<_,i64>(0))?,"errors":db.query_row("SELECT count(*) FROM documents WHERE status='error'",[],|r|r.get::<_,i64>(0))?}),
        )?;
    }
    if p.state.join("extract").exists() {
        fs::remove_dir_all(p.state.join("extract"))?;
    }
    let partial: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM documents WHERE status IN ('partial','retry'))", [], |r| r.get(0))?;
    if rebuilding && finished && !partial {
        finish_rebuild(p, db)?;
        return Ok(false);
    }
    Ok(out_of_time || partial || rebuilding)
}

fn write_status(p: &Paths, status: Value) -> Result<()> {
    config::atomic_write(&p.state.join("semantic-status.json"), &serde_json::to_vec(&status)?)
}

/// AI Core's model is now `new`: build a fresh index for it beside the live one,
/// unless `new` is what the live index already has (a change reverted mid-rebuild).
fn start_rebuild(p: &Paths, new: &str, rebuilding: bool) -> Result<bool> {
    let next = next_index(p);
    for suffix in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", next.display()));
    }
    if rebuilding && identity(&database(p)?).is_some_and(|live| aicore::compatible(&live, new)) {
        write_status(p, json!({"event":"indexing"}))?;
        return Ok(true);
    }
    let db = open_index(p, &next)?;
    set_identity(&db, new)?;
    write_status(p, json!({"event":"rebuilding","files":0,"errors":0}))?;
    Ok(true)
}

/// Every file is in the new index: it becomes the live one.
fn finish_rebuild(p: &Paths, next: rusqlite::Connection) -> Result<()> {
    next.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")?;
    drop(next);
    let live = live_index(p);
    if let Ok(old) = rusqlite::Connection::open(&live) {
        let _ = old.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;");
    }
    fs::rename(next_index(p), &live)?;
    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", live.display()));
        let _ = fs::remove_file(format!("{}{suffix}", next_index(p).display()));
    }
    write_status(p, json!({"event":"indexing","rebuilt":true}))
}

#[cfg(test)]
mod tests {
    #[test]
    fn entities_decode_once() {
        assert_eq!(
            super::strip_markup("<w:t>a &amp;lt; b &quot;c&apos; &#233;&#x41; &bogus; & x</w:t>"),
            " a &lt; b \"c' éA &bogus; & x "
        );
    }

    use super::*;
    use crate::aicore::fake::{fake_core, identity as model_identity};

    #[test]
    fn old_indexes_stay_and_a_new_model_rebuilds_beside_them() {
        let dir = tempfile::Builder::new().prefix("glimpse-test-").tempdir().unwrap();
        let p = Paths { config: dir.path().join("config"), data: dir.path().join("data"), state: dir.path().join("state") };
        p.ensure().unwrap();
        let root = dir.path().join("files");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("notes.md"), "The contract renewal is due on November 3, before the deadline.").unwrap();
        let c = config::Config { roots: vec![root], semantic: true, battery_pause: false, ..Default::default() };
        p.save(&c).unwrap();
        crate::index::scan(&p, &c).unwrap();
        let core = |socket: &std::path::Path| Core::with_socket(socket).unwrap();
        let socket = |name: &str, pooling: &str| {
            let d = dir.path().join(name);
            fs::create_dir(&d).unwrap();
            fake_core(&d, model_identity(pooling))
        };

        // An index from Glimpse's own runtime takes AI Core's identical model as its own.
        set_identity(&database(&p).unwrap(), aicore::LEGACY_IDENTITY).unwrap();
        let same = socket("same", "mean");
        assert!(!index_with(&p, core(&same)).unwrap());
        let live = database(&p).unwrap();
        let first = aicore::legacy_equivalent();
        assert_eq!(identity(&live).as_deref(), Some(first.as_str()));
        assert!(live.query_row("SELECT count(*) FROM segments", [], |r| r.get::<_, i64>(0)).unwrap() > 0);

        // Another model: searches match contents by words and flag the change...
        let other = socket("other", "cls");
        let mut session = Session::start(&p).unwrap();
        session.core = Some(core(&other));
        let hits = session.search(&p, "contract renewal", || false).unwrap();
        assert!(hits.iter().any(|h| h["path"].as_str().unwrap().ends_with("notes.md")), "{hits:?}");
        assert!(session.note().unwrap().contains("Rebuilding"));
        // ...and the indexer builds a new index beside the live one, which stays.
        assert!(index_with(&p, core(&other)).unwrap());
        assert!(next_index(&p).exists());
        assert_eq!(identity(&database(&p).unwrap()).as_deref(), Some(first.as_str()));
        assert!(!index_with(&p, core(&other)).unwrap(), "every file is in the new index, which takes over");
        assert!(!next_index(&p).exists());
        let second = aicore::identity_key(&model_identity("cls"));
        assert_eq!(identity(&database(&p).unwrap()).as_deref(), Some(second.as_str()));

        // AI Core gone: contents still match by words, and the status says why.
        let mut session = Session::start(&p).unwrap();
        session.core = Some(core(&dir.path().join("gone.sock")));
        let hits = session.search(&p, "contract renewal", || false).unwrap();
        assert!(hits.iter().any(|h| h["path"].as_str().unwrap().ends_with("notes.md")));
        assert!(session.note().unwrap().contains("words only"));
    }
}
