//! The store as one SQLite file (`.nah/cache/index.sqlite`) with the plan's
//! tables. WAL mode: one connection writes
//! (the service's one writer); every other read opens its own.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use arch_graph::{ComponentId, RelationKind, RepoPath, ServerStatus, Span};
use code_analyzer::rows::{
    ComponentDetail, ComponentRow, Evidence, FileRow, FileRows, FlowRow, PlaceholderRow, RefKind, RefRow, RelationRow,
};
use code_analyzer::{FileWrite, RuleHit};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::{Loaded, STORE_VERSION, StoreError};

/// How long a busy connection waits before trying again, and in all (µs).
static BUSY_RETRY_US: AtomicU64 = AtomicU64::new(0);
static BUSY_GIVE_UP_US: AtomicU64 = AtomicU64::new(0);

/// SQLite settings (from the service's config).
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// `PRAGMA synchronous`: "normal" is safe with WAL and much faster than "full".
    pub synchronous: String,
    /// Page cache size in KiB.
    pub cache_kib: i64,
    /// How long a busy connection waits before trying again (µs), and in
    /// all (ms).
    pub busy_retry_us: u64,
    pub busy_give_up_ms: u64,
}

/// Sets a connection up the way every NAH database is: write-ahead log,
/// `synchronous` and cache size from the settings, and a busy wait in
/// microseconds.
pub fn configure(c: &Connection, settings: &Settings) -> rusqlite::Result<()> {
    c.pragma_update(None, "journal_mode", "wal")?;
    c.pragma_update(None, "synchronous", &settings.synchronous)?;
    c.pragma_update(None, "cache_size", -settings.cache_kib)?;
    // Busy (the writer checkpointing as a read starts): try again in a
    // moment. SQLite's own wait sleeps whole milliseconds, growing, so a
    // read would wait milliseconds for what takes microseconds.
    // SQLite takes the handler as a plain function, so its timings are
    // the process's: the last settings a connection was made with.
    BUSY_RETRY_US.store(settings.busy_retry_us, Ordering::Relaxed);
    BUSY_GIVE_UP_US.store(settings.busy_give_up_ms.saturating_mul(1000), Ordering::Relaxed);
    c.busy_handler(Some(|tries: i32| {
        let retry = BUSY_RETRY_US.load(Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_micros(retry));
        (tries as u64).saturating_mul(retry) < BUSY_GIVE_UP_US.load(Ordering::Relaxed)
    }))
}

pub struct SqliteStore {
    path: PathBuf,
    settings: Settings,
    /// Connected by its first use, on the thread that writes.
    writer: Mutex<Option<Connection>>,
    /// Whether the writer's layout was made ready.
    prepared: std::sync::atomic::AtomicBool,
    /// Read connections not in use (opened as needed, kept for the next read).
    readers: Mutex<Vec<Connection>>,
    /// Removed with its files when dropped (a store for one run).
    temporary: bool,
}

fn failed(e: impl std::fmt::Display) -> StoreError {
    StoreError::Failed(e.to_string())
}

