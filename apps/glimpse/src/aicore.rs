//! Glimpse's embeddings come from Nyx AI Core, the suite's shared model runtime
//! (nyx-ai's docs/GLIMPSE.md). Glimpse keeps its index, ranking and file reading and
//! names its model itself; AI Core loads EmbeddingGemma 2 and its projector and
//! schedules the work beside every other app's.

use crate::config::Paths;
use anyhow::{Context, Result, bail};
use base64::Engine as _;
use nyx_ai_client::Client;
use nyx_ai_client::proto::{self, Capability, EmbedInput, ErrorKind, Event, Model, ModelIdentity, ModelState, Priority};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

pub const MODEL_FILE: &str = "embeddinggemma-2-Q8_0.gguf";
pub const PROJECTOR_FILE: &str = "mmproj-embeddinggemma-2-Q8_0.gguf";
/// What AI Core downloads when Glimpse's own folder doesn't have the files.
pub const DOWNLOAD: &str = "ggml-org/embeddinggemma-2-GGUF:Q8_0";
/// Load settings that make AI Core's vectors match the index Glimpse has built.
pub const SETTINGS: [(&str, &str); 6] = [
    ("pooling", "mean"),
    ("ctx-size", "2048"),
    ("batch-size", "2048"),
    ("ubatch-size", "2048"),
    ("image-min-tokens", "140"),
    ("image-max-tokens", "280"),
];
/// The identity indexes built by Glimpse's own runtime carry.
pub const LEGACY_IDENTITY: &str = "glimpse-embeddinggemma2-q8-v1";
pub const DIMENSIONS: usize = 768;
/// Inputs per request, so a file's pages or frames don't make one huge request.
const BATCH: usize = 16;

