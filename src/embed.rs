//! Embedding client over an OpenAI-compatible `/v1/embeddings` endpoint.
//!
//! Defaults at OpenRouter but `embedding.base_url` in the config swaps it to
//! OpenAI direct or any other compatible provider. The whole batch fails on
//! the first 4xx/5xx -- the caller is expected to retry the file.

use anyhow::{anyhow, bail, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::config::Config;

#[derive(Debug)]
pub(crate) struct OpenRouterEmbedder {
    pub batch_size: usize,
    dim: usize,
    model: String,
    base_url: String,
    api_key: String,
    http: Client,
}

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Deserialize)]
struct EmbedResponse {
    data: Vec<EmbedItem>,
}

#[derive(Deserialize)]
struct EmbedItem {
    embedding: Vec<f32>,
}

impl OpenRouterEmbedder {
    pub(crate) fn new(cfg: &Config) -> Result<Self> {
        if cfg.embedding.batch_size == 0 {
            bail!("embedding.batch_size must be > 0");
        }
        if cfg.embedding.dim == 0 {
            bail!("embedding.dim must be > 0");
        }
        let api_key = std::env::var(&cfg.embedding.api_key_env).with_context(|| {
            format!(
                "env var `{}` is unset -- export it before indexing or running `serve`",
                cfg.embedding.api_key_env
            )
        })?;
        let http = Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            batch_size: cfg.embedding.batch_size,
            dim: cfg.embedding.dim,
            model: cfg.embedding.model.clone(),
            base_url: cfg.embedding.base_url.trim_end_matches('/').to_string(),
            api_key,
            http,
        })
    }

    pub(crate) async fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let texts = vec![text.to_string()];
        let mut v = self.embed_batch(&texts).await?;
        v.pop()
            .ok_or_else(|| anyhow!("embedding endpoint returned no rows"))
    }

    pub(crate) async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{}/embeddings", self.base_url);
        let body = EmbedRequest {
            model: &self.model,
            input: texts,
        };
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            // OpenRouter etiquette: optional but recommended identifying headers.
            .header("HTTP-Referer", "https://github.com/lilctx/lilctx")
            .header("X-Title", "lilctx")
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let detail = resp.text().await.unwrap_or_default();
            bail!("embedding request failed: {status}: {detail}");
        }

        let parsed: EmbedResponse = resp
            .json()
            .await
            .context("decoding embedding response JSON")?;

        if parsed.data.len() != texts.len() {
            bail!(
                "embedding endpoint returned {} rows for {} inputs",
                parsed.data.len(),
                texts.len()
            );
        }

        let mut out = Vec::with_capacity(parsed.data.len());
        for item in parsed.data {
            if item.embedding.len() != self.dim {
                bail!(
                    "embedding dim mismatch from API: got {}, configured dim = {} \
                     (delete data_dir and reindex if you changed models)",
                    item.embedding.len(),
                    self.dim
                );
            }
            out.push(item.embedding);
        }
        Ok(out)
    }
}
