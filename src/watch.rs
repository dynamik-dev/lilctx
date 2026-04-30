//! File-tree watcher. Debounces editor save-bursts, then reindexes changed files
//! and removes deleted ones. Honors a basic ignored-dir list; for true gitignore
//! semantics, build an `ignore::gitignore::Gitignore` per root and check here.

use anyhow::Result;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;

use crate::{config::Config, embed::OpenRouterEmbedder, index, store::Store};

const DEBOUNCE_MS: u64 = 500;

pub(crate) async fn run(cfg: Config) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<notify::Event>();

    // notify's callback is sync — push into a tokio channel from there.
    let mut watcher: RecommendedWatcher =
        notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        })?;

    for path in &cfg.paths {
        watcher.watch(path, RecursiveMode::Recursive)?;
        eprintln!("watching {}", path.display());
    }

    let store = Store::open(&cfg).await?;
    let embedder = OpenRouterEmbedder::new(&cfg)?;

    let mut pending_changed: HashSet<PathBuf> = HashSet::new();
    let mut pending_removed: HashSet<PathBuf> = HashSet::new();

    loop {
        // Wait for the first event (no timer running).
        let Some(event) = rx.recv().await else { break };
        absorb(&mut pending_changed, &mut pending_removed, event);

        // Then collect more events until DEBOUNCE_MS of silence.
        loop {
            match tokio::time::timeout(Duration::from_millis(DEBOUNCE_MS), rx.recv()).await {
                Ok(Some(e)) => absorb(&mut pending_changed, &mut pending_removed, e),
                Ok(None) => return Ok(()), // channel closed
                Err(_) => break,           // timeout — flush
            }
        }

        // Removes first so a rapid remove+create on the same path lands on a clean slate.
        for p in pending_removed.drain() {
            let path_str = p.to_string_lossy().to_string();
            match store.delete_path(&path_str).await {
                Ok(()) => eprintln!("removed: {path_str}"),
                Err(e) => eprintln!("delete failed for {path_str}: {e}"),
            }
        }
        for p in pending_changed.drain() {
            if let Err(e) = index::reindex_file(&store, &embedder, &cfg, &p).await {
                eprintln!("reindex failed for {}: {}", p.display(), e);
            }
        }
    }

    Ok(())
}

fn absorb(changed: &mut HashSet<PathBuf>, removed: &mut HashSet<PathBuf>, event: notify::Event) {
    let is_remove = matches!(event.kind, EventKind::Remove(_));
    let is_change = matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_));
    if !(is_remove || is_change) {
        return;
    }
    for p in event.paths {
        if !should_index(&p) {
            continue;
        }
        if is_remove {
            // A remove cancels any pending change, and vice versa.
            removed.insert(p.clone());
            changed.remove(&p);
        } else {
            changed.insert(p.clone());
            removed.remove(&p);
        }
    }
}