/// A file's piece to embed: text, or an image or audio clip on disk.
pub enum Input<'a> {
    Text(&'a str),
    Image(&'a Path),
    Audio(&'a Path),
}

pub struct Core {
    rt: tokio::runtime::Runtime,
    client: Client,
    model: Option<String>,
}

impl Core {
    pub fn new() -> Result<Core> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        Ok(Core { rt, client: Client::new("nyx-glimpse"), model: None })
    }

    #[cfg(test)]
    pub fn with_socket(socket: &Path) -> Result<Core> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        Ok(Core { rt, client: Client::with_socket("nyx-glimpse", socket), model: None })
    }

    /// Whether AI Core's socket is there (it starts on the first request).
    pub fn available(&self) -> bool {
        self.client.available()
    }

    /// Run one call, giving up when `cancelled` says so or `timeout` passes. Giving
    /// up drops the request, which closes its connection and so cancels it in AI Core.
    fn run<T>(
        &self,
        work: impl std::future::Future<Output = nyx_ai_client::Result<T>>,
        cancelled: &dyn Fn() -> bool,
        timeout: Duration,
    ) -> Result<T> {
        self.rt.block_on(async {
            let mut work = std::pin::pin!(work);
            let deadline = tokio::time::Instant::now() + timeout;
            loop {
                tokio::select! {
                    r = &mut work => return r.map_err(anyhow::Error::from),
                    _ = tokio::time::sleep(Duration::from_millis(25)) => {
                        if cancelled() {
                            bail!("cancelled");
                        }
                        if tokio::time::Instant::now() > deadline {
                            return Err(proto::Error::new(ErrorKind::Timeout, "Nyx AI Core took too long to answer").into());
                        }
                    }
                }
            }
        })
    }

    /// The model Glimpse embeds with, as AI Core names it. Glimpse's own copy (from
    /// before AI Core) is offered to AI Core the first time it's needed, with the
    /// load settings the index was built with.
    pub fn model(&mut self, p: &Paths) -> Result<String> {
        if let Some(m) = &self.model {
            return Ok(m.clone());
        }
        let folder = p.data.join("Models");
        let mut models = self.run(self.client.list_models(), &|| false, Duration::from_secs(15))?;
        if pick(&models, &folder).is_none() && [MODEL_FILE, PROJECTOR_FILE].iter().all(|f| folder.join(f).is_file()) {
            if let Err(e) = self.run(self.client.add_model_dirs(vec![folder.display().to_string()]), &|| false, Duration::from_secs(60)) {
                eprintln!("Glimpse: Nyx AI Core didn't take Glimpse's model folder: {e}");
            }
            models = self.run(self.client.list_models(), &|| false, Duration::from_secs(15))?;
        }
        let id = pick(&models, &folder).ok_or_else(|| {
            proto::Error::new(ErrorKind::UnknownModel, "EmbeddingGemma 2 isn't in Nyx AI Core yet; turn on local AI in Glimpse Settings")
        })?;
        self.apply_settings(&id, &|| false);
        self.model = Some(id.clone());
        Ok(id)
    }

    /// Give `id` Glimpse's load settings where AI Core's differ. Untrusted on this
    /// computer, it can't; the vectors' identity then says what AI Core used.
    fn apply_settings(&self, id: &str, cancelled: &dyn Fn() -> bool) {
        let Ok(current) = self.run(self.client.model_settings(id), cancelled, Duration::from_secs(15)) else { return };
        let changes: BTreeMap<String, String> = SETTINGS
            .iter()
            .filter(|(k, v)| current.effective.get(*k).map(String::as_str) != Some(*v))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        if !changes.is_empty()
            && let Err(e) = self.run(self.client.configure_model(id, changes), cancelled, Duration::from_secs(15))
        {
            eprintln!("Glimpse: couldn't set EmbeddingGemma 2's load settings in Nyx AI Core: {e}");
        }
    }

    /// Vectors for `inputs` and the identity of the model that made them.
    pub fn embed(
        &mut self,
        p: &Paths,
        inputs: &[Input],
        priority: Priority,
        cancelled: &dyn Fn() -> bool,
        timeout: Duration,
    ) -> Result<(Vec<Vec<f32>>, String)> {
        let model = self.model(p)?;
        let mut vectors = Vec::with_capacity(inputs.len());
        let mut identity: Option<String> = None;
        for chunk in inputs.chunks(BATCH) {
            let wire = chunk.iter().map(wire_input).collect::<Result<Vec<_>>>()?;
            let e = match self.run(self.client.embed_inputs(&model, wire, priority), cancelled, timeout) {
                Ok(e) => e,
                Err(e) => {
                    // The model may have moved (a download replacing the folder copy).
                    if kind(&e) == Some(ErrorKind::UnknownModel) {
                        self.model = None;
                    }
                    return Err(e);
                }
            };
            if e.vectors.len() != chunk.len() {
                bail!("Nyx AI Core returned {} vectors for {} inputs", e.vectors.len(), chunk.len());
            }
            let key = identity_key(&e.identity);
            if *identity.get_or_insert_with(|| key.clone()) != key {
                bail!("the embedding model changed during the request");
            }
            vectors.extend(e.vectors);
        }
        Ok((vectors, identity.unwrap_or_default()))
    }

    /// Make EmbeddingGemma 2 available in AI Core with Glimpse's load settings: from
    /// Glimpse's own folder when it has both files (nothing is downloaded again),
    /// else downloaded by AI Core. `progress` gets bytes done and the total.
    pub fn prepare(&mut self, p: &Paths, cancelled: &dyn Fn() -> bool, progress: &dyn Fn(u64, u64)) -> Result<String> {
        if !self.available() {
            bail!("Nyx AI Core isn't installed or running; install it, then turn local AI on again");
        }
        self.model = None;
        let id = match self.model(p) {
            Ok(id) => id,
            Err(_) => {
                let download = async {
                    let mut stream = self.client.download_model(DOWNLOAD).await?;
                    while let Some(item) = stream.next().await {
                        match item? {
                            nyx_ai_client::Item::Event(Event::Progress { completed, total, .. }) => progress(completed, total),
                            nyx_ai_client::Item::Done(_) => return Ok(()),
                            nyx_ai_client::Item::Event(_) => {}
                        }
                    }
                    Err(proto::Error::new(ErrorKind::Unavailable, "Nyx AI Core stopped reporting the download"))
                };
                self.run(download, cancelled, Duration::from_secs(3 * 3600)).context("Couldn't download EmbeddingGemma 2")?;
                self.model(p)?
            }
        };
        Ok(id)
    }
}

