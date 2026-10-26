//! The languages' servers: one per language
//! whose server pack is installed and checked, started in the background
//! when the service gets the packs. What each changed or re-linked file's
//! calls are is asked of its language's server, in the background, once
//! it's ready, by a confirmer of the language's own (a slow server holds up
//! only its language). Files whose calls no server has confirmed yet, as
//! the kept rows say, are asked too, so nothing waiting at shutdown is lost.
//! An answer that's a refusal is asked again later. The answers go to the
//! pipeline thread, which links the file again with them (they win over
//! names) and writes and publishes it. Each language's state, why when it
//! isn't ready, and its files and proven links are kept in `servers`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError, RwLock};
use std::time::Duration;

use arch_graph::{ComponentId, RepoPath, ServerState, ServerStatus};
use code_analyzer::rows::RefKind;
use code_analyzer::{PackSet, ServerAnswer};
use nah_lsp::{Change, Client, ServerSpec};

use crate::ServerLimits;
use crate::pipeline::{Jobs, Snapshot};

/// One language's server: running, or why not.
struct Language {
    language: String,
    server: String,
    /// The pack's state when it isn't usable.
    unusable: Option<(ServerState, String)>,
    client: Option<Client>,
}

/// What the servers and their confirmer share.
struct Shared {
    root: PathBuf,
    languages: Vec<Language>,
    /// Files waiting to be asked about.
    queue: Mutex<BTreeSet<RepoPath>>,
    /// Files being asked about now.
    asking: std::sync::atomic::AtomicUsize,
    /// Files whose answers are on their way to the pipeline: not queued
    /// again for still looking unanswered until they're in.
    answering: Arc<Mutex<BTreeSet<RepoPath>>>,
    /// The rows each file was last asked about: a file isn't asked again
    /// until it's linked again (what the server couldn't answer then, it
    /// can't now).
    asked: Mutex<HashMap<RepoPath, std::sync::Weak<code_analyzer::rows::FileRows>>>,
    /// Refused files: when each may be asked again, and how many times it was.
    retry: Mutex<BTreeMap<RepoPath, (std::time::Instant, u32)>>,
    /// The snapshot generation whose unconfirmed files were last queued.
    seeded: std::sync::atomic::AtomicU64,
    wake: Condvar,
    stop: AtomicBool,
    snapshot: Arc<RwLock<Arc<Snapshot>>>,
    jobs: Jobs,
    files_at_once: usize,
    /// How often a confirmer looks again when nothing is ready or queued.
    poll: Duration,
    /// The longest a refused file waits before it's asked again.
    retry_most: Duration,
}

#[derive(Default)]
pub(crate) struct Servers {
    shared: Option<Arc<Shared>>,
    confirmers: Vec<std::thread::JoinHandle<()>>,
}

/// Why a pack can't be used, in words that say what to do.
fn unusable(server: &str, state: nah_packs::State) -> Option<(ServerState, String)> {
    match state {
        nah_packs::State::Ready { .. } | nah_packs::State::Outdated { .. } => None,
        nah_packs::State::Missing => Some((ServerState::NotInstalled, format!("not installed: meta-harness pack install {server}"))),
        nah_packs::State::OutOfRange { found, supports } => {
            Some((ServerState::OutOfRange, format!("{found} isn't in the supported range {supports}")))
        }
        nah_packs::State::Broken(why) => Some((ServerState::Broken, why)),
    }
}

