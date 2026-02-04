use super::Document;
use anyhow::Result;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;
use serde::{Serialize, Deserialize};

/// Trait for vector storage backends
pub trait VectorStore: Send + Sync {
    /// Add a document to the store
    fn add_document(&mut self, doc: Document) -> Result<()>;
    
    /// Search for similar documents
    fn search(&self, query_embedding: &[f32], user_id: &str, top_k: usize, min_threshold: f32) -> Result<Vec<(Document, f32)>>;
    
    /// Get all documents (for re-embedding or migration)
    fn get_all(&self) -> Result<Vec<Document>>;
    
    /// Count documents
    fn count(&self) -> usize;
    
    /// Clear all documents
    fn clear(&mut self) -> Result<()>;
    
    /// Save index to disk
    fn save(&self) -> Result<()>;
    
    /// Get storage path or description
    fn storage_path(&self) -> String;

    /// Get statistics
    fn get_stats(&self) -> StoreStats;
    
    /// Get store type description (e.g. "Linear Scan", "HNSW")
    /// Get store type description (e.g. "Linear Scan", "HNSW")
    fn store_type(&self) -> String;
    
    /// Check if a document with the given ID exists
    fn contains(&self, id: &str) -> bool;
    
    /// Remove a document by ID
    fn remove_document(&mut self, id: &str) -> Result<()>;

    /// Get documents by metadata key-value pair
    fn get_documents_by_metadata(&self, key: &str, value: &str) -> Result<Vec<Document>>;
}

#[derive(Default)]
pub struct StoreStats {
    pub document_count: usize,
    pub docs_by_type: HashMap<String, usize>,
    pub total_content_bytes: usize,
    pub embedding_dimensions: usize,
    pub file_size_bytes: u64,
}

/// Simple linear scan vector store (legacy/default)
#[derive(Serialize, Deserialize, Default)]
struct LinearIndex {
    documents: Vec<Document>,
}

pub struct LinearVectorStore {
    index: LinearIndex,
    storage_path: String,
}

impl LinearVectorStore {
    pub fn new(storage_path: &str) -> Result<Self> {
        let index = if Path::new(storage_path).exists() {
            let file = File::open(storage_path)?;
            let reader = BufReader::new(file);
            bincode::deserialize_from(reader).unwrap_or_default()
        } else {
            LinearIndex::default()
        };

        Ok(Self {
            index,
            storage_path: storage_path.to_string(),
        })
    }
}

impl VectorStore for LinearVectorStore {
    fn storage_path(&self) -> String {
        self.storage_path.clone()
    }

    fn store_type(&self) -> String {
        "Linear Scan (Exact)".to_string()
    }

    fn add_document(&mut self, doc: Document) -> Result<()> {
        self.index.documents.retain(|d| d.id != doc.id);
        self.index.documents.push(doc);
        self.save()
    }

    fn search(&self, query_embedding: &[f32], user_id: &str, top_k: usize, min_threshold: f32) -> Result<Vec<(Document, f32)>> {
        let mut scores: Vec<(Document, f32)> = self.index.documents.iter()
            .filter(|d| d.user_id == user_id)
            .map(|d| {
                let score = cosine_similarity(query_embedding, &d.embedding);
                (d.clone(), score)
            })
            .filter(|(_, score)| *score > min_threshold)
            .collect();
            
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        
        // Return potentially more than top_k if needed, but usually we truncate here
        // The calling function might want to get all valid candidates, but for now lets strict limit if top_k > 0
        if top_k > 0 && scores.len() > top_k {
            scores.truncate(top_k);
        }
        
        Ok(scores)
    }

    fn get_all(&self) -> Result<Vec<Document>> {
        Ok(self.index.documents.clone())
    }

    fn count(&self) -> usize {
        self.index.documents.len()
    }

    fn clear(&mut self) -> Result<()> {
        self.index.documents.clear();
        self.save()
    }

    fn contains(&self, id: &str) -> bool {
        self.index.documents.iter().any(|d| d.id == id)
    }

    fn remove_document(&mut self, id: &str) -> Result<()> {
        self.index.documents.retain(|d| d.id != id);
        self.save()
    }

    fn get_documents_by_metadata(&self, key: &str, value: &str) -> Result<Vec<Document>> {
        let docs = self.index.documents.iter()
            .filter(|d| d.metadata.get(key).map_or(false, |v| v == value))
            .cloned()
            .collect();
        Ok(docs)
    }

    fn save(&self) -> Result<()> {
        let file = File::create(&self.storage_path)?;
        let writer = BufWriter::new(file);
        bincode::serialize_into(writer, &self.index)?;
        Ok(())
    }
    
    fn get_stats(&self) -> StoreStats {
        let mut docs_by_type: HashMap<String, usize> = HashMap::new();
        let mut total_content_bytes: usize = 0;
        let mut total_embedding_dims: usize = 0;
        
        for doc in &self.index.documents {
            total_content_bytes += doc.content.len();
            total_embedding_dims = doc.embedding.len();
            
            let doc_type = doc.metadata.get("type").cloned().unwrap_or_else(|| "unknown".to_string());
            *docs_by_type.entry(doc_type).or_insert(0) += 1;
        }
        
        let file_size_bytes = std::fs::metadata(&self.storage_path)
            .map(|m| m.len())
            .unwrap_or(0);
            
        StoreStats {
            document_count: self.index.documents.len(),
            docs_by_type,
            total_content_bytes,
            embedding_dimensions: total_embedding_dims,
            file_size_bytes,
        }
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot_product: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot_product / (norm_a * norm_b)
    }
}