const SCHEMA: &str = "
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE files (path TEXT PRIMARY KEY, language TEXT NOT NULL, size INTEGER NOT NULL, modified INTEGER NOT NULL, content_hash TEXT NOT NULL, read_version TEXT NOT NULL);
CREATE TABLE components (id TEXT PRIMARY KEY, file TEXT NOT NULL, parent TEXT, type TEXT NOT NULL, name TEXT NOT NULL, span_start INTEGER NOT NULL, span_end INTEGER NOT NULL, name_start INTEGER, name_end INTEGER, signature TEXT, detail TEXT NOT NULL);
CREATE INDEX components_file ON components (file);
CREATE INDEX components_name ON components (name);
CREATE TABLE refs (file TEXT NOT NULL, start INTEGER NOT NULL, end INTEGER NOT NULL, name TEXT NOT NULL, kind TEXT NOT NULL, within TEXT, target TEXT, evidence TEXT NOT NULL, prover TEXT, detail TEXT, PRIMARY KEY (file, start));
CREATE INDEX refs_target ON refs (target);
CREATE INDEX refs_name ON refs (name);
CREATE TABLE relations (file TEXT NOT NULL, kind TEXT NOT NULL, source TEXT NOT NULL, target TEXT NOT NULL, detail TEXT, evidence TEXT NOT NULL, prover TEXT);
CREATE INDEX relations_file ON relations (file);
CREATE INDEX relations_source ON relations (source);
CREATE INDEX relations_target ON relations (target);
CREATE TABLE flows (file TEXT NOT NULL, idx INTEGER NOT NULL, kind TEXT NOT NULL, start INTEGER NOT NULL, end INTEGER NOT NULL, within TEXT, boundary TEXT, what TEXT NOT NULL, detail TEXT NOT NULL);
CREATE INDEX flows_file ON flows (file);
CREATE TABLE placeholders (file TEXT NOT NULL, start INTEGER NOT NULL, end INTEGER NOT NULL, start_line INTEGER NOT NULL, end_line INTEGER NOT NULL, macro TEXT);
CREATE INDEX placeholders_file ON placeholders (file);
CREATE TABLE rule_hits (file TEXT NOT NULL, rule TEXT NOT NULL, start INTEGER NOT NULL, end INTEGER NOT NULL, start_line INTEGER NOT NULL, end_line INTEGER NOT NULL);
CREATE INDEX rule_hits_file ON rule_hits (file);
CREATE TABLE highlights (file TEXT PRIMARY KEY, runs BLOB NOT NULL);
CREATE TABLE servers (language TEXT PRIMARY KEY, server TEXT NOT NULL, version TEXT, state TEXT NOT NULL, reason TEXT, last_run TEXT, files INTEGER NOT NULL DEFAULT 0, proven INTEGER NOT NULL DEFAULT 0, dropped_positions INTEGER NOT NULL DEFAULT 0);
";

/// The tables a file's rows are in (all but meta and servers).
const FILE_TABLES: [(&str, &str); 8] = [
    ("files", "path"),
    ("components", "file"),
    ("refs", "file"),
    ("relations", "file"),
    ("flows", "file"),
    ("placeholders", "file"),
    ("rule_hits", "file"),
    ("highlights", "file"),
];

/// A component's detail as stored: what the plan's columns don't hold.
#[derive(Serialize, Deserialize)]
struct StoredDetail {
    #[serde(flatten)]
    detail: ComponentDetail,
    lines: (u32, u32),
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name_lines: Option<(u32, u32)>,
}

#[derive(Serialize, Deserialize)]
struct StoredFlow {
    flow: code_analyzer::RawFlow,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    receivers: Vec<ComponentId>,
}

fn json<T: Serialize>(value: &T) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(failed)
}

fn from_json<T: for<'de> Deserialize<'de>>(text: &str) -> Result<T, StoreError> {
    serde_json::from_str(text).map_err(failed)
}

fn kind_of(name: &str) -> Result<RelationKind, StoreError> {
    RelationKind::ALL.into_iter().find(|k| k.name() == name).ok_or_else(|| failed(format!("unknown relation kind {name}")))
}

/// A ref linked to a target: file, start, end, kind, within, evidence.
pub type RefTo = (String, u32, u32, String, Option<String>, String);

impl SqliteStore {
    /// The store at `path`, touching nothing: the first use of the writer
    /// creates or opens the file and makes its layout ready (one with
    /// another layout, or that can't be read, is thrown away and started
    /// again then), so that happens on the thread that writes.
    pub fn open(path: &Path, settings: &Settings) -> Self {
        Self {
            path: path.to_path_buf(),
            settings: settings.clone(),
            writer: Mutex::new(None),
            prepared: std::sync::atomic::AtomicBool::new(false),
            readers: Mutex::new(Vec::new()),
            temporary: false,
        }
    }