impl Servers {
    /// Starts the server of every language in `packs` that names one and
    /// has it installed (the rest are kept with why not), and the
    /// confirmer that asks them.
    pub(crate) fn start(
        catalog: &nah_packs::Catalog,
        packs: &PackSet,
        root: &Path,
        limits: &ServerLimits,
        snapshot: Arc<RwLock<Arc<Snapshot>>>,
        jobs: Jobs,
    ) -> Self {
        // Servers name places by their real paths: the root is taken as one too.
        let real = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let root = real.as_path();
        let lock = nah_packs::Lock::read(&catalog.lock_path);
        let mut languages = Vec::new();
        for pack in &packs.packs {
            let Some(server) = &pack.server else { continue };
            let mut language = Language { language: pack.name().to_owned(), server: server.clone(), unusable: None, client: None };
            let launch = lock.as_ref().map_err(|e| (ServerState::Broken, e.to_string())).and_then(|lock| {
                match nah_packs::state(catalog, lock, server).map(|s| unusable(server, s)) {
                    Ok(Some(why)) => Err(why),
                    Ok(None) => nah_packs::server_launch(catalog, lock, server, root).map_err(|e| (ServerState::Broken, e.to_string())),
                    Err(e) => Err((ServerState::Broken, e.to_string())),
                }
            });
            match launch {
                Ok(launch) => {
                    if let Err(e) = launch.keep_cache(root) {
                        tracing::warn!("{server}'s cache {}: {e}", launch.cache.display());
                    }
                    language.client = Some(Client::start(ServerSpec {
                        name: server.clone(),
                        program: launch.program,
                        args: launch.args,
                        env: launch.env,
                        root: root.to_path_buf(),
                        initialization_options: launch.initialization_options,
                        settings: launch.settings,
                        requires: launch.requires,
                        ready_timeout: launch.ready_timeout.min(Duration::from_secs(limits.ready_s)),
                        settle: launch.settle,
                        request_timeout: launch.request_timeout,
                        in_flight: limits.requests_in_flight,
                        backoff: (Duration::from_millis(limits.restart_after_ms), Duration::from_millis(limits.restart_after_most_ms)),
                        shutdown: Duration::from_millis(limits.shutdown_ms),
                    }));
                }
                Err(why) => language.unusable = Some(why),
            }
            languages.push(language);
        }
        let shared = Arc::new(Shared {
            root: root.to_path_buf(),
            languages,
            queue: Mutex::new(BTreeSet::new()),
            asking: std::sync::atomic::AtomicUsize::new(0),
            retry: Mutex::new(BTreeMap::new()),
            seeded: std::sync::atomic::AtomicU64::new(0),
            answering: Arc::default(),
            asked: Mutex::default(),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
            snapshot,
            jobs,
            files_at_once: limits.files_at_once.max(1),
            poll: Duration::from_millis(limits.poll_ms.max(1)),
            retry_most: Duration::from_millis(limits.retry_most_ms),
        });
        // A confirmer per language with a server; the first also keeps the states.
        let mut confirmers = Vec::new();
        let with_server: Vec<usize> = (0..shared.languages.len()).filter(|&i| shared.languages[i].client.is_some()).collect();
        for (n, &i) in with_server.iter().enumerate() {
            let shared = shared.clone();
            let name = format!("nah-confirm-{}", shared.languages[i].language);
            confirmers.extend(std::thread::Builder::new().name(name).spawn(move || confirm_loop(&shared, i, n == 0)).ok());
        }
        if with_server.is_empty() {
            let shared = shared.clone();
            confirmers.extend(std::thread::Builder::new().name("nah-servers".into()).spawn(move || keep_loop(&shared)).ok());
        }
        Self { shared: Some(shared), confirmers }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.shared.as_ref().is_none_or(|s| s.languages.is_empty())
    }

    /// Each language's server as clients see it, with its files and proven links.
    pub(crate) fn statuses(&self) -> Vec<ServerStatus> {
        let Some(shared) = &self.shared else { return Vec::new() };
        let snapshot = shared.snapshot.read().unwrap_or_else(PoisonError::into_inner).clone();
        counted(statuses(&shared.languages), &snapshot)
    }

    /// Tells each server of changes to files no language reads that it
    /// asked to hear about (a manifest like `Cargo.toml`, its own config).
    pub(crate) fn others_changed(&self, packs: &PackSet, paths: &[PathBuf]) {
        let Some(shared) = &self.shared else { return };
        for l in &shared.languages {
            let Some(client) = &l.client else { continue };
            let files: Vec<(PathBuf, Change)> = paths
                .iter()
                .filter(|p| packs.for_path(p).is_none() && client.watches(p))
                .map(|p| (p.clone(), if p.exists() { Change::Changed } else { Change::Deleted }))
                .collect();
            client.changed(&files);
        }
    }

    /// Tells each server which of its language's files changed on disk
    /// (`created`: those of them that are new).
    pub(crate) fn changed(&self, packs: &PackSet, files: &[RepoPath], created: &[RepoPath]) {
        let Some(shared) = &self.shared else { return };
        let paths: Vec<(PathBuf, bool)> = files.iter().map(|f| (shared.root.join(f.as_str()), created.contains(f))).collect();
        for l in &shared.languages {
            let Some(client) = &l.client else { continue };
            let files: Vec<(PathBuf, Change)> = paths
                .iter()
                .filter(|(p, _)| packs.for_path(p).is_some_and(|pack| pack.name() == l.language))
                .map(|(p, new)| {
                    let change = match (p.exists(), new) {
                        (false, _) => Change::Deleted,
                        (true, true) => Change::Created,
                        (true, false) => Change::Changed,
                    };
                    (p.clone(), change)
                })
                .collect();
            client.changed(&files);
        }
    }

