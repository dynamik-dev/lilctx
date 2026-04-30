//! Feature tests at the CLI boundary. We invoke the real `lilctx` binary,
//! point it at a per-test config, and assert on stdout/stderr/exit/files —
//! the contract the user sees.
//!
//! What's mocked: the OpenRouter embeddings endpoint (we don't control it).
//! What's real: LanceDB, the file walker, the chunker, the binary itself.

// Setup `.unwrap()`/`.expect()` are idiomatic in tests; mirrors the precedent
// set on `src/config.rs`'s `#[cfg(test)] mod tests`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use predicates::prelude::*;

use crate::common::{TestEnv, UNIQUE_MARKER};

#[tokio::test]
async fn init_writes_a_starter_config_and_refuses_to_overwrite_it() {
    // The boundary is "user runs `lilctx init --path X`": the file gets
    // written, and a second invocation refuses to clobber it. We don't try
    // to *load* the config back because the starter points at default user
    // dirs (~/.local/share/...) — re-running init is the cleanest "this
    // file is real and well-formed" probe at the CLI boundary.
    let env = TestEnv::new().await;
    let target = env.config_dir.path().join("fresh.toml");

    // First: write the starter.
    assert_cmd::Command::cargo_bin("lilctx")
        .unwrap()
        .arg("init")
        .arg("--path")
        .arg(&target)
        .assert()
        .success();

    assert!(target.exists(), "init should have written {target:?}");
    let written = std::fs::read_to_string(&target).unwrap();
    assert!(
        written.contains("[embedding]") && written.contains("[chunk]"),
        "written config should contain the standard sections; got:\n{written}"
    );

    // Second: refuses to overwrite.
    assert_cmd::Command::cargo_bin("lilctx")
        .unwrap()
        .arg("init")
        .arg("--path")
        .arg(&target)
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
}

#[tokio::test]
async fn index_then_search_returns_content_from_indexed_files() {
    let env = TestEnv::new().await;

    // Two files with disjoint vocabulary so search can discriminate.
    env.write_source(
        "notes/alpha.md",
        "# Alpha\nThis note talks about alpha topics in detail.\n",
    );
    env.write_source(
        "notes/golf.md",
        "# Golf\nThis note talks about golf topics in detail.\n",
    );

    env.cmd().arg("index").assert().success();

    // Search for the marker that only appears in alpha.md. The CLI prints
    // the path of each hit; we assert the right file appears in output.
    env.cmd()
        .args(["search", UNIQUE_MARKER, "-k", "5"])
        .assert()
        .success()
        .stdout(predicate::str::contains("alpha.md"));
}

#[tokio::test]
async fn reindex_skips_files_whose_content_has_not_changed() {
    let env = TestEnv::new().await;
    env.write_source("a.md", "# A\nstable alpha content\n");
    env.write_source("b.md", "# B\nstable beta content\n");

    // First index: nothing is skipped (everything is new).
    env.cmd()
        .arg("index")
        .assert()
        .success()
        .stderr(predicate::str::contains("skipped 0 unchanged files"));

    // Second index: every file's hash matches what's in the store; all skipped.
    env.cmd()
        .arg("index")
        .assert()
        .success()
        .stderr(predicate::str::contains("skipped 2 unchanged files"));
}

#[tokio::test]
async fn modifying_a_file_replaces_its_chunks_without_duplicating() {
    let env = TestEnv::new().await;
    let path = env.write_source("a.md", "# A\nfirst alpha version\n");

    env.cmd().arg("index").assert().success();

    // Capture chunk count after first index. This is the contract:
    // re-indexing must never create duplicate (path, chunk_index) rows.
    let initial_status = env.cmd().arg("status").output().expect("running status");
    let initial = String::from_utf8(initial_status.stdout).unwrap();
    let count_re = regex_count(&initial);

    // Modify the file. The chunker output may differ, but the file is small
    // enough to still be a single chunk. Either way, total chunks for this
    // path must reflect the new content, not stack on top of the old.
    std::fs::write(
        &path,
        "# A\nsecond alpha version, slightly longer body text\n",
    )
    .unwrap();

    env.cmd().arg("index").assert().success();

    let after_status = env.cmd().arg("status").output().expect("running status");
    let after = String::from_utf8(after_status.stdout).unwrap();
    let after_count = regex_count(&after);

    // The single-file modification should replace, not append. Chunk count
    // must equal what we'd get from indexing just the new content fresh
    // (which is the same as the first index, since the file shape is similar).
    assert_eq!(
        after_count, count_re,
        "expected chunk count to stay equal after replacing one file's content; \
         got {count_re} → {after_count}. Status output:\nbefore: {initial}\nafter: {after}"
    );

    // And searching for content unique to the NEW file should still resolve.
    env.cmd()
        .args(["search", "alpha"])
        .assert()
        .success()
        .stdout(predicate::str::contains("a.md"));
}

#[tokio::test]
async fn status_reports_the_indexed_chunk_count() {
    let env = TestEnv::new().await;
    env.write_source("one.md", "# One\nalpha\n");
    env.write_source("two.md", "# Two\nbeta\n");

    env.cmd().arg("index").assert().success();

    env.cmd().arg("status").assert().success().stdout(
        predicate::str::contains("indexed chunks:")
            .and(predicate::str::contains("data dir:"))
            .and(predicate::function(|s: &str| {
                extract_chunk_count(s).is_some_and(|n| n >= 2)
            })),
    );
}

#[tokio::test]
async fn index_with_no_paths_in_config_and_no_args_fails_clearly() {
    // Reach into the test config and blank out `paths`. We're asserting the
    // user-visible error, not that `paths.is_empty()` triggers some internal
    // branch — exit nonzero with a useful message is the contract.
    let env = TestEnv::new().await;
    let raw = std::fs::read_to_string(&env.config_path).unwrap();
    let cleared = raw.replacen(
        &format!("paths = [\"{}\"]", env.source_dir.path().display()),
        "paths = []",
        1,
    );
    std::fs::write(&env.config_path, cleared).unwrap();

    env.cmd()
        .arg("index")
        .assert()
        .failure()
        .stderr(predicate::str::contains("no paths to index"));
}

fn extract_chunk_count(s: &str) -> Option<usize> {
    s.lines()
        .find_map(|l| l.strip_prefix("indexed chunks:"))
        .and_then(|rest| rest.trim().parse().ok())
}

fn regex_count(status_stdout: &str) -> usize {
    let parsed = extract_chunk_count(status_stdout);
    assert!(
        parsed.is_some(),
        "could not find `indexed chunks: N` in status output:\n{status_stdout}"
    );
    parsed.unwrap()
}