/// What a shared action did when Glimpse ran it.
pub enum Outcome {
    Done(Value),
    /// It asks first: run it again with `confirmed` once the person agrees.
    Ask,
}

impl Core {
    /// The actions Nyx apps offer through AI Core's shared registry.
    pub fn actions(&self, cancelled: &dyn Fn() -> bool) -> Result<Vec<proto::ActionInfo>> {
        self.run(self.client.list_actions(), cancelled, Duration::from_secs(3))
    }

    /// Run a shared action. AI Core checks the arguments, Glimpse's standing and
    /// whether the action asks first; the owning app has the last word.
    pub fn invoke(&self, action: &str, arguments: Value, confirmed: bool) -> Result<Outcome> {
        match self.run(self.client.invoke_action(action, arguments, confirmed), &|| false, Duration::from_secs(310)) {
            Ok(value) => Ok(Outcome::Done(value)),
            Err(e) if kind(&e) == Some(ErrorKind::ConfirmationRequired) => Ok(Outcome::Ask),
            Err(e) => Err(e),
        }
    }

    /// Offer `actions` through AI Core and answer their calls with `handle` until
    /// AI Core ends the offer or the connection drops.
    pub fn serve(&self, actions: Vec<proto::ActionSpec>, handle: &dyn Fn(&str, &Value) -> std::result::Result<Value, String>) -> Result<()> {
        self.rt.block_on(async {
            let mut server = self.client.serve_actions(actions).await?;
            while let Some(call) = server.next().await {
                let call = call?;
                server.answer(call.id, handle(&call.action, &call.arguments)).await?;
            }
            Ok::<_, proto::Error>(())
        })?;
        Ok(())
    }
}

fn wire_input(input: &Input) -> Result<EmbedInput> {
    let b64 = |path: &Path| -> Result<String> {
        let bytes = std::fs::read(path).with_context(|| format!("couldn't read {}", path.display()))?;
        Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
    };
    Ok(match input {
        Input::Text(text) => EmbedInput::Text(text.to_string()),
        Input::Image(path) => EmbedInput::Image { image: b64(path)? },
        Input::Audio(path) => EmbedInput::Audio { audio: b64(path)? },
    })
}

/// EmbeddingGemma 2 among AI Core's models, Glimpse's own folder copy first.
pub fn pick(models: &[Model], folder: &Path) -> Option<String> {
    let mut found: Vec<&Model> = models
        .iter()
        .filter(|m| m.has(Capability::Embedding) && !matches!(m.state, ModelState::Missing | ModelState::Downloading))
        .filter(|m| m.path.as_deref().and_then(|p| Path::new(p).file_name()).is_some_and(|f| f == MODEL_FILE) || m.id == DOWNLOAD)
        .collect();
    found.sort_by_key(|m| (!m.path.as_deref().is_some_and(|p| Path::new(p).starts_with(folder)), m.id.clone()));
    found.first().map(|m| m.id.clone())
}

/// What makes vectors comparable, as one string to store with the index: the model
/// file, its revision and architecture, the projector and the settings that change
/// vectors. The id isn't part of it: the same file is the same model wherever it sits.
pub fn identity_key(id: &ModelIdentity) -> String {
    json!({
        "file": id.file,
        "size": id.size,
        "revision": id.revision,
        "architecture": id.architecture,
        "projector": id.projector,
        "settings": id.settings,
    })
    .to_string()
}

/// Whether an index built under `stored` can take vectors made under `current`.
/// Indexes from Glimpse's own runtime match AI Core's EmbeddingGemma 2 with
/// Glimpse's settings (cosine 0.9998 to 1.0 on text, images and audio).
pub fn compatible(stored: &str, current: &str) -> bool {
    stored == current || (stored == LEGACY_IDENTITY && current == legacy_equivalent())
}