    /// Waits up to `timeout` until every server is ready (or can't be) and
    /// nothing is left to ask; false if it timed out.
    pub(crate) fn wait(&self, timeout: Duration) -> bool {
        let Some(shared) = &self.shared else { return true };
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let starting = shared
                .languages
                .iter()
                .filter_map(|l| l.client.as_ref())
                .any(|c| matches!(c.state(), nah_lsp::ServerState::Starting | nah_lsp::ServerState::Restarting { .. }));
            let queued = !shared.queue.lock().unwrap_or_else(PoisonError::into_inner).is_empty();
            if !starting && !queued && shared.asking.load(Ordering::SeqCst) == 0 {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(shared.poll);
        }
    }

    /// Asks the servers about `files` (linked again), in the background.
    pub(crate) fn confirm(&self, files: &[RepoPath]) {
        let Some(shared) = &self.shared else { return };
        if files.is_empty() || shared.languages.iter().all(|l| l.client.is_none()) {
            return;
        }
        shared.queue.lock().unwrap_or_else(PoisonError::into_inner).extend(files.iter().cloned());
        shared.wake.notify_all();
    }
}

impl Drop for Servers {
    fn drop(&mut self) {
        if let Some(shared) = &self.shared {
            shared.stop.store(true, Ordering::SeqCst);
            shared.wake.notify_all();
        }
        for t in self.confirmers.drain(..) {
            let _ = t.join();
        }
    }
}

fn statuses(languages: &[Language]) -> Vec<ServerStatus> {
    languages
        .iter()
        .map(|l| {
            let mut status = ServerStatus { language: l.language.clone(), server: l.server.clone(), ..Default::default() };
            match (&l.unusable, &l.client) {
                (Some((state, why)), _) => (status.state, status.reason) = (*state, Some(why.clone())),
                (None, Some(client)) => {
                    status.dropped_positions = client.dropped_positions();
                    match client.state() {
                        nah_lsp::ServerState::Ready { version } => (status.state, status.version) = (ServerState::Ready, version),
                        nah_lsp::ServerState::Starting => {
                            (status.state, status.reason) = (ServerState::Starting, Some("loading the project".into()))
                        }
                        nah_lsp::ServerState::Restarting { reason } | nah_lsp::ServerState::Failed { reason } => {
                            (status.state, status.reason) = (ServerState::Failed, Some(reason))
                        }
                        nah_lsp::ServerState::Stopped => (status.state, status.reason) = (ServerState::Failed, Some("stopped".into())),
                    }
                }
                (None, None) => (status.state, status.reason) = (ServerState::Failed, Some("not started".into())),
            }
            status
        })
        .collect()
}

/// Each status with its language's files and the links its server proved.
fn counted(mut statuses: Vec<ServerStatus>, snapshot: &Snapshot) -> Vec<ServerStatus> {
    for status in &mut statuses {
        let files = snapshot.files.values().filter(|f| f.file.language == status.language);
        let (mut count, mut proven) = (0, 0);
        for f in files {
            count += 1;
            proven += f.refs.iter().filter(|r| r.evidence == code_analyzer::rows::Evidence::Server).count();
        }
        (status.files, status.proven) = (count, proven);
    }
    statuses
}

/// Files with calls no server has answered for yet (never asked, or asked
/// before a change), in languages with a server: what's left to ask.
fn unconfirmed(shared: &Shared, snapshot: &Snapshot) -> Vec<RepoPath> {
    let asked = shared.asked.lock().unwrap_or_else(PoisonError::into_inner);
    snapshot
        .files
        .values()
        .filter(|f| shared.languages.iter().any(|l| l.language == f.file.language && l.client.is_some()))
        .filter(|f| asked.get(&f.file.path).is_none_or(|was| !std::sync::Weak::ptr_eq(was, &Arc::downgrade(f))))
        .filter(|f| f.refs.iter().any(|r| asked_about(r) && !answered(r)))
        .map(|f| f.file.path.clone())
        .collect()
}

