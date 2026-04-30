//! LanceDB-backed store. Schema: id, path, chunk_index, content, content_hash, file_hash, vector.
//!
//! Schema change from v0.1: added `file_hash`. Existing data dirs need to be deleted
//! and reindexed (the schema mismatch will fail on `open_table`).

use anyhow::{anyhow, Context, Result};
use arrow_array::{
    builder::{FixedSizeListBuilder, Float32Builder},
    Float32Array, RecordBatch, RecordBatchIterator, StringArray, UInt32Array,
};
use arrow_schema::{DataType, Field, FieldRef, Schema};
use futures::TryStreamExt;
use lancedb::{
    query::{ExecutableQuery, QueryBase, Select},
    Connection, Table,
};
use std::sync::Arc;

use crate::{chunk::Chunk, config::Config};

const TABLE: &str = "chunks";

pub(crate) struct Store {
    _conn: Connection,
    table: Table,
    dim: i32,
}

#[derive(Debug)]
pub(crate) struct SearchHit {
    pub score: f32,
    pub path: String,
    pub chunk_index: u32,
    pub content: String,
}

#[derive(Debug)]
pub(crate) struct Stats {
    pub chunk_count: usize,
}

impl Store {
    pub(crate) async fn open(cfg: &Config) -> Result<Self> {
        std::fs::create_dir_all(&cfg.data_dir)?;
        let uri = cfg.data_dir.to_string_lossy().to_string();
        let conn = lancedb::connect(&uri)
            .execute()
            .await
            .context("opening lancedb")?;

        let dim = cfg.embedding.dim as i32;
        let schema = make_schema(dim);

        let table = match conn.open_table(TABLE).execute().await {
            Ok(t) => t,
            Err(_) => {
                let empty = RecordBatchIterator::new(std::iter::empty(), schema.clone());
                conn.create_table(TABLE, Box::new(empty))
                    .execute()
                    .await
                    .context("creating chunks table")?
            }
        };

        Ok(Self {
            _conn: conn,
            table,
            dim,
        })
    }

    /// Returns true if the store already has chunks for `path` matching `file_hash`.
    /// This is the fast path that lets `index` and `watch` skip unchanged files.
    pub(crate) async fn file_already_indexed(&self, path: &str, file_hash: &str) -> Result<bool> {
        let path_esc = path.replace('\'', "''");
        let hash_esc = file_hash.replace('\'', "''");
        let predicate = format!("path = '{path_esc}' AND file_hash = '{hash_esc}'");
        let stream = self
            .table
            .query()
            .only_if(predicate)
            .select(Select::Columns(vec!["chunk_index".into()]))
            .limit(1)
            .execute()
            .await?;
        let batches: Vec<RecordBatch> = stream.try_collect().await?;
        Ok(batches.iter().any(|b| b.num_rows() > 0))
    }

    /// Delete every chunk for a given path. Used before reindexing a file
    /// (replace strategy is simpler than diffing chunk-by-chunk).
    pub(crate) async fn delete_path(&self, path: &str) -> Result<()> {
        let path_esc = path.replace('\'', "''");
        self.table
            .delete(&format!("path = '{path_esc}'"))
            .await
            .context("deleting chunks for path")?;
        Ok(())
    }

    pub(crate) async fn upsert(&self, chunks: Vec<Chunk>, vecs: Vec<Vec<f32>>) -> Result<()> {
        if chunks.len() != vecs.len() {
            return Err(anyhow!("chunks/vecs length mismatch"));
        }
        let batch = chunks_to_batch(&chunks, &vecs, self.dim)?;
        let schema = batch.schema();
        let iter = RecordBatchIterator::new(std::iter::once(Ok(batch)), schema);
        self.table
            .add(Box::new(iter))
            .execute()
            .await
            .context("inserting chunks")?;
        Ok(())
    }

    pub(crate) async fn search(&self, query: &[f32], limit: usize) -> Result<Vec<SearchHit>> {
        let stream = self
            .table
            .vector_search(query.to_vec())?
            .limit(limit)
            .execute()
            .await?;
        let batches: Vec<RecordBatch> = stream.try_collect().await?;

        let mut hits = Vec::new();
        for batch in batches {
            let path = batch
                .column_by_name("path")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .ok_or_else(|| anyhow!("missing path column"))?;
            let chunk_idx = batch
                .column_by_name("chunk_index")
                .and_then(|c| c.as_any().downcast_ref::<UInt32Array>())
                .ok_or_else(|| anyhow!("missing chunk_index column"))?;
            let content = batch
                .column_by_name("content")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .ok_or_else(|| anyhow!("missing content column"))?;
            let dist = batch
                .column_by_name("_distance")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>());

            for i in 0..batch.num_rows() {
                hits.push(SearchHit {
                    score: dist.map(|d| 1.0 - d.value(i)).unwrap_or(0.0),
                    path: path.value(i).to_string(),
                    chunk_index: chunk_idx.value(i),
                    content: content.value(i).to_string(),
                });
            }
        }
        Ok(hits)
    }

    pub(crate) async fn stats(&self) -> Result<Stats> {
        let chunk_count = self.table.count_rows(None).await?;
        Ok(Stats { chunk_count })
    }
}

fn make_schema(dim: i32) -> Arc<Schema> {
    let item: FieldRef = Arc::new(Field::new("item", DataType::Float32, true));
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("path", DataType::Utf8, false),
        Field::new("chunk_index", DataType::UInt32, false),
        Field::new("content", DataType::Utf8, false),
        Field::new("content_hash", DataType::Utf8, false),
        Field::new("file_hash", DataType::Utf8, false),
        Field::new("vector", DataType::FixedSizeList(item, dim), false),
    ]))
}

fn chunks_to_batch(chunks: &[Chunk], vecs: &[Vec<f32>], dim: i32) -> Result<RecordBatch> {
    let ids: Vec<String> = chunks
        .iter()
        .map(|c| format!("{}#{}", c.path, c.chunk_index))
        .collect();
    let paths: Vec<&str> = chunks.iter().map(|c| c.path.as_str()).collect();
    let idxs: Vec<u32> = chunks.iter().map(|c| c.chunk_index).collect();
    let contents: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();
    let hashes: Vec<&str> = chunks.iter().map(|c| c.content_hash.as_str()).collect();
    let file_hashes: Vec<&str> = chunks.iter().map(|c| c.file_hash.as_str()).collect();

    let mut vec_builder = FixedSizeListBuilder::new(
        Float32Builder::with_capacity(vecs.len() * dim as usize),
        dim,
    );
    for v in vecs {
        if v.len() != dim as usize {
            return Err(anyhow!(
                "vector dim mismatch: got {}, expected {}",
                v.len(),
                dim
            ));
        }
        vec_builder.values().append_slice(v);
        vec_builder.append(true);
    }
    let vector_arr = vec_builder.finish();

    let schema = make_schema(dim);
    Ok(RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(ids)),
            Arc::new(StringArray::from(paths)),
            Arc::new(UInt32Array::from(idxs)),
            Arc::new(StringArray::from(contents)),
            Arc::new(StringArray::from(hashes)),
            Arc::new(StringArray::from(file_hashes)),
            Arc::new(vector_arr),
        ],
    )?)
}
