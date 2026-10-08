//! The combine stage over the Rust fixture, with what an indexer would say
//! about it written here: calls it resolves become proven (replacing name
//! matches, linking what names couldn't), the rest stay as they were, and
//! nothing read from the code is lost.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use arch_graph::{ComponentId, RelationKind, RepoPath, TypeSystem};
use code_analyzer::index::{IndexedFile, ROLE_DEFINITION};
use code_analyzer::{CodeRules, FileExtract, PackSet, Resolved, combine};

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rust/sample")
}

fn packs() -> PackSet {
    PackSet::from_shipped_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs")).unwrap()
}

fn id(s: &str) -> ComponentId {
    ComponentId::for_symbol("rust", s)
}

/// What an indexer says about one file: `(text, nth occurrence, symbol, is a definition)`.
fn indexed(ex: &FileExtract, places: &[(&str, usize, &str, bool)]) -> IndexedFile {
    let text = std::fs::read_to_string(sample().join(ex.file.as_str())).unwrap();
    let mut file = IndexedFile { path: ex.file.clone(), indexer: "fake 1.0".into(), content_hash: ex.content_hash.clone(), symbols: Vec::new(), occurrences: Vec::new(), defined: Vec::new() };
    for (needle, nth, symbol, def) in places {
        let start = text.match_indices(needle).nth(*nth).unwrap_or_else(|| panic!("{needle} #{nth} in {}", ex.file)).0 as u32;
        let at = file.symbols.iter().position(|s| s == symbol).unwrap_or_else(|| {
            file.symbols.push((*symbol).to_owned());
            file.symbols.len() - 1
        });
        // The name is what's matched: up to a call's parenthesis.
        let name = needle.split('(').next().unwrap_or(needle);
        file.occurrences.push([start, start + name.len() as u32, at as u32, if *def { ROLE_DEFINITION } else { 0 }]);
    }
    file.occurrences.sort();
    file
}

fn read() -> (Vec<FileExtract>, PackSet, TypeSystem) {
    let packs = packs();
    let types = TypeSystem::defaults().unwrap();
    let extracts = code_analyzer::analyze(&sample(), Path::new("."), &packs, &CodeRules::default(), &types, None).unwrap().extracts;
    (extracts, packs, types)
}

fn file<'a>(extracts: &'a [FileExtract], path: &str) -> &'a FileExtract {
    extracts.iter().find(|e| e.file.as_str() == path).unwrap()
}

const UTIL_HELPER: &str = "rust-analyzer cargo sample 0.1.0 util/helper().";
const STORE_SAVE: &str = "rust-analyzer cargo sample 0.1.0 store/Store#save().";
const STD_LOG: &str = "rust-analyzer cargo log 0.4.0 log().";

fn index(extracts: &[FileExtract]) -> Vec<IndexedFile> {
    vec![
        indexed(file(extracts, "src/util.rs"), &[("helper", 0, UTIL_HELPER, true)]),
        indexed(file(extracts, "src/store.rs"), &[("save", 0, STORE_SAVE, true), ("save", 1, STORE_SAVE, false)]),
        indexed(
            file(extracts, "src/lib.rs"),
            &[
                // `helper()` in ambiguous(): two helpers share the name; the indexer knows which.
                ("helper", 0, UTIL_HELPER, false),
                // `store.save(&out)` in run(): what names found too.
                ("save", 0, STORE_SAVE, false),
                // `log(&out)` in run(): the indexer says it's another crate's log, not ours.
                ("log(&out)", 0, STD_LOG, false),
            ],
        ),
    ]
}

fn calls(r: &Resolved) -> BTreeSet<(String, String, Option<String>)> {
    r.graph.relations().filter(|r| r.kind == RelationKind::Calls).map(|r| (r.source.to_string(), r.target.to_string(), r.proven_by.clone())).collect()
}

fn call(r: &Resolved, from: &str, to: &str) -> Option<Option<String>> {
    r.graph.relations().find(|r| r.kind == RelationKind::Calls && r.source == id(from) && r.target == id(to)).map(|r| r.proven_by.clone())
}