/// Whether a server is asked about `r`: a call with a name, a type named,
/// a field used, an import or a path written.
fn asked_about(r: &code_analyzer::rows::RefRow) -> bool {
    match &r.detail.call {
        Some((_, call)) => call.name_span.is_some(),
        None => matches!(r.kind, RefKind::Type | RefKind::Field | RefKind::Import | RefKind::Path),
    }
}

/// Whether a server's answer stands for a name: one it proved, or one it
/// said is outside the repo.
fn answered(r: &code_analyzer::rows::RefRow) -> bool {
    use code_analyzer::rows::Evidence;
    r.prover.is_some() && matches!(r.evidence, Evidence::Server | Evidence::External | Evidence::NotFound | Evidence::Local)
}

/// Keeps the servers' states, files and proven links as they change (a
/// job for the one writer).
fn keep(shared: &Shared, kept: &mut (Vec<ServerStatus>, u64)) {
    let snapshot = shared.snapshot.read().unwrap_or_else(PoisonError::into_inner).clone();
    let now = statuses(&shared.languages);
    if now == kept.0 && snapshot.generation == kept.1 {
        return;
    }
    if keep_statuses(shared, counted(now.clone(), &snapshot)) {
        *kept = (now, snapshot.generation);
    }
}

/// With no server running: the states are kept once the pipeline runs.
fn keep_loop(shared: &Shared) {
    let mut kept = (Vec::new(), u64::MAX);
    while !shared.stop.load(Ordering::SeqCst) {
        keep(shared, &mut kept);
        let queue = shared.queue.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = shared.wake.wait_timeout(queue, shared.poll * 10);
    }
}