    /// A store in a temporary file, removed when dropped (for one run, like
    /// `meta-harness analyze`).
    pub fn temporary(settings: &Settings) -> Result<Self, StoreError> {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("nah-store-{}-{n}.sqlite", std::process::id()));
        Self::remove(&path);
        let mut store = Self::open(&path, settings);
        store.temporary = true;
        Ok(store)
    }

    /// The file and its write-ahead log.
    fn remove(path: &Path) {
        for suffix in ["", "-wal", "-shm"] {
            let mut p = path.as_os_str().to_owned();
            p.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(p));
        }
    }

    fn connect(path: &Path, settings: &Settings) -> Result<Connection, StoreError> {
        let c = Connection::open(path).map_err(failed)?;
        configure(&c, settings).map_err(failed)?;
        Ok(c)
    }

    /// The plan's tables, at this layout's version; any other is dropped.
    fn prepare(c: &Connection) -> Result<(), StoreError> {
        let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(failed)?;
        if version != STORE_VERSION {
            let tables: Vec<String> = c
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
                .map_err(failed)?
                .query_map([], |r| r.get(0))
                .map_err(failed)?
                .collect::<Result<_, _>>()
                .map_err(failed)?;
            for t in tables {
                c.execute_batch(&format!("DROP TABLE IF EXISTS \"{t}\"")).map_err(failed)?;
            }
            c.execute_batch(SCHEMA).map_err(failed)?;
            c.execute("INSERT INTO meta (key, value) VALUES ('schema_version', ?1)", params![STORE_VERSION.to_string()]).map_err(failed)?;
            c.pragma_update(None, "user_version", STORE_VERSION).map_err(failed)?;
        }
        Ok(())
    }

    /// The one connection that writes: connected, and its layout made
    /// ready, the first time.
    fn writer(&self) -> Result<Writer<'_>, StoreError> {
        let mut c = self.writer.lock().map_err(|_| failed("lock poisoned"))?;
        if !self.prepared.load(std::sync::atomic::Ordering::SeqCst) {
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir).map_err(failed)?;
            }
            let ready = Self::connect(&self.path, &self.settings).and_then(|w| Self::prepare(&w).map(|()| w));
            let w = match ready {
                Ok(w) => w,
                Err(_) => {
                    // Unreadable: thrown away and started again.
                    self.readers.lock().map_err(|_| failed("lock poisoned"))?.clear();
                    Self::remove(&self.path);
                    let w = Self::connect(&self.path, &self.settings)?;
                    Self::prepare(&w)?;
                    w
                }
            };
            *c = Some(w);
            self.prepared.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(Writer(c))
    }

    /// Runs `read` on a connection of its own (a read never waits on a write).
    fn read<T>(&self, read: impl FnOnce(&Connection) -> Result<T, StoreError>) -> Result<T, StoreError> {
        let kept = self.readers.lock().map_err(|_| failed("lock poisoned"))?.pop();
        let c = match kept {
            Some(c) => c,
            None => Self::connect(&self.path, &self.settings)?,
        };
        let out = read(&c);
        self.readers.lock().map_err(|_| failed("lock poisoned"))?.push(c);
        out
    }

    /// Every row (highlights aside).
    pub fn load(&self) -> Result<Loaded, StoreError> {
        let c = self.writer()?;
        let mut files: BTreeMap<String, FileRows> = BTreeMap::new();
        let mut statement = c.prepare("SELECT path, language, size, modified, content_hash, read_version FROM files").map_err(failed)?;
        let rows = statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                ))
            })
            .map_err(failed)?;
        for row in rows {
            let (path, language, size, modified, content_hash, read_version) = row.map_err(failed)?;
            let Ok(repo_path) = RepoPath::new(&path) else { continue };
            let file = FileRow { path: repo_path, language, size: size as u64, modified, content_hash, read_version };
            files.insert(
                path,
                FileRows {
                    file,
                    components: Vec::new(),
                    refs: Vec::new(),
                    relations: Vec::new(),
                    flows: Vec::new(),
                    placeholders: Vec::new(),
                    rule_hits: Vec::new(),
                    highlights: Default::default(),
                },
            );
        }
        let mut statement = c
            .prepare("SELECT file, id, parent, type, name, span_start, span_end, name_start, name_end, signature, detail FROM components")
            .map_err(failed)?;
        let mut rows = statement.query([]).map_err(failed)?;
        while let Some(r) = rows.next().map_err(failed)? {
            let file: String = r.get(0).map_err(failed)?;
            let Some(rows) = files.get_mut(&file) else { continue };
            let stored: StoredDetail = from_json(&r.get::<_, String>(10).map_err(failed)?)?;
            let id = ComponentId::for_symbol(&rows.file.language, &stored.detail.symbol);
            let (start, end): (i64, i64) = (r.get(5).map_err(failed)?, r.get(6).map_err(failed)?);
            let name_span =
                match (r.get::<_, Option<i64>>(7).map_err(failed)?, r.get::<_, Option<i64>>(8).map_err(failed)?, stored.name_lines) {
                    (Some(s), Some(e), Some((sl, el))) => {
                        Some(Span { start_line: sl, end_line: el, start_byte: s as u32, end_byte: e as u32 })
                    }
                    _ => None,
                };
            rows.components.push(ComponentRow {
                id,
                parent: r.get::<_, Option<String>>(2).map_err(failed)?.map(ComponentId::new),
                type_name: r.get(3).map_err(failed)?,
                name: r.get(4).map_err(failed)?,
                span: Span { start_line: stored.lines.0, end_line: stored.lines.1, start_byte: start as u32, end_byte: end as u32 },
                name_span,
                signature: r.get::<_, Option<String>>(9).map_err(failed)?.map(|s| from_json(&s)).transpose()?,
                detail: stored.detail,
            });
        }
        drop(rows);
        let mut statement = c
            .prepare("SELECT file, start, end, name, kind, within, target, evidence, prover, detail FROM refs ORDER BY file, start")
            .map_err(failed)?;
        let mut rows = statement.query([]).map_err(failed)?;
        while let Some(r) = rows.next().map_err(failed)? {
            let file: String = r.get(0).map_err(failed)?;
            let Some(rows) = files.get_mut(&file) else { continue };
            let kind: String = r.get(4).map_err(failed)?;
            let evidence: String = r.get(7).map_err(failed)?;
            rows.refs.push(RefRow {
                start: r.get::<_, i64>(1).map_err(failed)? as u32,
                end: r.get::<_, i64>(2).map_err(failed)? as u32,
                name: r.get(3).map_err(failed)?,
                kind: RefKind::from_name(&kind).ok_or_else(|| failed(format!("unknown ref kind {kind}")))?,
                within: r.get::<_, Option<String>>(5).map_err(failed)?.map(ComponentId::new),
                target: r.get::<_, Option<String>>(6).map_err(failed)?.map(ComponentId::new),
                evidence: Evidence::from_name(&evidence).ok_or_else(|| failed(format!("unknown evidence {evidence}")))?,
                prover: r.get(8).map_err(failed)?,
                detail: r.get::<_, Option<String>>(9).map_err(failed)?.map(|d| from_json(&d)).transpose()?.unwrap_or_default(),
            });
        }
        drop(rows);
        let mut statement = c.prepare("SELECT file, kind, source, target, detail, evidence, prover FROM relations").map_err(failed)?;
        let mut rows = statement.query([]).map_err(failed)?;
        while let Some(r) = rows.next().map_err(failed)? {
            let file: String = r.get(0).map_err(failed)?;
            let Some(rows) = files.get_mut(&file) else { continue };
            let evidence: String = r.get(5).map_err(failed)?;
            rows.relations.push(RelationRow {
                kind: kind_of(&r.get::<_, String>(1).map_err(failed)?)?,
                source: ComponentId::new(r.get::<_, String>(2).map_err(failed)?),
                target: ComponentId::new(r.get::<_, String>(3).map_err(failed)?),
                detail: r.get::<_, Option<String>>(4).map_err(failed)?.map(|d| from_json(&d)).transpose()?.unwrap_or_default(),
                evidence: Evidence::from_name(&evidence).ok_or_else(|| failed(format!("unknown evidence {evidence}")))?,
                prover: r.get(6).map_err(failed)?,
            });
        }
        drop(rows);
        let mut statement = c.prepare("SELECT file, idx, within, detail FROM flows ORDER BY file, idx").map_err(failed)?;
        let mut rows = statement.query([]).map_err(failed)?;
        while let Some(r) = rows.next().map_err(failed)? {
            let file: String = r.get(0).map_err(failed)?;
            let Some(rows) = files.get_mut(&file) else { continue };
            let stored: StoredFlow = from_json(&r.get::<_, String>(3).map_err(failed)?)?;
            rows.flows.push(FlowRow {
                index: r.get::<_, i64>(1).map_err(failed)? as usize,
                flow: stored.flow,
                within: r.get::<_, Option<String>>(2).map_err(failed)?.map(ComponentId::new),
                receivers: stored.receivers,
            });
        }
        drop(rows);
        let mut statement =
            c.prepare("SELECT file, start, end, start_line, end_line, macro FROM placeholders ORDER BY file, start").map_err(failed)?;
        let mut rows = statement.query([]).map_err(failed)?;
        while let Some(r) = rows.next().map_err(failed)? {
            let file: String = r.get(0).map_err(failed)?;
            let Some(rows) = files.get_mut(&file) else { continue };
            rows.placeholders.push(PlaceholderRow { span: span_of(r, 1)?, macro_name: r.get(5).map_err(failed)? });
        }
        drop(rows);
        let mut statement = c.prepare("SELECT file, rule, start, end, start_line, end_line FROM rule_hits").map_err(failed)?;
        let mut rows = statement.query([]).map_err(failed)?;
        while let Some(r) = rows.next().map_err(failed)? {
            let file: String = r.get(0).map_err(failed)?;
            let Some(rows) = files.get_mut(&file) else { continue };
            rows.rule_hits.push(RuleHit { rule: r.get(1).map_err(failed)?, span: span_of(r, 2)? });
        }
        drop(rows);
        // Rows come back in the order they were made.
        for rows in files.values_mut() {
            rows.components.sort_by_key(|c| c.detail.def.as_ref().map_or(0, |d| d.index + 1));
        }
        let fingerprints = c.query_row("SELECT value FROM meta WHERE key = 'fingerprints'", [], |r| r.get(0)).optional().map_err(failed)?;
        Ok(Loaded { files: files.into_values().collect(), fingerprints })
    }

    /// Writes a change in one transaction: for each file, its tables whose
    /// rows changed (deleted, then inserted); removed files' rows go.
    pub fn commit(&self, writes: &[FileWrite], removed: &[RepoPath], fingerprints: Option<&str>) -> Result<(), StoreError> {
        let mut c = self.writer()?;
        let tx = c.transaction().map_err(failed)?;
        for path in removed {
            for (table, column) in FILE_TABLES {
                tx.execute(&format!("DELETE FROM {table} WHERE {column} = ?1"), params![path.as_str()]).map_err(failed)?;
            }
        }
        for w in writes {
            write_file(&tx, w)?;
        }
        if let Some(f) = fingerprints {
            let kept: Option<String> =
                tx.query_row("SELECT value FROM meta WHERE key = 'fingerprints'", [], |r| r.get(0)).optional().map_err(failed)?;
            if kept.as_deref() != Some(f) {
                tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('fingerprints', ?1)", params![f]).map_err(failed)?;
            }
        }
        tx.commit().map_err(failed)
    }

    /// Replaces every file's rows with `writes`, in one transaction (a full
    /// re-index: readers see the old rows until it's done).
    pub fn replace_all(&self, writes: &[FileWrite], fingerprints: &str) -> Result<(), StoreError> {
        let mut c = self.writer()?;
        let tx = c.transaction().map_err(failed)?;
        for (table, _) in FILE_TABLES {
            tx.execute_batch(&format!("DELETE FROM {table}")).map_err(failed)?;
        }
        for w in writes {
            write_file(&tx, w)?;
        }
        tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('fingerprints', ?1)", params![fingerprints]).map_err(failed)?;
        tx.commit().map_err(failed)
    }

    /// One file's colours, on a connection of its own.
    pub fn highlights(&self, file: &RepoPath) -> Result<Option<code_analyzer::highlight::Highlights>, StoreError> {
        let runs: Option<Vec<u8>> = self.read(|c| {
            c.query_row("SELECT runs FROM highlights WHERE file = ?1", params![file.as_str()], |r| r.get(0)).optional().map_err(failed)
        })?;
        runs.map(|b| decode_runs(&b)).transpose()
    }

    /// The refs linked to `target` (file, start, end, kind, within, evidence),
    /// by file and place, on a connection of their own.
    pub fn refs_to(&self, target: &str) -> Result<Vec<RefTo>, StoreError> {
        self.read(|c| {
            let mut statement = c
                .prepare("SELECT file, start, \"end\", kind, within, evidence FROM refs WHERE target = ?1 ORDER BY file, start")
                .map_err(failed)?;
            let rows = statement
                .query_map(params![target], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
                .map_err(failed)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(failed)
        })
    }

    /// The servers' states, as last known.
    pub fn servers(&self) -> Result<Vec<ServerStatus>, StoreError> {
        self.read(|c| {
            let mut statement = c
                .prepare("SELECT language, server, version, state, reason, files, proven, dropped_positions FROM servers ORDER BY language")
                .map_err(failed)?;
            let rows = statement
                .query_map([], |r| {
                    let counts = (r.get::<_, i64>(5)? as usize, r.get::<_, i64>(6)? as usize, r.get::<_, i64>(7)? as usize);
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        counts,
                    ))
                })
                .map_err(failed)?;
            let mut out = Vec::new();
            for row in rows {
                let (language, server, version, state, reason, (files, proven, dropped_positions)) = row.map_err(failed)?;
                let state = serde_json::from_value(serde_json::Value::String(state)).map_err(failed)?;
                out.push(ServerStatus { language, server, state, version, reason, dropped_positions, files, proven });
            }
            Ok(out)
        })
    }

    /// Keeps the servers' states (only the rows that changed are written).
    pub fn put_servers(&self, statuses: &[ServerStatus], at: &str) -> Result<(), StoreError> {
        let kept: BTreeMap<String, ServerStatus> = self.servers()?.into_iter().map(|s| (s.language.clone(), s)).collect();
        let c = self.writer()?;
        for s in statuses {
            if kept.get(&s.language) == Some(s) {
                continue;
            }
            let state = serde_json::to_value(s.state).map_err(failed)?;
            c.execute(
                "INSERT OR REPLACE INTO servers (language, server, version, state, reason, last_run, files, proven, dropped_positions) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![s.language, s.server, s.version, state.as_str().unwrap_or_default(), s.reason, at, s.files as i64, s.proven as i64, s.dropped_positions as i64],
            )
            .map_err(failed)?;
        }
        Ok(())
    }

    /// The file and its write-ahead log.
    pub fn bytes(&self) -> u64 {
        ["", "-wal"]
            .iter()
            .map(|suffix| {
                let mut p = self.path.as_os_str().to_owned();
                p.push(suffix);
                std::fs::metadata(PathBuf::from(p)).map_or(0, |m| m.len())
            })
            .sum()
    }
}

