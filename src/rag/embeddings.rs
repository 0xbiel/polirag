use anyhow::{Result, Context};
use std::sync::Arc;
use std::io::Write;
use tempfile::NamedTempFile;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::AddBos;

// Embed the model directly into the binary
const MODEL_BYTES: &[u8] = include_bytes!("../../embeddinggemma-300m-Q4_0.gguf");

struct LlamaState {
    // We keep the backend alive
    backend: Arc<LlamaBackend>,
    // We keep the model alive
    model: Arc<LlamaModel>,
    // We keep the temporary file alive so it isn't deleted while needed
    _temp_file: Arc<NamedTempFile>,
}

#[derive(Clone)]
pub struct EmbeddingModel {
    state: Arc<LlamaState>,
    context_params: LlamaContextParams,
}

// Approximate characters per token ratio
const CHARS_PER_TOKEN: usize = 2;
const MAX_TOKENS: usize = 512; 
const MAX_CHUNK_CHARS: usize = MAX_TOKENS * CHARS_PER_TOKEN;

extern "C" fn log_callback(_level: llama_cpp_sys_2::ggml_log_level, _text: *const std::os::raw::c_char, _user_data: *mut std::ffi::c_void) {
    // Silently ignore all logs
}

use std::num::NonZeroU32;

// ...

impl EmbeddingModel {
    pub fn new() -> Result<Self> {
        // Disable logging
        unsafe {
            llama_cpp_sys_2::llama_log_set(Some(log_callback), std::ptr::null_mut());
        }
        
        // Silence Metal logs
        std::env::set_var("GGML_METAL_NDEBUG", "1");

        // Initialize backend
        let backend = Arc::new(LlamaBackend::init()?);

        // Write model to temp file
        let mut temp_file = NamedTempFile::new()?;
        temp_file.write_all(MODEL_BYTES)?;
        temp_file.flush()?; // Ensure written
        let path = temp_file.path();

        // Offload all layers to GPU (Metal on macOS) for maximum acceleration
        // Setting n_gpu_layers to a high number ensures all layers run on GPU
        let model_params = LlamaModelParams::default()
            .with_n_gpu_layers(999); // Offload ALL layers to Metal
        let model = LlamaModel::load_from_file(backend.as_ref(), path, &model_params)
            .context("Failed to load Llama model from temp file")?;

        // We enable embeddings in context params
        // Set n_batch to be large enough (e.g. 2048) to avoid "n_ubatch >= n_tokens" assert
        // Also set n_ctx to ensure we have room.
        let context_params = LlamaContextParams::default()
            .with_n_ctx(Some(NonZeroU32::new(4096).unwrap()))
            .with_n_batch(2048)
            .with_n_ubatch(2048)
            .with_embeddings(true);

        let state = Arc::new(LlamaState {
            backend,
            model: Arc::new(model),
            _temp_file: Arc::new(temp_file),
        });

        Ok(Self {
            state,
            context_params,
        })
    }

    /// Embed text, chunking if necessary and averaging embeddings
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let text = text.replace("\n", " ");
        
        let chunks = if text.len() <= MAX_CHUNK_CHARS {
            vec![text.clone()]
        } else {
             self.chunk_text(&text)
        };

        if chunks.is_empty() {
             anyhow::bail!("No chunks generated from input text");
        }

        // Process chunks sequentially
        // We use tokio::task::spawn_blocking because inference is blocking and heavy
        let mut embeddings = Vec::new();

        for chunk in chunks {
            let state = self.state.clone();
            let ctx_params = self.context_params.clone();
            let chunk_text = chunk.clone();
            
            let embedding = tokio::task::spawn_blocking(move || -> Result<Vec<f32>> {
                 Self::inference(&state.backend, &state.model, &ctx_params, &chunk_text)
            }).await??;
            
            embeddings.push(embedding);
        }

        if embeddings.is_empty() {
            anyhow::bail!("No embeddings generated");
        }
        
        let dim = embeddings[0].len();
        let mut averaged = vec![0.0f32; dim];
        for emb in &embeddings {
            for (i, val) in emb.iter().enumerate() {
                averaged[i] += val;
            }
        }
        let n = embeddings.len() as f32;
        for val in &mut averaged {
            *val /= n;
        }
        
        let norm: f32 = averaged.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for val in &mut averaged {
                *val /= norm;
            }
        }
        
        Ok(averaged)
    }
    
    fn inference(backend: &LlamaBackend, model: &LlamaModel, ctx_params: &LlamaContextParams, text: &str) -> Result<Vec<f32>> {
        let text = text.replace('\0', ""); // Sanitize null bytes for C interoperability
        
        tracing::debug!("Starting inference for text length: {}", text.len());
        // Create a fresh context for this inference
        let mut ctx = model.new_context(backend, ctx_params.clone())
            .context("Failed to create context")?;
        tracing::debug!("Context created.");
            
        // Tokenize
        let tokens = model.str_to_token(&text, AddBos::Always)
            .map_err(|e| anyhow::anyhow!("Tokenization error: {}", e))?;
        tracing::debug!("Tokenized into {} tokens.", tokens.len());
 
        // Create batch
        // We evaluate all tokens at once
        let mut batch = LlamaBatch::new(tokens.len(), 1); 
        let last_index = tokens.len() as i32 - 1;
        for (i, token) in tokens.iter().enumerate() {
            // logits=true for the last one usually ensures embedding calculation?
            // "If the model is an embedding model, the embedding is computed for the prompt."
            // We set logits=true for the last token just in case.
            batch.add(*token, i as i32, &[0], i as i32 == last_index)?;
        }

        tracing::debug!("Decoding batch with {} tokens...", tokens.len());
        ctx.decode(&mut batch).context("Failed to decode batch")?;
        tracing::debug!("Batch decoded.");

        // Extract embedding
        // For embedding models, use embeddings_seq_ith(0) to get the pooled sequence embedding
        // embeddings_ith returns per-token embeddings which are often zero for embedding models
        let embedding_slice = ctx.embeddings_seq_ith(0)
             .context("Failed to get sequence embedding")?;
        
        // Debug: log embedding stats
        let emb_len = embedding_slice.len();
        let emb_sum: f32 = embedding_slice.iter().sum();
        let emb_norm: f32 = embedding_slice.iter().map(|x| x * x).sum::<f32>().sqrt();
        let non_zero_count = embedding_slice.iter().filter(|x| **x != 0.0).count();
        tracing::info!("Embedding: len={}, sum={:.4}, norm={:.4}, non_zero={}", 
                       emb_len, emb_sum, emb_norm, non_zero_count);
        
        if emb_norm == 0.0 || non_zero_count == 0 {
            tracing::warn!("WARNING: Embedding is all zeros! Model may not be producing embeddings correctly.");
        }
             
        Ok(embedding_slice.to_vec())
    }

    fn chunk_text(&self, text: &str) -> Vec<String> {
        let mut chunks = Vec::new();
        let words: Vec<&str> = text.split_whitespace().collect();
        let mut current_chunk = String::new();
        for word in words {
            if current_chunk.len() + word.len() + 1 > MAX_CHUNK_CHARS {
                if !current_chunk.is_empty() {
                    chunks.push(current_chunk.trim().to_string());
                    current_chunk = String::new();
                }
            }
            if !current_chunk.is_empty() { current_chunk.push(' '); }
            current_chunk.push_str(word);
        }
        if !current_chunk.is_empty() { chunks.push(current_chunk.trim().to_string()); }
        chunks
    }
    

}