pub fn legacy_equivalent() -> String {
    let settings: BTreeMap<String, String> =
        [("image-max-tokens", "280"), ("image-min-tokens", "140"), ("pooling", "mean")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    identity_key(&ModelIdentity {
        id: String::new(),
        file: MODEL_FILE.into(),
        size: 309855456,
        revision: None,
        architecture: Some("gemma-embedding2".into()),
        projector: Some(PROJECTOR_FILE.into()),
        settings,
    })
}

pub fn kind(e: &anyhow::Error) -> Option<ErrorKind> {
    e.downcast_ref::<proto::Error>().map(|e| e.kind)
}

/// AI Core is down or its model runner is: worth waiting for, not a bad file.
pub fn transient(e: &anyhow::Error) -> bool {
    matches!(
        kind(e),
        Some(
            ErrorKind::Unavailable
                | ErrorKind::WorkerUnavailable
                | ErrorKind::WorkerFailed
                | ErrorKind::ModelLoadFailed
                | ErrorKind::Timeout
                | ErrorKind::Busy
                | ErrorKind::ResourceExhausted
                | ErrorKind::UnknownModel
                | ErrorKind::IncompatibleVersion
                | ErrorKind::PermissionDenied
        )
    )
}

/// One line for Glimpse's status: what's wrong, in plain words.
pub fn explain(e: &anyhow::Error) -> String {
    match kind(e) {
        Some(ErrorKind::Unavailable) => "Nyx AI Core isn't running".into(),
        Some(ErrorKind::IncompatibleVersion) => "Glimpse and Nyx AI Core need updating together".into(),
        Some(ErrorKind::WorkerUnavailable | ErrorKind::WorkerFailed) => "Nyx AI Core's model runner is restarting".into(),
        Some(ErrorKind::Timeout | ErrorKind::Busy) => "Nyx AI Core is busy".into(),
        _ => e.to_string(),
    }
}

/// What Glimpse offers other apps (the Assistant) through the shared registry:
/// finding files, read-only.
pub fn offered() -> Vec<proto::ActionSpec> {
    vec![proto::ActionSpec {
        id: "glimpse.search".into(),
        description: "Find files in the folders Glimpse indexes, by name and by their contents (and by meaning when local AI is on). Returns paths only; opens nothing.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "minLength": 1, "maxLength": 200},
                "limit": {"type": "integer", "minimum": 1, "maximum": 50}
            },
            "required": ["query"],
            "additionalProperties": false
        }),
        output_schema: None,
        permissions: vec!["files.read".into()],
        confirmation: proto::Confirmation::Never,
        side_effects: proto::SideEffects::ReadOnly,
        timeout_ms: Some(15_000),
    }]
}

/// Answer a call to one of [`offered`]'s actions.
pub fn answer(p: &Paths, action: &str, arguments: &Value) -> std::result::Result<Value, String> {
    if action != "glimpse.search" {
        return Err(format!("Glimpse doesn't offer {action}"));
    }
    let query = arguments["query"].as_str().map(str::trim).filter(|q| !q.is_empty()).ok_or("a query is needed")?;
    let limit = arguments["limit"].as_u64().unwrap_or(10).clamp(1, 50) as usize;
    let c = p.load().map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut add = |path: &Path, how: &str| {
        if files.len() < limit && crate::config::allowed(&c, path) && seen.insert(path.to_path_buf()) {
            files.push(json!({"path": path, "name": path.file_name().map(|n| n.to_string_lossy()), "match": how}));
        }
    };
    for hit in crate::index::search(p, &c, query, limit).map_err(|e| e.to_string())? {
        add(&hit.path, "name");
    }
    for (hit, _) in crate::index::content_search(p, &c, query).unwrap_or_default() {
        add(&hit.path, "contents");
    }
    for hit in crate::semantic::search(p, query).unwrap_or_default() {
        if let Some(path) = hit["path"].as_str() {
            add(Path::new(path), if hit["related"] == true { "meaning" } else { "contents" });
        }
    }
    Ok(json!({"files": files}))
}