/// The writer, connected.
struct Writer<'a>(std::sync::MutexGuard<'a, Option<Connection>>);

impl std::ops::Deref for Writer<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.0.as_ref().expect("the writer connects before it's handed out")
    }
}

impl std::ops::DerefMut for Writer<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        self.0.as_mut().expect("the writer connects before it's handed out")
    }
}

impl Drop for SqliteStore {
    fn drop(&mut self) {
        if self.temporary {
            Self::remove(&self.path);
        }
    }
}

/// Highlight runs as bytes: the capture names (one per line), a NUL, then
/// each run's start, end and name index as little-endian u32s.
fn encode_runs(h: &code_analyzer::highlight::Highlights) -> Vec<u8> {
    let mut out = h.names.join("\n").into_bytes();
    out.push(0);
    out.reserve(h.runs.len() * 12);
    for run in &h.runs {
        for n in run {
            out.extend_from_slice(&n.to_le_bytes());
        }
    }
    out
}

fn decode_runs(bytes: &[u8]) -> Result<code_analyzer::highlight::Highlights, StoreError> {
    let nul = bytes.iter().position(|b| *b == 0).ok_or_else(|| failed("highlights without names"))?;
    let names = std::str::from_utf8(&bytes[..nul]).map_err(failed)?;
    let names: Vec<String> = if names.is_empty() { Vec::new() } else { names.split('\n').map(str::to_owned).collect() };
    let body = &bytes[nul + 1..];
    if !body.len().is_multiple_of(12) {
        return Err(failed("highlight runs cut short"));
    }
    let word = |at: usize| u32::from_le_bytes([body[at], body[at + 1], body[at + 2], body[at + 3]]);
    let runs = (0..body.len() / 12).map(|i| [word(i * 12), word(i * 12 + 4), word(i * 12 + 8)]).collect();
    Ok(code_analyzer::highlight::Highlights { names, runs })
}

