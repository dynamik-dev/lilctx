//! Project configuration loaded from a TOML file.
//!
//! The on-disk schema is the public API of this module. Bumping a field rename
//! or removal is a breaking change for any user who has run `init`.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Config {
    /// Roots to walk during `index` and `watch`.
    pub paths: Vec<PathBuf>,
    /// LanceDB data directory.
    pub data_dir: PathBuf,
    pub chunk: ChunkConfig,
    pub embedding: EmbeddingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChunkConfig {
    pub size: usize,
    pub overlap: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct EmbeddingConfig {
    pub dim: usize,
    pub model: String,
    pub api_key_env: String,
    pub batch_size: usize,
    /// OpenAI-compatible `/v1` base URL (no trailing `/embeddings`).
    /// Defaults to OpenRouter so older configs without this field still load.
    #[serde(default = "default_base_url")]
    pub base_url: String,
}

fn default_base_url() -> String {
    "https://openrouter.ai/api/v1".to_string()
}

const STARTER_CONFIG: &str = r#"# lilctx configuration. `~` is expanded at load time.
#
# Runtime env overrides (set any of these to skip editing this file):
#   LILCTX_OPENROUTER_API_KEY   -> OpenRouter mode; sets base_url and api_key_env for you
#   LILCTX_OPENAI_API_KEY       -> custom OpenAI-compatible mode
#   LILCTX_OPENAI_BASE_URL      -> base URL for the OpenAI-compatible mode (default: https://api.openai.com/v1)
#   LILCTX_EMBEDDING_MODEL      -> override `model` below
#   LILCTX_EMBEDDING_DIM        -> override `dim` below (requires wiping data_dir if it changes)

# Roots to walk during `index` and `watch`.
paths = ["~/notes"]

# LanceDB data directory. Created on first run.
data_dir = "~/.local/share/lilctx"

[chunk]
# Soft target chunk size in bytes (line-aware, so chunks may overshoot).
size = 1500
# Bytes of overlap between successive chunks.
overlap = 200

[embedding]
# Vector dimension. MUST match the model -- changing it requires deleting
# `data_dir` (LanceDB schema is fixed at table-creation time).
dim = 768
# Model name as accepted by the embeddings endpoint.
model = "baai/bge-base-en-v1.5"
# Env var that holds the API key. Read at runtime, never persisted.
api_key_env = "LILCTX_OPENROUTER_API_KEY"
# Number of texts to embed per HTTP request.
batch_size = 32
# OpenAI-compatible /v1 base URL. Drop the trailing /embeddings.
base_url = "https://openrouter.ai/api/v1"
"#;

/// Loads `~/.lilctx.json` (if it exists) into the process environment.
///
/// The file is a flat JSON object mapping env var names to string values, e.g.
/// `{"LILCTX_OPENROUTER_API_KEY": "sk-or-..."}`. A variable already set in the
/// environment is left alone, so shell exports always win and this file is a
/// fallback, not an override.
///
/// Call from the very top of `main` before any other env-reading code runs.
pub(crate) fn load_secrets_into_env() -> Result<()> {
    let Some(home) = dirs::home_dir() else {
        return Ok(());
    };
    let path = home.join(".lilctx.json");
    if !path.exists() {
        return Ok(());
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let map: std::collections::HashMap<String, String> =
        serde_json::from_str(&raw).with_context(|| {
            format!(
                "{} must be a flat JSON object of string -> string",
                path.display()
            )
        })?;
    for (k, v) in map {
        if std::env::var_os(&k).is_none() {
            std::env::set_var(&k, v);
        }
    }
    Ok(())
}

pub(crate) fn default_config_path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .ok_or_else(|| anyhow!("could not resolve config dir; pass --config explicitly"))?
        .join("lilctx");
    Ok(dir.join("config.toml"))
}

pub(crate) fn init(path: Option<PathBuf>) -> Result<()> {
    let target = match path {
        Some(p) => p,
        None => default_config_path()?,
    };
    if target.exists() {
        bail!(
            "config already exists at {} -- delete it first to regenerate",
            target.display()
        );
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&target, STARTER_CONFIG)
        .with_context(|| format!("writing {}", target.display()))?;
    eprintln!("wrote {}", target.display());
    Ok(())
}

pub(crate) fn load(path: Option<&Path>) -> Result<Config> {
    let resolved: PathBuf = match path {
        Some(p) => p.to_path_buf(),
        None => default_config_path()?,
    };
    let raw = std::fs::read_to_string(&resolved).with_context(|| {
        format!(
            "reading config at {} -- run `lilctx init` first",
            resolved.display()
        )
    })?;
    let mut cfg: Config =
        toml::from_str(&raw).with_context(|| format!("parsing {}", resolved.display()))?;
    cfg.paths = cfg.paths.into_iter().map(expand_tilde).collect();
    cfg.data_dir = expand_tilde(cfg.data_dir);
    apply_env_overrides(&mut cfg, &env_getter)?;
    Ok(cfg)
}

fn env_getter(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
const ENV_OPENROUTER_KEY: &str = "LILCTX_OPENROUTER_API_KEY";
const ENV_OPENAI_KEY: &str = "LILCTX_OPENAI_API_KEY";
const ENV_OPENAI_BASE_URL: &str = "LILCTX_OPENAI_BASE_URL";
const ENV_EMBEDDING_MODEL: &str = "LILCTX_EMBEDDING_MODEL";
const ENV_EMBEDDING_DIM: &str = "LILCTX_EMBEDDING_DIM";

fn apply_env_overrides(cfg: &mut Config, get: &dyn Fn(&str) -> Option<String>) -> Result<()> {
    let openrouter = get(ENV_OPENROUTER_KEY).is_some();
    let openai = get(ENV_OPENAI_KEY).is_some();
    if openrouter && openai {
        bail!(
            "both {ENV_OPENROUTER_KEY} and {ENV_OPENAI_KEY} are set -- pick one; \
             unset whichever you don't want lilctx to use"
        );
    }
    if openrouter {
        cfg.embedding.base_url = OPENROUTER_BASE_URL.to_string();
        cfg.embedding.api_key_env = ENV_OPENROUTER_KEY.to_string();
    } else if openai {
        cfg.embedding.api_key_env = ENV_OPENAI_KEY.to_string();
        cfg.embedding.base_url = get(ENV_OPENAI_BASE_URL).unwrap_or_else(|| OPENAI_BASE_URL.to_string());
    }
    if let Some(model) = get(ENV_EMBEDDING_MODEL) {
        cfg.embedding.model = model;
    }
    if let Some(dim) = get(ENV_EMBEDDING_DIM) {
        cfg.embedding.dim = dim.parse().with_context(|| {
            format!("{ENV_EMBEDDING_DIM} must be a positive integer, got `{dim}`")
        })?;
    }
    Ok(())
}

fn expand_tilde(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if s == "~" {
        return dirs::home_dir().unwrap_or(p);
    }
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    p
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_root() {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        assert_eq!(expand_tilde(PathBuf::from("~")), home);
    }

    #[test]
    fn expand_tilde_subpath() {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        assert_eq!(
            expand_tilde(PathBuf::from("~/foo/bar")),
            home.join("foo/bar")
        );
    }

    #[test]
    fn expand_tilde_passthrough() {
        let p = PathBuf::from("/abs/path");
        assert_eq!(expand_tilde(p.clone()), p);
        let p = PathBuf::from("relative/path");
        assert_eq!(expand_tilde(p.clone()), p);
        // Tilde mid-path is not a shell expansion — leave it alone.
        let p = PathBuf::from("/foo/~bar");
        assert_eq!(expand_tilde(p.clone()), p);
    }

    #[test]
    fn starter_config_parses() {
        let cfg: Config = toml::from_str(STARTER_CONFIG).expect("starter parses");
        assert!(!cfg.paths.is_empty());
        assert_eq!(cfg.embedding.dim, 768);
        assert_eq!(cfg.embedding.model, "baai/bge-base-en-v1.5");
        assert_eq!(cfg.embedding.api_key_env, ENV_OPENROUTER_KEY);
        assert!(cfg.embedding.base_url.starts_with("https://"));
    }

    // ---- apply_env_overrides ----

    use std::collections::HashMap;

    fn parse_starter() -> Config {
        toml::from_str(STARTER_CONFIG).expect("starter parses")
    }

    fn getter<'a>(
        map: &'a HashMap<&'static str, &'static str>,
    ) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| map.get(name).map(|v| (*v).to_string())
    }

    #[test]
    fn env_overrides_no_vars_set_leaves_config_alone() {
        let mut cfg = parse_starter();
        let before = cfg.embedding.clone();
        let map: HashMap<&str, &str> = HashMap::new();
        apply_env_overrides(&mut cfg, &getter(&map)).expect("no overrides should not error");
        assert_eq!(cfg.embedding.api_key_env, before.api_key_env);
        assert_eq!(cfg.embedding.base_url, before.base_url);
        assert_eq!(cfg.embedding.model, before.model);
        assert_eq!(cfg.embedding.dim, before.dim);
    }

    #[test]
    fn env_overrides_openrouter_key_forces_openrouter_base_url() {
        let mut cfg = parse_starter();
        // Pretend the user wiped api_key_env / base_url to confirm we set them.
        cfg.embedding.api_key_env = "SOMETHING_ELSE".to_string();
        cfg.embedding.base_url = "https://elsewhere.example/v1".to_string();
        let mut map = HashMap::new();
        map.insert(ENV_OPENROUTER_KEY, "sk-or-...");
        apply_env_overrides(&mut cfg, &getter(&map)).expect("override should succeed");
        assert_eq!(cfg.embedding.api_key_env, ENV_OPENROUTER_KEY);
        assert_eq!(cfg.embedding.base_url, OPENROUTER_BASE_URL);
    }

    #[test]
    fn env_overrides_openai_key_defaults_base_url_to_openai() {
        let mut cfg = parse_starter();
        let mut map = HashMap::new();
        map.insert(ENV_OPENAI_KEY, "sk-...");
        apply_env_overrides(&mut cfg, &getter(&map)).expect("override should succeed");
        assert_eq!(cfg.embedding.api_key_env, ENV_OPENAI_KEY);
        assert_eq!(cfg.embedding.base_url, OPENAI_BASE_URL);
    }

    #[test]
    fn env_overrides_openai_key_with_custom_base_url_uses_custom() {
        let mut cfg = parse_starter();
        let mut map = HashMap::new();
        map.insert(ENV_OPENAI_KEY, "sk-...");
        map.insert(ENV_OPENAI_BASE_URL, "https://my-proxy.example/v1");
        apply_env_overrides(&mut cfg, &getter(&map)).expect("override should succeed");
        assert_eq!(cfg.embedding.api_key_env, ENV_OPENAI_KEY);
        assert_eq!(cfg.embedding.base_url, "https://my-proxy.example/v1");
    }

    #[test]
    fn env_overrides_both_keys_set_is_an_error() {
        let mut cfg = parse_starter();
        let mut map = HashMap::new();
        map.insert(ENV_OPENROUTER_KEY, "sk-or-...");
        map.insert(ENV_OPENAI_KEY, "sk-...");
        let err = apply_env_overrides(&mut cfg, &getter(&map))
            .expect_err("both keys set should fail");
        let msg = err.to_string();
        assert!(msg.contains(ENV_OPENROUTER_KEY));
        assert!(msg.contains(ENV_OPENAI_KEY));
    }

    #[test]
    fn env_overrides_model_and_dim_take_effect() {
        let mut cfg = parse_starter();
        let mut map = HashMap::new();
        map.insert(ENV_EMBEDDING_MODEL, "intfloat/e5-base-v2");
        map.insert(ENV_EMBEDDING_DIM, "512");
        apply_env_overrides(&mut cfg, &getter(&map)).expect("override should succeed");
        assert_eq!(cfg.embedding.model, "intfloat/e5-base-v2");
        assert_eq!(cfg.embedding.dim, 512);
    }

    #[test]
    fn env_overrides_non_numeric_dim_returns_a_helpful_error() {
        let mut cfg = parse_starter();
        let mut map = HashMap::new();
        map.insert(ENV_EMBEDDING_DIM, "not-a-number");
        let err = apply_env_overrides(&mut cfg, &getter(&map))
            .expect_err("non-numeric dim should fail");
        let msg = err.to_string();
        assert!(msg.contains(ENV_EMBEDDING_DIM));
        assert!(msg.contains("not-a-number"));
    }
}