/// Offer Glimpse's actions while Nyx AI Core runs. It isn't started for this: AI
/// Core starts when an app needs a model, and the offer follows once it's up.
pub fn offer_while_running(p: Paths) {
    let running = || {
        std::process::Command::new("systemctl")
            .args(["--user", "is-active", "--quiet", "nyx-aid.service"])
            .status()
            .is_ok_and(|s| s.success())
    };
    loop {
        if running()
            && let Ok(core) = Core::new()
            && let Err(e) = core.serve(offered(), &|action, args| answer(&p, action, args))
        {
            eprintln!("Glimpse: offering actions to Nyx AI Core stopped: {e}");
        }
        std::thread::sleep(Duration::from_secs(60));
    }
}

/// For Settings and the command line.
pub fn status(p: &Paths) -> Value {
    let Ok(mut core) = Core::new() else { return json!({"ai_core": false}) };
    if !core.available() {
        return json!({"ai_core": false, "model": Value::Null});
    }
    match core.model(p) {
        Ok(id) => json!({"ai_core": true, "model": id}),
        Err(e) => json!({"ai_core": true, "model": Value::Null, "error": explain(&e)}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::fake::{fake_core, identity, model};

    #[test]
    fn picks_glimpse_s_own_copy_first() {
        let folder = Path::new("/home/u/.local/share/nyx-glimpse/Models");
        let models = vec![
            model(DOWNLOAD, "/home/u/.cache/huggingface/hub/x/embeddinggemma-2-Q8_0.gguf"),
            model("embeddinggemma-2-Q8_0", "/home/u/.local/share/nyx-glimpse/Models/embeddinggemma-2-Q8_0.gguf"),
            model("other-embed", "/m/nomic.gguf"),
        ];
        assert_eq!(pick(&models, folder).as_deref(), Some("embeddinggemma-2-Q8_0"));
        assert_eq!(pick(&models[..1], folder).as_deref(), Some(DOWNLOAD));
        assert_eq!(pick(&models[2..], folder), None);
    }

    #[test]
    fn offers_a_read_only_file_search() {
        let dir = tempfile::Builder::new().prefix("glimpse-test-").tempdir().unwrap();
        let p = Paths { config: dir.path().join("c"), data: dir.path().join("d"), state: dir.path().join("s") };
        p.ensure().unwrap();
        let root = dir.path().join("files");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("tax-return-2025.pdf"), b"%PDF").unwrap();
        let c = crate::config::Config { roots: vec![root.clone()], ..Default::default() };
        p.save(&c).unwrap();
        crate::index::scan(&p, &c).unwrap();
        let spec = &offered()[0];
        assert_eq!((spec.side_effects, spec.confirmation), (proto::SideEffects::ReadOnly, proto::Confirmation::Never));
        let found = answer(&p, "glimpse.search", &json!({"query": "tax return"})).unwrap();
        assert_eq!(found["files"][0]["path"], json!(root.join("tax-return-2025.pdf")));
        assert_eq!(found["files"][0]["match"], "name");
        assert!(answer(&p, "glimpse.search", &json!({"query": " "})).is_err());
        assert!(answer(&p, "glimpse.delete", &json!({})).is_err());
        // Out of Glimpse's folders, nothing.
        p.save(&crate::config::Config { roots: vec![], ..Default::default() }).unwrap();
        assert_eq!(answer(&p, "glimpse.search", &json!({"query": "tax"})).unwrap()["files"], json!([]));
    }

    #[test]
    fn old_indexes_match_ai_core_s_identical_model() {
        let same = legacy_equivalent();
        assert!(compatible(LEGACY_IDENTITY, &same));
        assert!(compatible(&same, &same));
        let other = same.replace("\"mean\"", "\"cls\"");
        assert!(!compatible(LEGACY_IDENTITY, &other));
        assert!(!compatible(&same, &other));
        let cached = same.replace("\"revision\":null", "\"revision\":\"abc\"");
        assert!(!compatible(LEGACY_IDENTITY, &cached));
    }

    #[test]
    fn embeds_through_ai_core_with_its_identity() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths { config: dir.path().join("c"), data: dir.path().join("d"), state: dir.path().join("s") };
        let id = identity("mean");
        let socket = fake_core(dir.path(), id);
        let mut core = Core::with_socket(&socket).unwrap();
        let image = dir.path().join("x.png");
        std::fs::write(&image, b"png").unwrap();
        let inputs: Vec<Input> = (0..20).map(|i| if i == 3 { Input::Image(&image) } else { Input::Text("hello") }).collect();
        let (vectors, identity) = core.embed(&p, &inputs, Priority::Background, &|| false, Duration::from_secs(5)).unwrap();
        assert_eq!(vectors.len(), 20);
        assert!(compatible(LEGACY_IDENTITY, &identity));
        let gone = Input::Image(Path::new("/no/such/file.png"));
        assert!(core.embed(&p, &[gone], Priority::Background, &|| false, Duration::from_secs(5)).is_err());
    }
}