fn span_of(r: &rusqlite::Row<'_>, at: usize) -> Result<Span, StoreError> {
    Ok(Span {
        start_byte: r.get::<_, i64>(at).map_err(failed)? as u32,
        end_byte: r.get::<_, i64>(at + 1).map_err(failed)? as u32,
        start_line: r.get::<_, i64>(at + 2).map_err(failed)? as u32,
        end_line: r.get::<_, i64>(at + 3).map_err(failed)? as u32,
    })
}

/// One file's changed tables.
fn write_file(tx: &Transaction<'_>, w: &FileWrite) -> Result<(), StoreError> {
    let path = w.path.as_str();
    if let Some(f) = &w.file {
        let kept: Option<(String, i64, i64, String, String)> = tx
            .query_row("SELECT language, size, modified, content_hash, read_version FROM files WHERE path = ?1", params![path], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .optional()
            .map_err(failed)?;
        match kept {
            // Only the stamp: an update of those two columns.
            Some((language, _, _, hash, version)) if language == f.language && hash == f.content_hash && version == f.read_version => {
                tx.execute("UPDATE files SET size = ?2, modified = ?3 WHERE path = ?1", params![path, f.size as i64, f.modified])
                    .map_err(failed)?;
            }
            _ => {
                tx.execute(
                    "INSERT OR REPLACE INTO files (path, language, size, modified, content_hash, read_version) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![path, f.language, f.size as i64, f.modified, f.content_hash, f.read_version],
                )
                .map_err(failed)?;
            }
        }
    }
    if let Some(components) = &w.components {
        tx.execute("DELETE FROM components WHERE file = ?1", params![path]).map_err(failed)?;
        for c in components {
            let detail = json(&StoredDetail {
                detail: c.detail.clone(),
                lines: (c.span.start_line, c.span.end_line),
                name_lines: c.name_span.map(|s| (s.start_line, s.end_line)),
            })?;
            let signature = c.signature.as_ref().map(json).transpose()?;
            let values = |key: &str| {
                tx.execute(
                    "INSERT OR IGNORE INTO components (id, file, parent, type, name, span_start, span_end, name_start, name_end, signature, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        key,
                        path,
                        c.parent.as_ref().map(ComponentId::as_str),
                        c.type_name,
                        c.name,
                        c.span.start_byte,
                        c.span.end_byte,
                        c.name_span.map(|s| s.start_byte),
                        c.name_span.map(|s| s.end_byte),
                        signature,
                        detail
                    ],
                )
            };
            // The same id twice (one module in two files, a definition
            // under two `cfg`s): the later one's key says where it is.
            if values(c.id.as_str()).map_err(failed)? == 0 {
                values(&format!("{}#{path}:{}", c.id, c.span.start_byte)).map_err(failed)?;
            }
        }
    }
    if let Some(refs) = &w.refs {
        tx.execute("DELETE FROM refs WHERE file = ?1", params![path]).map_err(failed)?;
        let mut insert = tx.prepare_cached("INSERT INTO refs (file, start, end, name, kind, within, target, evidence, prover, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)").map_err(failed)?;
        for r in refs {
            let detail = (r.detail != Default::default()).then(|| json(&r.detail)).transpose()?;
            insert
                .execute(params![
                    path,
                    r.start,
                    r.end,
                    r.name,
                    r.kind.name(),
                    r.within.as_ref().map(ComponentId::as_str),
                    r.target.as_ref().map(ComponentId::as_str),
                    r.evidence.name(),
                    r.prover,
                    detail
                ])
                .map_err(failed)?;
        }
    }
    if let Some(relations) = &w.relations {
        tx.execute("DELETE FROM relations WHERE file = ?1", params![path]).map_err(failed)?;
        let mut insert = tx
            .prepare_cached(
                "INSERT INTO relations (file, kind, source, target, detail, evidence, prover) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .map_err(failed)?;
        for r in relations {
            let detail = (r.detail != Default::default()).then(|| json(&r.detail)).transpose()?;
            insert
                .execute(params![path, r.kind.name(), r.source.as_str(), r.target.as_str(), detail, r.evidence.name(), r.prover])
                .map_err(failed)?;
        }
    }
    if let Some(flows) = &w.flows {
        tx.execute("DELETE FROM flows WHERE file = ?1", params![path]).map_err(failed)?;
        let mut insert = tx.prepare_cached("INSERT INTO flows (file, idx, kind, start, end, within, boundary, what, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)").map_err(failed)?;
        for f in flows {
            let kind = serde_json::to_value(f.flow.kind).map_err(failed)?;
            let detail = json(&StoredFlow { flow: f.flow.clone(), receivers: f.receivers.clone() })?;
            insert
                .execute(params![
                    path,
                    f.index as i64,
                    kind.as_str().unwrap_or_default(),
                    f.flow.span.start_byte,
                    f.flow.span.end_byte,
                    f.within.as_ref().map(ComponentId::as_str),
                    f.flow.boundary,
                    f.flow.what,
                    detail
                ])
                .map_err(failed)?;
        }
    }
    if let Some(placeholders) = &w.placeholders {
        tx.execute("DELETE FROM placeholders WHERE file = ?1", params![path]).map_err(failed)?;
        for p in placeholders {
            tx.execute(
                "INSERT INTO placeholders (file, start, end, start_line, end_line, macro) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![path, p.span.start_byte, p.span.end_byte, p.span.start_line, p.span.end_line, p.macro_name],
            )
            .map_err(failed)?;
        }
    }
    if let Some(hits) = &w.rule_hits {
        tx.execute("DELETE FROM rule_hits WHERE file = ?1", params![path]).map_err(failed)?;
        for h in hits {
            tx.execute(
                "INSERT INTO rule_hits (file, rule, start, end, start_line, end_line) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![path, h.rule, h.span.start_byte, h.span.end_byte, h.span.start_line, h.span.end_line],
            )
            .map_err(failed)?;
        }
    }
    if let Some(highlights) = &w.highlights {
        let runs = encode_runs(highlights);
        tx.execute("INSERT OR REPLACE INTO highlights (file, runs) VALUES (?1, ?2)", params![path, runs]).map_err(failed)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings { synchronous: "normal".into(), cache_kib: 8192, busy_retry_us: 50, busy_give_up_ms: 10_000 }
    }

    #[test]
    fn a_store_with_another_layout_or_that_cant_be_read_is_made_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.sqlite");
        let store = SqliteStore::open(&path, &settings());
        assert!(!path.exists(), "opening touches nothing");
        store.commit(&[], &[], Some("f1")).unwrap();
        assert_eq!(store.load().unwrap().fingerprints.as_deref(), Some("f1"));
        store.writer().unwrap().pragma_update(None, "user_version", 1).unwrap();
        drop(store);
        assert_eq!(SqliteStore::open(&path, &settings()).load().unwrap(), Loaded::default(), "another layout: made again");
        // Not a database at all.
        SqliteStore::remove(&path);
        std::fs::write(&path, "this is not a database, just bytes, enough of them to look like a header").unwrap();
        let store = SqliteStore::open(&path, &settings());
        assert_eq!(store.load().unwrap(), Loaded::default(), "started again");
    }
}