/// Asks language `which`'s server about its queued files once it's ready, a
/// few at a time; `keeps`: also keeps the servers' states.
fn confirm_loop(shared: &Shared, which: usize, keeps: bool) {
    let language = &shared.languages[which];
    let Some(client) = &language.client else { return };
    let mut kept = (Vec::new(), u64::MAX);
    while !shared.stop.load(Ordering::SeqCst) {
        if keeps {
            keep(shared, &mut kept);
        }
        let snapshot = shared.snapshot.read().unwrap_or_else(PoisonError::into_inner).clone();
        // Each new version of the rows: what's still unconfirmed is queued
        // (after a restart, too: nothing waiting is lost).
        if snapshot.generation > 0 && shared.seeded.fetch_max(snapshot.generation, Ordering::SeqCst) < snapshot.generation {
            let answering = shared.answering.lock().unwrap_or_else(PoisonError::into_inner).clone();
            let left: Vec<RepoPath> = unconfirmed(shared, &snapshot).into_iter().filter(|f| !answering.contains(f)).collect();
            shared.queue.lock().unwrap_or_else(PoisonError::into_inner).extend(left);
            shared.wake.notify_all();
        }
        let batch: Vec<RepoPath> = {
            let mut queue = shared.queue.lock().unwrap_or_else(PoisonError::into_inner);
            // Files no server could answer for (no language, no server) leave.
            queue.retain(|f| {
                snapshot
                    .files
                    .get(f)
                    .is_some_and(|rows| shared.languages.iter().any(|l| l.language == rows.file.language && l.client.is_some()))
            });
            let now = std::time::Instant::now();
            let waiting = shared.retry.lock().unwrap_or_else(PoisonError::into_inner);
            let mine: Vec<RepoPath> = if client.is_ready() {
                queue
                    .iter()
                    .filter(|f| snapshot.files.get(*f).is_some_and(|r| r.file.language == language.language))
                    .filter(|f| waiting.get(*f).is_none_or(|(at, _)| *at <= now))
                    .take(shared.files_at_once)
                    .cloned()
                    .collect()
            } else {
                Vec::new()
            };
            drop(waiting);
            for f in &mine {
                queue.remove(f);
            }
            shared.asking.fetch_add(mine.len(), Ordering::SeqCst);
            if mine.is_empty() {
                let _ = shared.wake.wait_timeout(queue, shared.poll);
            }
            mine
        };
        for file in batch {
            match ask(shared, &file) {
                Ok(()) | Err(false) => {
                    shared.retry.lock().unwrap_or_else(PoisonError::into_inner).remove(&file);
                }
                // Refused, not ready, or lost: asked again, later each time.
                Err(true) => {
                    let mut retry = shared.retry.lock().unwrap_or_else(PoisonError::into_inner);
                    let tries = retry.get(&file).map_or(0, |(_, n)| *n) + 1;
                    let wait = (shared.poll * 2u32.saturating_pow(tries.min(16))).min(shared.retry_most);
                    retry.insert(file.clone(), (std::time::Instant::now() + wait, tries));
                    drop(retry);
                    shared.queue.lock().unwrap_or_else(PoisonError::into_inner).insert(file);
                }
            }
            shared.asking.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

fn client_for<'a>(shared: &'a Shared, snapshot: &Snapshot, file: &RepoPath) -> Option<&'a Client> {
    let language = &snapshot.files.get(file)?.file.language;
    shared.languages.iter().find(|l| &l.language == language)?.client.as_ref()
}

/// Asks the file's server what each of its calls is, and sends the answers
/// to the pipeline thread. Err(true): ask again later.
fn ask(shared: &Shared, file: &RepoPath) -> Result<(), bool> {
    let snapshot = shared.snapshot.read().unwrap_or_else(PoisonError::into_inner).clone();
    let rows = snapshot.files.get(file).ok_or(false)?;
    let client = client_for(shared, &snapshot, file).ok_or(false)?;
    let absolute = shared.root.join(file.as_str());
    let text = std::fs::read_to_string(&absolute).map_err(|_| false)?;
    // Changed since it was read: a newer change asks again.
    if code_analyzer::content_hash(&text) != rows.file.content_hash {
        return Err(false);
    }
    // What it's asked: each call (by its place among the calls), each type
    // named and each field used (by where its name starts).
    let calls: Vec<(Asked, u32)> = rows
        .refs
        .iter()
        .filter_map(|r| r.detail.call.as_ref().and_then(|(ci, call)| Some((Asked::Call(*ci), call.name_span?.start_byte))))
        .collect();
    let named: Vec<(Asked, u32)> = rows
        .refs
        .iter()
        .filter(|r| r.detail.call.is_none() && asked_about(r))
        .map(|r| (if r.kind == RefKind::Field { Asked::Field(r.start) } else { Asked::Type(r.start) }, r.start))
        .collect();
    let asked: Vec<(Asked, u32)> = calls.into_iter().chain(named).collect();
    shared.asked.lock().unwrap_or_else(PoisonError::into_inner).insert(file.clone(), Arc::downgrade(rows));
    if asked.is_empty() {
        return Ok(());
    }
    let at: Vec<usize> = asked.iter().map(|(_, start)| *start as usize).collect();
    let places = match client.definitions(&absolute, &text, &at) {
        Ok(places) => places,
        Err(_) => return Err(true),
    };
    // How each field use uses it (when the server says).
    let fields: Vec<usize> = asked.iter().filter(|(a, _)| matches!(a, Asked::Field(_))).map(|(_, start)| *start as usize).collect();
    let accesses: BTreeMap<usize, Option<code_analyzer::rows::Access>> = if fields.is_empty() || !client.answers_highlights() {
        BTreeMap::new()
    } else {
        match client.accesses(&absolute, &text, &fields) {
            Ok(found) => fields
                .iter()
                .copied()
                .zip(found.into_iter().map(|a| {
                    a.map(|a| match a {
                        nah_lsp::Access::Read => code_analyzer::rows::Access::Read,
                        nah_lsp::Access::Write => code_analyzer::rows::Access::Write,
                    })
                }))
                .collect(),
            Err(_) => return Err(true),
        }
    };
    // A target file read as it is now, but changed since the rows: its
    // answers wait for its own change to be linked (which asks again).
    let mut current: BTreeMap<RepoPath, bool> = BTreeMap::new();
    let mut answers = code_analyzer::Answers::default();
    let mut give = |asked: &Asked, answer: ServerAnswer| match asked {
        Asked::Call(ci) => {
            answers.calls.insert(*ci, answer);
        }
        Asked::Type(start) => {
            answers.at.insert(*start, (answer, None));
        }
        Asked::Field(start) => {
            answers.at.insert(*start, (answer, accesses.get(&(*start as usize)).copied().flatten()));
        }
    };
    for ((what, _), place) in asked.iter().zip(places) {
        let place = match place {
            nah_lsp::Definition::At(place) => place,
            // No definition: an answer too (nothing to link).
            nah_lsp::Definition::Nowhere => {
                give(what, ServerAnswer::Nothing);
                continue;
            }
            // A place that can't be read: no answer (counted).
            nah_lsp::Definition::Unreadable => continue,
        };
        let answer = match place.file.strip_prefix(&shared.root).ok().and_then(|p| RepoPath::new(&p.to_string_lossy()).ok()) {
            // In a repo file we don't read (ignored, or no language's): no
            // component of ours is there.
            Some(path) if !snapshot.files.contains_key(&path) => ServerAnswer::Local,
            // In the repo: the component defined there, if it's one we read.
            Some(path) => {
                // Changed since it was read: asked again once it's linked again.
                let same = *current.entry(path.clone()).or_insert_with(|| {
                    let kept = snapshot.files.get(&path).map(|r| r.file.content_hash.clone());
                    std::fs::read_to_string(shared.root.join(path.as_str())).ok().map(|t| code_analyzer::content_hash(&t)) == kept
                });
                if !same {
                    continue;
                }
                // A field is defined inside its type: that's what it names.
                let found = match what {
                    Asked::Field(_) => holding(&snapshot, &path, place.start as u32),
                    _ => defined_at(&snapshot, &path, place.start as u32),
                };
                match found {
                    Some(id) => ServerAnswer::At(id),
                    None => ServerAnswer::Local,
                }
            }
            // Outside the repo: a library's.
            None => ServerAnswer::Outside,
        };
        give(what, answer);
    }
    let status = statuses(&shared.languages);
    let version = status.iter().find(|s| s.language == rows.file.language).and_then(|s| s.version.clone());
    let server = status.iter().find(|s| s.language == rows.file.language).map(|s| s.server.clone()).unwrap_or_default();
    let prover = match version {
        Some(v) => format!("{server} {v}"),
        None => server,
    };
    let (file, hash) = (file.clone(), rows.file.content_hash.clone());
    let answering = shared.answering.clone();
    answering.lock().unwrap_or_else(PoisonError::into_inner).insert(file.clone());
    crate::pipeline::send(
        &shared.jobs,
        Box::new(move |core, model| {
            let _in = Answered(answering, file.clone());
            let _ = core.timed("confirm", file.as_str().to_owned(), |t| {
                let written = t.stage("link", || model.confirm(&core.packs, &core.types, &file, &hash, &answers, &prover));
                core.write(&written, t)?;
                if written.graph_changed() {
                    core.publish(model, t);
                    core.emit(arch_graph::ArchEvent::Changed {
                        reason: arch_graph::ChangeReason::Ingested,
                        plans: Vec::new(),
                        components: Vec::new(),
                    });
                }
                Ok(())
            });
        }),
    );
    Ok(())
}

/// Takes a file off the answering list when its answers are in (or the
/// job is dropped).
struct Answered(Arc<Mutex<BTreeSet<RepoPath>>>, RepoPath);

impl Drop for Answered {
    fn drop(&mut self) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).remove(&self.1);
    }
}

