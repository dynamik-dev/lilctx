//! Shared scaffolding for integration tests.
//!
//! Each test gets its own `TestEnv`: a temp config, temp data dir, temp source
//! tree, and a per-test fake embedding server bound to a random port. Drop
//! cleans everything up.

// Test scaffolding. `.unwrap()`/`.expect()` are idiomatic for setup that
// must succeed for the test to be meaningful — failing setup *is* a test
// failure. Mirrors the precedent set in `src/config.rs` test module.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use wiremock::{
    matchers::{method, path as wm_path},
    Mock, MockServer, Request, Respond, ResponseTemplate,
};

/// Test embedding dim. Small on purpose — keeps fake vectors readable and
/// LanceDB index build cheap. Must match what the test config writes.
pub(crate) const TEST_DIM: usize = 8;

/// Vocabulary used to project text into vectors. Each position in the vector
/// counts occurrences of one keyword. A document containing "alpha" has a
/// nonzero value at position 0; a query for "alpha" produces the same.
/// That makes vector search recover the document — which is the contract we
/// want to exercise (plumbing, not embedding quality).
pub(crate) const VOCAB: [&str; TEST_DIM] = [
    "alpha", "beta", "gamma", "delta", "echo", "foxtrot", "golf", "hotel",
];

/// Out-of-band keyword the rest of the corpus never contains, so search hits
/// for it identify a single seeded file unambiguously.
pub(crate) const UNIQUE_MARKER: &str = "alpha";

#[derive(Deserialize)]
struct EmbedReqBody {
    #[allow(dead_code)]
    model: String,
    input: Vec<String>,
}

#[derive(Serialize)]
struct EmbedRespItem {
    embedding: Vec<f32>,
    index: usize,
    object: &'static str,
}

#[derive(Serialize)]
struct EmbedRespBody {
    data: Vec<EmbedRespItem>,
    object: &'static str,
    model: String,
}

struct VocabResponder;

impl Respond for VocabResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: EmbedReqBody = serde_json::from_slice(&req.body)
            .expect("test fake server: malformed embeddings request body");
        let data = body
            .input
            .iter()
            .enumerate()
            .map(|(i, text)| EmbedRespItem {
                embedding: text_to_vec(text),
                index: i,
                object: "embedding",
            })
            .collect();
        let resp = EmbedRespBody {
            data,
            object: "list",
            model: body.model,
        };
        ResponseTemplate::new(200).set_body_json(resp)
    }
}

fn text_to_vec(text: &str) -> Vec<f32> {
    let lower = text.to_lowercase();
    let mut v = vec![0.0f32; TEST_DIM];
    for (i, &word) in VOCAB.iter().enumerate() {
        let count = lower.matches(word).count() as f32;
        v[i] = count;
    }
    // Avoid an all-zero vector — LanceDB's distance handling on zero norms
    // is undefined territory for tests. The shared baseline keeps every
    // chunk reachable without breaking discrimination.
    if v.iter().all(|x| *x == 0.0) {
        v[TEST_DIM - 1] = 0.001;
    }
    v
}

pub(crate) struct TestEnv {
    /// Where source files for the test live; configured as the only `paths` root.
    pub(crate) source_dir: TempDir,
    /// LanceDB data dir for the test.
    pub(crate) data_dir: TempDir,
    /// Holds the generated config.toml.
    pub(crate) config_dir: TempDir,
    /// Local fake of OpenRouter / OpenAI embeddings.
    pub(crate) mock_server: MockServer,
    /// Path of the generated config file.
    pub(crate) config_path: PathBuf,
}

impl TestEnv {
    pub(crate) async fn new() -> Self {
        let source_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();
        let config_dir = TempDir::new().unwrap();
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(wm_path("/v1/embeddings"))
            .respond_with(VocabResponder)
            .mount(&mock_server)
            .await;

        let config_path = config_dir.path().join("config.toml");
        let config = format!(
            r#"
paths = ["{source}"]
data_dir = "{data}"

[chunk]
size = 1500
overlap = 200

[embedding]
dim = {dim}
model = "test-fake/embed"
api_key_env = "LILCTX_TEST_API_KEY"
batch_size = 4
base_url = "{base}/v1"
"#,
            source = source_dir.path().display(),
            data = data_dir.path().display(),
            dim = TEST_DIM,
            base = mock_server.uri(),
        );
        std::fs::write(&config_path, config).unwrap();

        Self {
            source_dir,
            data_dir,
            config_dir,
            mock_server,
            config_path,
        }
    }

    /// Drop a file with given content into the test source tree.
    pub(crate) fn write_source(&self, rel: &str, content: &str) -> PathBuf {
        let abs = self.source_dir.path().join(rel);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&abs, content).unwrap();
        abs
    }

    /// Build an `assert_cmd::Command` for the lilctx binary with the test
    /// config and a per-invocation API key in env.
    pub(crate) fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("lilctx").unwrap();
        c.arg("--config")
            .arg(&self.config_path)
            .env("LILCTX_TEST_API_KEY", "test-key-not-validated")
            // Don't inherit the user's real API key or other lilctx vars.
            .env_remove("OPENROUTER_API_KEY");
        c
    }

    /// Spawn `lilctx serve`. stdin/stdout are piped so the test can speak
    /// JSON-RPC; stderr is piped so panics or log lines don't pollute the
    /// runner's terminal. `kill_on_drop` ensures we don't leak processes
    /// if a test fails partway through.
    pub(crate) fn spawn_serve(&self) -> tokio::process::Child {
        let bin = assert_cmd::cargo::cargo_bin("lilctx");
        tokio::process::Command::new(bin)
            .arg("--config")
            .arg(&self.config_path)
            .arg("serve")
            .env("LILCTX_TEST_API_KEY", "test-key-not-validated")
            .env_remove("OPENROUTER_API_KEY")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn lilctx serve")
    }

    pub(crate) fn data_dir_path(&self) -> &Path {
        self.data_dir.path()
    }
}