/// Cheap path filter. For full .gitignore semantics, build an
/// `ignore::gitignore::Gitignore` per root at startup and consult it here.
fn should_index(p: &Path) -> bool {
    // Existence check skips directories and gone-on-arrival temp files.
    if !p.is_file() && !matches!(p.try_exists(), Ok(false)) {
        // Allow paths that don't exist (i.e. removed) through — we still want to delete them.
        // Block paths that exist but aren't files (directories etc).
        if p.exists() {
            return false;
        }
    }
    let s = p.to_string_lossy();
    let ignored_segments = [
        "/.git/",
        "/node_modules/",
        "/target/",
        "/dist/",
        "/build/",
        "/__pycache__/",
        "/.venv/",
        "/.next/",
        "/.cache/",
    ];
    if ignored_segments.iter().any(|seg| s.contains(seg)) {
        return false;
    }
    if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
        // Editor swap files
        if name.ends_with('~') || name.starts_with(".#") || name.starts_with("#") {
            return false;
        }
    }
    !index::is_probably_binary(p)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, DataChange, ModifyKind, RemoveKind};
    use notify::Event;

    fn evt(kind: EventKind, path: &str) -> Event {
        Event::new(kind).add_path(PathBuf::from(path))
    }

    fn create_evt(path: &str) -> Event {
        evt(EventKind::Create(CreateKind::File), path)
    }

    fn modify_evt(path: &str) -> Event {
        evt(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            path,
        )
    }

    fn remove_evt(path: &str) -> Event {
        evt(EventKind::Remove(RemoveKind::File), path)
    }

    // ---- should_index ----
    //
    // Most cases use absolute paths that don't exist on disk; the function's
    // file-existence guard short-circuits to "allow" for missing paths so
    // deletes of already-gone files can still be propagated to the store.
    // That makes it safe to exercise the segment/swap/binary filters without
    // touching the filesystem.

    #[test]
    fn should_index_allows_normal_text_file_under_a_normal_path() {
        let p = PathBuf::from("/definitely/not/real/notes/alpha.md");
        assert!(should_index(&p));
    }

    #[test]
    fn should_index_allows_paths_that_no_longer_exist_so_deletes_can_propagate() {
        // The watcher receives Remove events with paths that are already gone
        // by the time we check them. should_index must let those through, or
        // the store will accumulate stale rows for files the user has deleted.
        let p = PathBuf::from("/tmp/lilctx-nonexistent-deleted-file-xyz.md");
        assert!(!p.exists(), "test precondition: path must not exist");
        assert!(should_index(&p));
    }

    #[test]
    fn should_index_skips_paths_inside_ignored_directories() {
        let cases = [
            "/proj/.git/HEAD",
            "/proj/node_modules/x/index.js",
            "/proj/target/debug/foo.rs",
            "/proj/dist/bundle.js",
            "/proj/build/output.txt",
            "/proj/__pycache__/foo.pyc",
            "/proj/.venv/bin/python",
            "/proj/.next/server.js",
            "/proj/.cache/x.txt",
        ];
        for c in cases {
            let p = PathBuf::from(c);
            assert!(
                !should_index(&p),
                "expected ignored-segment path {c} to be skipped"
            );
        }
    }

    #[test]
    fn should_index_skips_editor_swap_files() {
        // Vim writes `foo~`, Emacs writes `.#foo` and `#foo#`. Reindexing
        // those would index transient editor state, not the user's content.
        let cases = [
            "/notes/foo.md~",
            "/notes/.#foo.md",
            "/notes/#foo.md#",
            "/notes/#foo.md",
        ];
        for c in cases {
            let p = PathBuf::from(c);
            assert!(
                !should_index(&p),
                "expected editor-swap path {c} to be skipped"
            );
        }
    }

    #[test]
    fn should_index_skips_files_with_binary_extensions() {
        let cases = [
            "/notes/x.png",
            "/notes/x.pdf",
            "/notes/x.zip",
            "/notes/x.so",
            "/notes/x.woff2",
            "/notes/x.mp4",
        ];
        for c in cases {
            let p = PathBuf::from(c);
            assert!(
                !should_index(&p),
                "expected binary-extension path {c} to be skipped"
            );
        }
    }

    #[test]
    fn should_index_skips_existing_directories() {
        // The is_file()/exists() guard fires for paths that exist but aren't
        // files. We need a real entry on disk to exercise it.
        let dir = tempfile::tempdir().unwrap();
        assert!(!should_index(dir.path()));
    }

    // ---- absorb ----

    #[test]
    fn absorb_records_create_as_a_change() {
        let mut changed = HashSet::new();
        let mut removed = HashSet::new();
        absorb(&mut changed, &mut removed, create_evt("/notes/a.md"));
        assert_eq!(changed.len(), 1);
        assert!(changed.contains(&PathBuf::from("/notes/a.md")));
        assert!(removed.is_empty());
    }

    #[test]
    fn absorb_records_modify_as_a_change() {
        let mut changed = HashSet::new();
        let mut removed = HashSet::new();
        absorb(&mut changed, &mut removed, modify_evt("/notes/a.md"));
        assert!(changed.contains(&PathBuf::from("/notes/a.md")));
        assert!(removed.is_empty());
    }

    #[test]
    fn absorb_records_remove_as_a_removal() {
        let mut changed = HashSet::new();
        let mut removed = HashSet::new();
        absorb(&mut changed, &mut removed, remove_evt("/notes/a.md"));
        assert!(removed.contains(&PathBuf::from("/notes/a.md")));
        assert!(changed.is_empty());
    }

    #[test]
    fn absorb_ignores_event_kinds_that_do_not_change_content() {
        // Access events fire on reads/opens. They must not trigger reindex —
        // doing so would re-embed every file every time it was opened.
        let mut changed = HashSet::new();
        let mut removed = HashSet::new();
        absorb(
            &mut changed,
            &mut removed,
            evt(EventKind::Access(AccessKind::Read), "/notes/a.md"),
        );
        absorb(&mut changed, &mut removed, evt(EventKind::Any, "/notes/a.md"));
        absorb(
            &mut changed,
            &mut removed,
            evt(EventKind::Other, "/notes/a.md"),
        );
        assert!(changed.is_empty());
        assert!(removed.is_empty());
    }

    #[test]
    fn absorb_remove_after_modify_in_a_burst_results_in_only_a_removal() {
        // Burst order: Modify then Remove on the same path. The path is gone;
        // reindexing it would either fail to read or, worse, race a concurrent
        // recreate. The remove must win and cancel the pending modify.
        let mut changed = HashSet::new();
        let mut removed = HashSet::new();
        let path = "/notes/a.md";
        absorb(&mut changed, &mut removed, modify_evt(path));
        absorb(&mut changed, &mut removed, remove_evt(path));
        assert!(
            changed.is_empty(),
            "later remove must cancel pending modify"
        );
        assert!(removed.contains(&PathBuf::from(path)));
    }

    #[test]
    fn absorb_create_after_remove_in_a_burst_results_in_only_a_change() {
        // Editors that "save by remove + recreate" emit Remove then Create on
        // the same path. The file ends up present with new content; the net
        // effect must be reindex, not delete.
        let mut changed = HashSet::new();
        let mut removed = HashSet::new();
        let path = "/notes/a.md";
        absorb(&mut changed, &mut removed, remove_evt(path));
        absorb(&mut changed, &mut removed, create_evt(path));
        assert!(
            removed.is_empty(),
            "later create must cancel pending remove"
        );
        assert!(changed.contains(&PathBuf::from(path)));
    }

    #[test]
    fn absorb_drops_paths_that_should_not_be_indexed() {
        // Saving a file inside node_modules generates a Modify event; the
        // policy is "we don't watch dependency directories". The pending sets
        // must stay empty.
        let mut changed = HashSet::new();
        let mut removed = HashSet::new();
        absorb(
            &mut changed,
            &mut removed,
            modify_evt("/proj/node_modules/x.js"),
        );
        absorb(
            &mut changed,
            &mut removed,
            remove_evt("/proj/.git/refs/heads/main"),
        );
        assert!(changed.is_empty());
        assert!(removed.is_empty());
    }

    #[test]
    fn absorb_processes_each_path_in_a_multi_path_event_independently() {
        // Some backends batch paths into a single Event (notably renames).
        // Each path must be classified on its own; one ignored path must not
        // affect the others.
        let mut changed = HashSet::new();
        let mut removed = HashSet::new();
        let event = Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Content)))
            .add_path(PathBuf::from("/notes/a.md"))
            .add_path(PathBuf::from("/proj/node_modules/x.js"))
            .add_path(PathBuf::from("/notes/b.md"));
        absorb(&mut changed, &mut removed, event);
        assert_eq!(changed.len(), 2);
        assert!(changed.contains(&PathBuf::from("/notes/a.md")));
        assert!(changed.contains(&PathBuf::from("/notes/b.md")));
    }

    #[test]
    fn absorb_deduplicates_repeated_events_for_the_same_path() {
        // A single vim save can fire 5–20 modify events for the same file.
        // The pending set is the union, not a count — repeated events must
        // not blow up memory or trigger N reindexes.
        let mut changed = HashSet::new();
        let mut removed = HashSet::new();
        for _ in 0..10 {
            absorb(&mut changed, &mut removed, modify_evt("/notes/a.md"));
        }
        assert_eq!(changed.len(), 1);
    }
}
