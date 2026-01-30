pub mod embeddings;

use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::fs::File;
use std::io::{BufReader, BufWriter};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Document {
    pub id: String,
    pub content: String,
    pub embedding: Vec<f32>,
    pub metadata: HashMap<String, String>,
    pub user_id: String,
}

#[derive(Serialize, Deserialize, Default)]
struct VectorIndex {
    documents: Vec<Document>,
}

pub struct RagSystem {
    db: Arc<Mutex<VectorIndex>>,
    embedder: Arc<embeddings::EmbeddingModel>,
    storage_path: String,
}

/// Statistics about the RAG index
pub struct RagStats {
    pub document_count: usize,
    pub docs_by_type: HashMap<String, usize>,
    pub total_content_bytes: usize,
    pub embedding_dimensions: usize,
    pub file_size_bytes: u64,
    pub storage_path: String,
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
        
        let db = if std::path::Path::new(storage_path).exists() {
            let file = File::open(storage_path)?;
            let reader = BufReader::new(file);
            bincode::deserialize_from(reader).unwrap_or_default()
        } else {
            VectorIndex::default()
        };

        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            embedder,
            storage_path: storage_path.to_string(),
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

        let mut db = self.db.lock().unwrap();
        db.documents.retain(|d| d.id != id);
        db.documents.push(doc);
        
        self.save_internal(&db)?;
        Ok(())
    }

    pub fn count_documents(&self) -> usize {
        self.db.lock().unwrap().documents.len()
    }

    /// Clear all documents from the index
    pub fn clear(&self) -> anyhow::Result<()> {
        let mut db = self.db.lock().unwrap();
        db.documents.clear();
        self.save_internal(&db)?;
        Ok(())
    }
    
    /// Recalculate embeddings for all documents
    /// progress_fn receives (current, total, doc_id, metadata)
    pub async fn reembed_all<F>(&self, mut progress_fn: F) -> anyhow::Result<usize>
    where
        F: FnMut(usize, usize, &str, &HashMap<String, String>),
    {
        // Get all document contents
        let docs_data: Vec<(String, String, String, HashMap<String, String>)> = {
            let db = self.db.lock().unwrap();
            db.documents.iter()
                .map(|d| (d.id.clone(), d.content.clone(), d.user_id.clone(), d.metadata.clone()))
                .collect()
        };
        
        let total = docs_data.len();
        let mut reembedded = 0;
        
        for (i, (id, content, user_id, metadata)) in docs_data.into_iter().enumerate() {
            progress_fn(i + 1, total, &id, &metadata);
            
            // Recalculate embedding
            let embedding_res = self.embedder.embed(&content).await;
            
            match embedding_res {
                Ok(embedding) => {
                    // Update document
                    let doc = Document {
                        id: id.clone(),
                        content,
                        embedding,
                        metadata,
                        user_id,
                    };
                    
                    let mut db = self.db.lock().unwrap();
                    db.documents.retain(|d| d.id != id);
                    db.documents.push(doc);
                    reembedded += 1;
                },
                Err(e) => {
                    tracing::error!("Failed to re-embed output document {}: {}", id, e);
                    // Continue to next document
                }
            }
        }
        
        // Save at the end
        let db = self.db.lock().unwrap();
        self.save_internal(&db)?;
        
        Ok(reembedded)
    }

    /// Get comprehensive statistics about the RAG index
    pub fn get_stats(&self) -> RagStats {
        let db = self.db.lock().unwrap();
        
        // Count documents by type
        let mut docs_by_type: HashMap<String, usize> = HashMap::new();
        let mut total_content_bytes: usize = 0;
        let mut total_embedding_dims: usize = 0;
        
        for doc in &db.documents {
            total_content_bytes += doc.content.len();
            total_embedding_dims = doc.embedding.len(); // All same dimension
            
            let doc_type = doc.metadata.get("type").cloned().unwrap_or_else(|| "unknown".to_string());
            *docs_by_type.entry(doc_type).or_insert(0) += 1;
        }
        
        // Get file size on disk
        let file_size_bytes = std::fs::metadata(&self.storage_path)
            .map(|m| m.len())
            .unwrap_or(0);
        
        RagStats {
            document_count: db.documents.len(),
            docs_by_type,
            total_content_bytes,
            embedding_dimensions: total_embedding_dims,
            file_size_bytes,
            storage_path: self.storage_path.clone(),
        }
    }

    pub async fn search(&self, query: &str, user_id: &str, top_k: usize) -> anyhow::Result<Vec<(Document, f32)>> {
        // Embed the query
        let query_embedding = self.embedder.embed(query).await?;
        let db = self.db.lock().unwrap();
        
        let mut scores: Vec<(Document, f32)> = db.documents.iter()
            .filter(|d| d.user_id == user_id)
            .map(|d| {
                let score = cosine_similarity(&query_embedding, &d.embedding);
                (d.clone(), score)
            })
            .collect();
            
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scores.truncate(top_k);
        
        Ok(scores)
    }
    
    /// Search and return concise snippets suitable for LLM context
    /// Returns ALL documents above relevance threshold, not limited by top_k
    pub async fn search_snippets(&self, query: &str, user_id: &str, top_k: usize) -> anyhow::Result<Vec<(String, String, f32)>> {
        // Get all documents, not limited
        let query_embedding = self.embedder.embed(query).await?;
        let db = self.db.lock().unwrap();
        
        let mut scores: Vec<(Document, f32)> = db.documents.iter()
            .filter(|d| d.user_id == user_id)
            .map(|d| {
                let score = cosine_similarity(&query_embedding, &d.embedding);
                (d.clone(), score)
            })
            .collect();
            
        tracing::debug!("RAG Search: Found {} candidates (pre-filter)", scores.len());
        
        // Log top 5 scores for debugging
        let mut sorted_scores: Vec<f32> = scores.iter().map(|(_, s)| *s).collect();
        sorted_scores.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        if !sorted_scores.is_empty() {
            let top_5: Vec<f32> = sorted_scores.iter().take(5).copied().collect();
            tracing::info!("RAG Search: Top 5 scores: {:?}", top_5);
        }
        
        // Filter out low-relevance results
        // With working embeddings, 0.3 is a reasonable threshold for semantic similarity
        let min_threshold = 0.3;
        scores.retain(|(_, score)| *score > min_threshold);
        
        tracing::debug!("RAG Search: {} candidates passed threshold > {}", scores.len(), min_threshold);
            
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        
        if scores.len() > top_k {
            scores.truncate(top_k);
        }
        
        let query_lower = query.to_lowercase();
        let query_words: Vec<String> = query_lower.split_whitespace().map(|s| s.to_string()).collect();
        
        let snippets: Vec<(String, String, f32)> = scores.into_iter()
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
        
        Ok(snippets)
    }
    fn save_internal(&self, idx: &VectorIndex) -> anyhow::Result<()> {
        let file = File::create(&self.storage_path)?;
        let writer = BufWriter::new(file);
        bincode::serialize_into(writer, idx)?;
        Ok(())
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot_product: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    
    if norm_a == 0.0 || norm_b == 0.0 {
        // Log when we get zero norm - this helps debug
        tracing::trace!("cosine_similarity: norm_a={:.4}, norm_b={:.4}, dims=({}, {})", 
                       norm_a, norm_b, a.len(), b.len());
        0.0
    } else {
        dot_product / (norm_a * norm_b)
    }
}