/// A stand-in AI Core for tests.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use nyx_ai_client::proto::{ClientLine, Embeddings, Op, Reply, ServerLine, Welcome};
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    pub fn model(id: &str, path: &str) -> Model {
        serde_json::from_value(json!({"id": id, "name": id, "source": "folder", "path": path, "size": 1,
            "capabilities": ["embedding", "vision", "audio"], "state": "installed"}))
        .unwrap()
    }

    /// EmbeddingGemma 2 from Glimpse's folder, with `pooling` as its pooling.
    pub fn identity(pooling: &str) -> ModelIdentity {
        ModelIdentity {
            id: "embeddinggemma-2-Q8_0".into(),
            file: MODEL_FILE.into(),
            size: 309855456,
            revision: None,
            architecture: Some("gemma-embedding2".into()),
            projector: Some(PROJECTOR_FILE.into()),
            settings: [("pooling", pooling), ("image-min-tokens", "140"), ("image-max-tokens", "280")]
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    /// A stand-in AI Core: lists Glimpse's folder model and embeds every input as
    /// the same unit vector, under the identity it was started with.
    pub fn fake_core(dir: &Path, identity: ModelIdentity) -> std::path::PathBuf {
        let path = dir.join("ai.sock");
        let socket = path.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async move {
                let listener = tokio::net::UnixListener::bind(&socket).unwrap();
                std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
                ready_tx.send(()).unwrap();
                while let Ok((sock, _)) = listener.accept().await {
                    let identity = identity.clone();
                    tokio::spawn(async move {
                        let (read, mut write) = sock.into_split();
                        let mut lines = BufReader::new(read).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            let reply = match serde_json::from_str::<ClientLine>(&line).unwrap() {
                                ClientLine::Hello(_) => ServerLine::Hello(Welcome { api_version: proto::API_VERSION, server: "t".into(), client: "nyx-glimpse".into() }),
                                ClientLine::Request(r) => ServerLine::Result {
                                    id: r.id,
                                    result: match r.op {
                                        Op::ListModels {} => Reply::Models { models: vec![model("embeddinggemma-2-Q8_0", "/g/Models/embeddinggemma-2-Q8_0.gguf")] },
                                        Op::Embed { input, .. } => {
                                            let mut unit = vec![0.0; DIMENSIONS];
                                            unit[0] = 1.0;
                                            Reply::Embeddings(Embeddings { identity: identity.clone(), dimensions: DIMENSIONS, vectors: vec![unit; input.len()] })
                                        }
                                        _ => Reply::Ok,
                                    },
                                },
                            };
                            let mut s = serde_json::to_string(&reply).unwrap();
                            s.push('\n');
                            write.write_all(s.as_bytes()).await.unwrap();
                        }
                    });
                }
            });
        });
        ready_rx.recv().unwrap();
        path
    }

}
