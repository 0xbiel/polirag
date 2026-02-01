pub mod embeddings;
pub mod store;
pub mod hnsw_store;

use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use crate::rag::store::VectorStore;
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Document {
    pub id: String,
    pub content: String,
    pub embedding: Vec<f32>,
    pub metadata: HashMap<String, String>,
    pub user_id: String,
}

pub struct RagSystem {
    store: Arc<Mutex<Box<dyn VectorStore>>>,
    embedder: Arc<embeddings::EmbeddingModel>,
}

/// Statistics about the RAG index
pub struct RagStats {
    pub document_count: usize,
    pub docs_by_type: HashMap<String, usize>,
    pub total_content_bytes: usize,
    pub embedding_dimensions: usize,
    pub file_size_bytes: u64,
    pub storage_path: String,
    pub store_type: String,
    pub chunking_strategy: String,
    pub embedding_model: String,
}

impl RagStats {
    /// Format file size in human readable format
    pub fn format_file_size(&self) -> String {
        let bytes = self.file_size_bytes;
        if bytes < 1024 {
            format!("{} B", bytes)
        } else if bytes < 1024 * 1024 {
            format!("{:.2} KB", bytes as f64 / 1024.0)
        } else if bytes < 1024 * 1024 * 1024 {
            format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
        } else {
            format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
        }
    }

    /// Format content size in human readable format
    pub fn format_content_size(&self) -> String {
        let bytes = self.total_content_bytes;
        if bytes < 1024 {
            format!("{} B", bytes)
        } else if bytes < 1024 * 1024 {
            format!("{:.2} KB", bytes as f64 / 1024.0)
        } else {
            format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
        }
    }
}

impl RagSystem {
    pub fn new(storage_path: &str) -> anyhow::Result<Self> {
        let embedder = Arc::new(embeddings::EmbeddingModel::new()?);
        
        // Check if HNSW index exists
        let hnsw_path = Path::new(storage_path).with_extension("hnsw");
        
        let mut store = hnsw_store::HnswVectorStore::new(storage_path)?;
        
        // Migration logic: If HNSW didn't exist but Linear store does, migrate
        if !hnsw_path.exists() && Path::new(storage_path).exists() {
             tracing::info!("Migrating from Linear Store to HNSW Store...");
             match store::LinearVectorStore::new(storage_path) {
                 Ok(old_store) => {
                     let docs = old_store.get_all()?;
                     tracing::info!("Found {} documents to migrate.", docs.len());
                     for doc in docs {
                         store.add_document(doc)?;
                     }
                     store.save()?;
                     tracing::info!("Migration complete.");
                 },
                 Err(e) => {
                     tracing::warn!("Failed to open existing linear store for migration: {}", e);
                 }
             }
        }

        Ok(Self {
            store: Arc::new(Mutex::new(Box::new(store))),
            embedder,
        })
    }

    pub async fn add_document(&self, id: &str, content: &str, user_id: &str, meta: HashMap<String, String>) -> anyhow::Result<()> {
        let embedding = self.embedder.embed(content).await?;
        
        let doc = Document {
            id: id.to_string(),
            content: content.to_string(),
            embedding,
            metadata: meta,
            user_id: user_id.to_string(),
        };

        let mut store = self.store.lock().unwrap();
        store.add_document(doc)?;
        Ok(())
    }

    pub fn count_documents(&self) -> usize {
        self.store.lock().unwrap().count()
    }

    /// Clear all documents from the index
    pub fn clear(&self) -> anyhow::Result<()> {
        let mut store = self.store.lock().unwrap();
        store.clear()
    }
    
    /// Recalculate embeddings for all documents
    /// progress_fn receives (current, total, doc_id, metadata)
    pub async fn reembed_all<F>(&self, mut progress_fn: F) -> anyhow::Result<usize>
    where
        F: FnMut(usize, usize, &str, &HashMap<String, String>),
    {
        // Get all document contents
        let docs = {
            let store = self.store.lock().unwrap();
            store.get_all()?
        };
        
        let total = docs.len();
        let mut reembedded = 0;
        
        for (i, old_doc) in docs.into_iter().enumerate() {
            progress_fn(i + 1, total, &old_doc.id, &old_doc.metadata);
            
            // Recalculate embedding
            let embedding_res = self.embedder.embed(&old_doc.content).await;
            
            match embedding_res {
                Ok(embedding) => {
                    // Update document
                    let mut doc = old_doc.clone();
                    doc.embedding = embedding;
                    
                    let mut store = self.store.lock().unwrap();
                    store.add_document(doc)?;
                    reembedded += 1;
                },
                Err(e) => {
                    tracing::error!("Failed to re-embed output document {}: {}", old_doc.id, e);
                }
            }
        }
        
        let store = self.store.lock().unwrap();
        store.save()?;
        
        Ok(reembedded)
    }

