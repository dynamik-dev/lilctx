//! Index orchestration: walk → file_hash check → chunk → batch-embed → upsert.
//!
//! Both `run` (bulk) and `reindex_file` (single-file, used by watch) live here so
//! the dedup logic stays in one place.

use anyhow::Result;
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

use crate::{
    chunk::{self, sha256_hex},
    config::Config,
    embed::OpenRouterEmbedder,
    store::Store,
};

pub(crate) async fn run(cfg: &Config, paths: &[PathBuf], force: bool) -> Result<()> {
    let store = Store::open(cfg).await?;
    let embedder = OpenRouterEmbedder::new(cfg)?;

    let files = collect_files(paths);
    eprintln!(
        "walking {} root(s) → {} candidate files",
        paths.len(),
        files.len()
    );

    let mut buffer: Vec<chunk::Chunk> = Vec::new();
    let mut total = 0usize;
    let mut skipped = 0usize;

    for file in &files {
        let Ok(content) = std::fs::read_to_string(file) else {
            continue;
        };
        let file_hash = sha256_hex(&content);
        let path_str = file.to_string_lossy().to_string();

        if !force && store.file_already_indexed(&path_str, &file_hash).await? {
            skipped += 1;
            continue;
        }

        let chunks = chunk::chunk_file(
            file,
            &content,
            &file_hash,
            cfg.chunk.size,
            cfg.chunk.overlap,
        );
        if chunks.is_empty() {
            continue;
        }

        // Replace strategy: drop any old chunks for this path before staging new ones.
        // If the process dies mid-batch, the file_hash check on next run will reindex.
        store.delete_path(&path_str).await?;
        buffer.extend(chunks);

        while buffer.len() >= embedder.batch_size {
            let batch: Vec<chunk::Chunk> = buffer.drain(..embedder.batch_size).collect();
            total += flush(&store, &embedder, batch).await?;
        }
    }
    if !buffer.is_empty() {
        total += flush(&store, &embedder, std::mem::take(&mut buffer)).await?;
    }

    eprintln!("indexed {total} new chunks (skipped {skipped} unchanged files)");
    Ok(())
}

/// Reindex a single file. Used by the watcher.
/// Reads, hashes, skips if unchanged, otherwise replaces all chunks for the path.
pub(crate) async fn reindex_file(
    store: &Store,
    embedder: &OpenRouterEmbedder,
    cfg: &Config,
    file: &Path,
) -> Result<()> {
    let Ok(content) = std::fs::read_to_string(file) else {
        return Ok(());
    };
    let file_hash = sha256_hex(&content);
    let path_str = file.to_string_lossy().to_string();

    if store.file_already_indexed(&path_str, &file_hash).await? {
        return Ok(());
    }

    let chunks = chunk::chunk_file(
        file,
        &content,
        &file_hash,
        cfg.chunk.size,
        cfg.chunk.overlap,
    );
    if chunks.is_empty() {
        return Ok(());
    }

    let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
    let mut all_vecs = Vec::with_capacity(chunks.len());
    for batch in texts.chunks(embedder.batch_size) {
        let vecs = embedder.embed_batch(batch).await?;
        all_vecs.extend(vecs);
    }

    store.delete_path(&path_str).await?;
    store.upsert(chunks, all_vecs).await?;
    eprintln!("reindexed: {path_str}");
    Ok(())
}

async fn flush(
    store: &Store,
    embedder: &OpenRouterEmbedder,
    batch: Vec<chunk::Chunk>,
) -> Result<usize> {
    let texts: Vec<String> = batch.iter().map(|c| c.content.clone()).collect();
    let vecs = embedder.embed_batch(&texts).await?;
    let n = batch.len();
    store.upsert(batch, vecs).await?;
    Ok(n)
}

fn collect_files(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in roots {
        let mut wb = WalkBuilder::new(root);
        wb.standard_filters(true).hidden(false);
        for entry in wb.build().flatten() {
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                let p = entry.into_path();
                if !is_probably_binary(&p) {
                    out.push(p);
                }
            }
        }
    }
    out
}

pub(crate) fn is_probably_binary(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()),
        Some(
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "webp"
                | "ico"
                | "pdf"
                | "zip"
                | "tar"
                | "gz"
                | "br"
                | "xz"
                | "7z"
                | "exe"
                | "dll"
                | "so"
                | "dylib"
                | "bin"
                | "wasm"
                | "ttf"
                | "otf"
                | "woff"
                | "woff2"
                | "mp3"
                | "mp4"
                | "mov"
                | "avi"
                | "mkv"
        )
    )
}