/// Extract the most relevant snippet from content based on query words
fn extract_relevant_snippet(content: &str, query_words: &[String], max_chars: usize) -> String {
    // Find the best starting position based on query word matches
    let mut best_pos = 0;
    let mut best_score = 0;
    
    // Scan through content in chunks looking for query word density
    let words: Vec<&str> = content.split_whitespace().collect();
    let window_size = 50; // words
    
    for i in 0..words.len().saturating_sub(window_size) {
        let window: String = words[i..i + window_size].join(" ").to_lowercase();
        let score: usize = query_words.iter()
            .filter(|qw| window.contains(*qw))
            .count();
        
        if score > best_score {
            best_score = score;
            // Calculate character position
            best_pos = words[..i].iter().map(|w| w.len() + 1).sum::<usize>();
        }
    }
    
    // Extract snippet around best position
    let start = best_pos.saturating_sub(50);
    let end = (start + max_chars).min(content.len());
    
    let mut snippet: String = content.chars().skip(start).take(end - start).collect();
    
    // Clean up the snippet
    if start > 0 {
        // Trim to first word boundary
        if let Some(pos) = snippet.find(' ') {
            snippet = snippet[pos + 1..].to_string();
        }
        snippet = format!("...{}", snippet);
    }
    
    if end < content.len() {
        // Trim to last word boundary
        if let Some(pos) = snippet.rfind(' ') {
            snippet = snippet[..pos].to_string();
        }
        snippet = format!("{}...", snippet);
    }
    
    snippet.trim().to_string()
}
