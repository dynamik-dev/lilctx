//! File chunking. Markdown splits on headings; everything else uses a
//! line-aware sliding window with overlap.

use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Debug, Clone)]
pub(crate) struct Chunk {
    pub path: String,
    pub chunk_index: u32,
    pub content: String,
    pub content_hash: String,
    /// Hash of the entire source file. Same value across all chunks of one file.
    /// Used for fast "did this file change since last index" checks.
    pub file_hash: String,
}

pub(crate) fn chunk_file(
    path: &Path,
    content: &str,
    file_hash: &str,
    size: usize,
    overlap: usize,
) -> Vec<Chunk> {
    let path_str = path.to_string_lossy().to_string();
    let raw = match path.extension().and_then(|e| e.to_str()) {
        Some("md") | Some("markdown") => chunk_markdown(content, size, overlap),
        _ => chunk_lines(content, size, overlap),
    };
    raw.into_iter()
        .enumerate()
        .map(|(i, content)| Chunk {
            path: path_str.clone(),
            chunk_index: i as u32,
            content_hash: sha256_hex(&content),
            file_hash: file_hash.to_string(),
            content,
        })
        .collect()
}

pub(crate) fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    hex::encode(h.finalize())
}

fn chunk_markdown(content: &str, size: usize, overlap: usize) -> Vec<String> {
    let mut sections: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in content.lines() {
        let is_heading =
            line.starts_with("# ") || line.starts_with("## ") || line.starts_with("### ");
        if is_heading && !current.is_empty() {
            sections.push(std::mem::take(&mut current));
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.is_empty() {
        sections.push(current);
    }
    sections
        .into_iter()
        .flat_map(|s| {
            if s.len() <= size * 2 {
                vec![s]
            } else {
                chunk_lines(&s, size, overlap)
            }
        })
        .collect()
}

fn chunk_lines(content: &str, size: usize, overlap: usize) -> Vec<String> {
    let lines: Vec<&str> = content.lines().collect();
    let mut chunks = Vec::new();
    let mut buf = String::new();
    let mut window_start = 0usize;
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        if !buf.is_empty() && buf.len() + line.len() + 1 > size {
            chunks.push(std::mem::take(&mut buf));
            let mut back = i;
            let mut back_chars = 0usize;
            while back > window_start && back_chars < overlap {
                back -= 1;
                back_chars += lines[back].len() + 1;
            }
            window_start = back;
            i = back;
            continue;
        }
        buf.push_str(line);
        buf.push('\n');
        i += 1;
    }
    if !buf.is_empty() {
        chunks.push(buf);
    }
    chunks
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    // ---- sha256_hex ----

    #[test]
    fn sha256_hex_matches_known_vector_for_abc() {
        // Lock the format: lowercase hex, no prefix. file_hash dedup
        // (CLAUDE.md hard rule) depends on a stable string representation.
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    // ---- chunk_file: invariants required by the indexing pipeline ----

    #[test]
    fn chunk_file_empty_content_returns_no_chunks() {
        let chunks = chunk_file(&p("notes.md"), "", "deadbeef", 1024, 64);
        assert!(chunks.is_empty());
    }

    #[test]
    fn chunk_file_propagates_file_hash_to_every_chunk() {
        // Hard rule from CLAUDE.md: every Chunk must carry file_hash and the
        // value must be identical across all chunks of one file. Without this,
        // Store::file_already_indexed cannot short-circuit unchanged files.
        let content = "# A\nalpha\n# B\nbeta\n# C\ngamma\n";
        let chunks = chunk_file(&p("notes.md"), content, "file-hash-xyz", 1024, 0);
        assert!(chunks.len() > 1, "test setup expects multiple chunks");
        for c in &chunks {
            assert_eq!(c.file_hash, "file-hash-xyz");
        }
    }

    #[test]
    fn chunk_file_assigns_sequential_chunk_indexes_starting_at_zero() {
        let content = "# A\nalpha\n# B\nbeta\n# C\ngamma\n";
        let chunks = chunk_file(&p("doc.md"), content, "h", 1024, 0);
        assert!(chunks.len() >= 2);
        for (i, c) in chunks.iter().enumerate() {
            assert_eq!(c.chunk_index as usize, i);
        }
    }

    #[test]
    fn chunk_file_records_path_string_for_each_chunk() {
        let chunks = chunk_file(&p("/tmp/sub/data.txt"), "alpha\nbeta\n", "h", 1024, 0);
        assert!(!chunks.is_empty());
        for c in &chunks {
            assert_eq!(c.path, "/tmp/sub/data.txt");
        }
    }

    #[test]
    fn chunk_file_content_hash_is_sha256_of_chunk_content() {
        let content = "# A\nalpha\n# B\nbeta\n";
        let chunks = chunk_file(&p("doc.md"), content, "h", 1024, 0);
        assert!(chunks.len() >= 2);
        for c in &chunks {
            assert_eq!(c.content_hash, sha256_hex(&c.content));
        }
    }

    // ---- chunk_file: extension dispatch ----

    #[test]
    fn chunk_file_md_extension_splits_on_headings() {
        let content = "# A\nalpha\n# B\nbeta\n";
        // size large enough that no further sub-chunking happens, so the only
        // split we see is the heading-driven one.
        let chunks = chunk_file(&p("doc.md"), content, "h", 1024, 0);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].content.starts_with("# A"));
        assert!(chunks[1].content.starts_with("# B"));
    }

    #[test]
    fn chunk_file_markdown_alias_extension_also_uses_markdown_chunker() {
        let content = "# A\nalpha\n# B\nbeta\n";
        let chunks = chunk_file(&p("doc.markdown"), content, "h", 1024, 0);
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn chunk_file_non_markdown_extension_does_not_split_on_headings() {
        // Same content as the markdown test, but a .txt extension forces the
        // line chunker. With a large size the whole thing must collapse to one
        // chunk regardless of the `# ` lines inside.
        let content = "# A\nalpha\n# B\nbeta\n";
        let chunks = chunk_file(&p("doc.txt"), content, "h", 1024, 0);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("# A"));
        assert!(chunks[0].content.contains("# B"));
    }

    #[test]
    fn chunk_file_extensionless_path_uses_line_chunker() {
        let content = "# A\nalpha\n# B\nbeta\n";
        let chunks = chunk_file(&p("Makefile"), content, "h", 1024, 0);
        assert_eq!(chunks.len(), 1);
    }

    // ---- markdown chunking ----

    #[test]
    fn markdown_splits_on_each_of_h1_h2_h3() {
        let content = "# H1\na\n## H2\nb\n### H3\nc\n";
        let chunks = chunk_file(&p("doc.md"), content, "h", 1024, 0);
        assert_eq!(chunks.len(), 3);
        assert!(chunks[0].content.starts_with("# H1"));
        assert!(chunks[1].content.starts_with("## H2"));
        assert!(chunks[2].content.starts_with("### H3"));
    }

    #[test]
    fn markdown_does_not_split_on_h4_or_deeper() {
        // chunk_markdown only treats `# `, `## `, `### ` as section breaks.
        // An h4 should stay inside the surrounding section.
        let content = "# Top\nintro\n#### Deep\nstuff\n";
        let chunks = chunk_file(&p("doc.md"), content, "h", 1024, 0);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("#### Deep"));
    }

    #[test]
    fn markdown_subchunks_oversized_section_via_line_chunker() {
        // A section longer than size*2 must be fed through the line chunker
        // so embedding inputs stay bounded even when an author writes one
        // giant heading-less wall.
        let body = "line\n".repeat(30);
        let content = format!("# Big\n{body}");
        let chunks = chunk_file(&p("doc.md"), &content, "h", 20, 5);
        assert!(
            chunks.len() > 1,
            "expected oversized section to be sub-chunked, got {} chunk(s)",
            chunks.len()
        );
    }

    // ---- line chunker (exercised via .txt) ----

    #[test]
    fn line_chunker_emits_single_chunk_when_content_fits_in_size() {
        let chunks = chunk_file(&p("notes.txt"), "abc\ndef\n", "h", 1024, 0);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, "abc\ndef\n");
    }

    #[test]
    fn line_chunker_splits_when_buffer_would_exceed_size() {
        // Five identical 8-char lines (9 bytes each with newline). With size=20
        // at most ~2 lines fit per chunk, so we must see multiple chunks and
        // every original line must appear at least once across the output.
        let line = "aaaaaaaa";
        let content = format!("{l}\n{l}\n{l}\n{l}\n{l}\n", l = line);
        let chunks = chunk_file(&p("notes.txt"), &content, "h", 20, 0);
        assert!(
            chunks.len() >= 2,
            "expected multiple chunks, got {}",
            chunks.len()
        );
        let occurrences: usize = chunks.iter().map(|c| c.content.matches(line).count()).sum();
        assert!(occurrences >= 5);
    }

    #[test]
    fn line_chunker_overlap_repeats_tail_lines_at_next_chunk_start() {
        // Overlap exists so that semantic context isn't severed at a hard
        // boundary. The last line of chunk N should reappear at the start of
        // chunk N+1.
        let content = "line-1\nline-2\nline-3\nline-4\nline-5\nline-6\n";
        let chunks = chunk_file(&p("notes.txt"), content, "h", 20, 5);
        assert!(chunks.len() >= 2);
        let prev_last = chunks[0]
            .content
            .lines()
            .last()
            .expect("first chunk has at least one line");
        assert!(
            chunks[1].content.starts_with(prev_last),
            "expected chunk[1] to begin with overlapping line `{prev_last}`, got `{}`",
            chunks[1].content
        );
    }
}