    /// Get comprehensive statistics about the RAG index
    pub fn get_stats(&self) -> RagStats {
        let store = self.store.lock().unwrap();
        let stats = store.get_stats();
        let storage_path = store.storage_path();
        let store_type = store.store_type();
        
        RagStats {
            document_count: stats.document_count,
            docs_by_type: stats.docs_by_type,
            total_content_bytes: stats.total_content_bytes,
            embedding_dimensions: stats.embedding_dimensions,
            file_size_bytes: stats.file_size_bytes, 
            storage_path,
            store_type,
            chunking_strategy: self.embedder.chunking_strategy(),
            embedding_model: self.embedder.model_name(),
        }
    }

    pub async fn search(&self, query: &str, user_id: &str, top_k: usize) -> anyhow::Result<Vec<(Document, f32)>> {
        let query_embedding = self.embedder.embed(query).await?;
        let store = self.store.lock().unwrap();
        store.search(&query_embedding, user_id, top_k, 0.0)
    }
    
    /// Search and return concise snippets suitable for LLM context
    pub async fn search_snippets(&self, query: &str, user_id: &str, top_k: usize) -> anyhow::Result<Vec<(String, String, f32)>> {
        let query_embedding = self.embedder.embed(query).await?;
        
        let candidates = {
            let store = self.store.lock().unwrap();
            store.search(&query_embedding, user_id, top_k * 2, 0.3)?
        };
        
        tracing::debug!("RAG Search: Found {} candidates (pre-filter)", candidates.len());
        
        if !candidates.is_empty() {
            let top_5: Vec<f32> = candidates.iter().take(5).map(|(_,s)| *s).collect();
            tracing::info!("RAG Search: Top 5 scores: {:?}", top_5);
        }
        
        let query_lower = query.to_lowercase();
        let query_words: Vec<String> = query_lower.split_whitespace().map(|s| s.to_string()).collect();
        
        let mut snippets: Vec<(String, String, f32)> = candidates.into_iter()
            .map(|(doc, score)| {
                let source = doc.metadata.get("type")
                    .map(|t| {
                        if t == "subject" {
                            doc.id.clone()
                        } else {
                            doc.metadata.get("filename").cloned().unwrap_or(doc.id.clone())
                        }
                    })
                    .unwrap_or(doc.id.clone());
                
                let snippet = extract_relevant_snippet(&doc.content, &query_words, 1500);
                (source, snippet, score)
            })
            .collect();
            
        if snippets.len() > top_k {
            snippets.truncate(top_k);
        }
        
        Ok(snippets)
    }
}

/// Extract the most relevant snippet from content based on query words
fn extract_relevant_snippet(content: &str, query_words: &[String], max_chars: usize) -> String {
    let mut best_pos = 0;
    let mut best_score = 0;
    
    let words: Vec<&str> = content.split_whitespace().collect();
    let window_size = 50; 
    
    for i in 0..words.len().saturating_sub(window_size) {
        let window: String = words[i..i + window_size].join(" ").to_lowercase();
        let score: usize = query_words.iter()
            .filter(|qw| window.contains(*qw))
            .count();
        
        if score > best_score {
            best_score = score;
            best_pos = words[..i].iter().map(|w| w.len() + 1).sum::<usize>();
        }
    }
    
    let start = best_pos.saturating_sub(50);
    let end = (start + max_chars).min(content.len());
    
    let mut snippet: String = content.chars().skip(start).take(end - start).collect();
    
    if start > 0 {
        if let Some(pos) = snippet.find(' ') {
            snippet = snippet[pos + 1..].to_string();
        }
        snippet = format!("...{}", snippet);
    }
    
    if end < content.len() {
        if let Some(pos) = snippet.rfind(' ') {
            snippet = snippet[..pos].to_string();
        }
        snippet = format!("{}...", snippet);
    }
    
    snippet.trim().to_string()
}