// req: AP.8
#[test]
fn an_indexers_references_make_calls_proven_and_nothing_read_is_lost() {
    let (extracts, packs, types) = read();
    let plain = combine(&extracts, &[], &packs, &types);
    let text = std::fs::read_to_string(sample().join("src/lib.rs")).unwrap();
    let r = combine(&extracts, &index(&extracts), &packs, &types);

    // Calls the indexer resolved are proven: one names couldn't choose...
    assert_eq!(plain.ambiguous.len(), 1);
    assert!(r.ambiguous.is_empty(), "{:?}", r.ambiguous);
    assert_eq!(call(&r, "sample::ambiguous", "sample::util::helper"), Some(Some("fake 1.0".into())));
    assert_eq!(call(&r, "sample::ambiguous", "sample::store::helper"), None);
    // ...one names couldn't prove (the receiver comes from a loop)...
    assert_eq!(plain.unproven.len(), 1);
    assert!(r.unproven.is_empty());
    assert_eq!(call(&r, "sample::store::store_each", "sample::store::Store::save"), Some(Some("fake 1.0".into())));
    // ...one names found too...
    assert_eq!(call(&plain, "sample::run", "sample::store::Store::save"), Some(None), "matched by name, unconfirmed");
    assert_eq!(call(&r, "sample::run", "sample::store::Store::save"), Some(Some("fake 1.0".into())));
    // ...and one names got wrong: the indexer says it's code outside the repo.
    assert_eq!(call(&plain, "sample::run", "sample::log"), Some(None));
    assert_eq!(call(&r, "sample::run", "sample::log"), None, "replaced, not kept beside it");
    assert!(r.external.iter().any(|e| e.caller == id("sample::run") && e.callee == "log"));
    // Calls the indexer didn't speak to stay as names found them.
    assert_eq!(call(&r, "sample::yelled", "sample::util::shout"), Some(None));

    // Nothing else read from the code is lost.
    let others = |x: &Resolved| -> BTreeSet<String> { x.graph.relations().filter(|r| r.kind != RelationKind::Calls).map(|r| r.id().0).collect() };
    let missing: Vec<String> = others(&plain).difference(&others(&r)).cloned().collect();
    assert_eq!(missing, ["data_flow:rust:sample::Upper::apply->rust:sample::log:return->msg"], "only the flow into the wrong guess goes with it");
    let lost: Vec<_> = calls(&plain).into_iter().filter(|(a, b, _)| call(&r, a.trim_start_matches("rust:"), b.trim_start_matches("rust:")).is_none()).collect();
    assert_eq!(lost, [(id("sample::run").to_string(), id("sample::log").to_string(), None)], "only the wrong guess is gone");
    assert_eq!(plain.graph.components().count(), r.graph.components().count());

    // What each named place refers to, for the code viewer.
    let lib_places = &r.occurrences[&RepoPath::new("src/lib.rs").unwrap()];
    let helper_at = text.find("helper").unwrap() as u32;
    let place = lib_places.iter().find(|o| o.start == helper_at).unwrap();
    assert_eq!((place.target.clone(), place.symbol.as_deref(), place.definition), (Some(id("sample::util::helper")), Some(UTIL_HELPER), false));
    let plain_places = &plain.occurrences[&RepoPath::new("src/lib.rs").unwrap()];
    assert!(plain_places.iter().any(|o| o.definition && o.target == Some(id("sample::run"))), "without an indexer: ours");
}

// req: AP.8
#[test]
fn a_file_changed_since_it_was_indexed_uses_none_of_it() {
    let (extracts, packs, types) = read();
    let mut stale = index(&extracts);
    for f in &mut stale {
        f.content_hash = "indexed-before-an-edit".into();
    }
    let r = combine(&extracts, &stale, &packs, &types);
    assert_eq!(r.ambiguous.len(), 1, "matched by name, as without an indexer");
    assert!(r.graph.relations().all(|r| r.proven_by.is_none()));
}