/// What a server is asked about in a file.
enum Asked {
    /// A call, by its place among the file's calls.
    Call(usize),
    /// A type named, an import or a path written, by where its name starts.
    Type(u32),
    /// A field used, by where its name starts.
    Field(u32),
}

/// The innermost component in `path` whose code holds `start` (a field's
/// definition: the type it's in).
fn holding(snapshot: &Snapshot, path: &RepoPath, start: u32) -> Option<ComponentId> {
    let rows = snapshot.files.get(path)?;
    rows.components
        .iter()
        .filter(|c| c.detail.def.is_some() && c.span.start_byte <= start && start < c.span.end_byte)
        .min_by_key(|c| c.span.end_byte - c.span.start_byte)
        .map(|c| c.id.clone())
}

/// The component whose name is written at `start` in `path`.
fn defined_at(snapshot: &Snapshot, path: &RepoPath, start: u32) -> Option<ComponentId> {
    let rows = snapshot.files.get(path)?;
    let named = rows.components.iter().find(|c| c.name_span.is_some_and(|s| s.start_byte == start)).map(|c| c.id.clone());
    // A module's name leads to its file's start: the file's own module.
    named.or_else(|| if start == 0 { rows.module().map(|m| m.id.clone()) } else { None })
}

/// Keeps the servers' states (a job for the one writer); false when the
/// pipeline hasn't started yet.
fn keep_statuses(shared: &Shared, statuses: Vec<ServerStatus>) -> bool {
    crate::pipeline::send(
        &shared.jobs,
        Box::new(move |core, _| {
            if let Err(e) = core.keep_servers(&statuses, &crate::service::now_rfc3339()) {
                tracing::warn!("keeping the servers' states failed: {e}");
            }
        }),
    )
}
