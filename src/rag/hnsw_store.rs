use super::{Document, store::{VectorStore, StoreStats}};
use anyhow::{Result, Context};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs::File;
use std::io::{BufReader, BufWriter};
use serde::{Serialize, Deserialize};
use hnsw_rs::prelude::*;
use hnsw_rs::hnswio::HnswIo;
use hnsw_rs::api::AnnT;
use std::sync::RwLock;

// Wrapper struct for serialization
#[derive(Serialize, Deserialize)]
struct StoredData {
    documents: HashMap<usize, Document>,
    next_id: usize,
    // We don't serialize HNSW here, it has its own method
}

pub struct HnswVectorStore {
    hnsw: RwLock<Hnsw<'static, f32, DistCosine>>,
    documents: RwLock<HashMap<usize, Document>>, // Internal ID -> Document
    id_map: RwLock<HashMap<String, usize>>,      // External ID -> Internal ID
    next_id: RwLock<usize>,
    storage_path: PathBuf,
}

impl HnswVectorStore {
    pub fn new(storage_path: &str) -> Result<Self> {
        let path = Path::new(storage_path);
        let _hnsw_path = path.with_extension("hnsw.graph"); // hnsw_rs appends .graph and .data
        let data_path = path.with_extension("data");

        // HNSW file naming convention in hnsw_rs: basename.hnsw.graph
        // So checking existence might be tricky if we don't know exact name.
        // HnswIo usually uses basename.
        
        // We will assume if data_path exists, we can try to load.
        let (hnsw, documents, next_id) = if data_path.exists() {
            tracing::info!("Loading HNSW index from {:?}", path);
            
            let directory = path.parent().unwrap_or(Path::new("."));
            let basename = path.file_stem().unwrap().to_str().unwrap();
            
            // We need to leak HnswIo because Hnsw returned by load_hnsw takes a lifetime linked to HnswIo.
            // Since we need Hnsw to match HnswVectorStore's 'static lifetime requirement (from VectorStore trait),
            // we must make HnswIo live for 'static.
            // This is a one-time leak per application run (singleton store), so it's acceptable.
            let hnswio = Box::new(HnswIo::new(directory, basename));
            let hnswio = Box::leak(hnswio);
            
            let hnsw = hnswio.load_hnsw::<f32, DistCosine>()
                .context("Failed to load HNSW index")?;
            
            let file = File::open(&data_path)?;
            let reader = BufReader::new(file);
            let data: StoredData = bincode::deserialize_from(reader)?;
            
            (hnsw, data.documents, data.next_id)
        } else {
            tracing::info!("Creating new HNSW index");
            // Parameters can be tuned. M=24, ef_construction=10000 are decent defaults.
            let hnsw = Hnsw::new(24, 10000, 16, 200, DistCosine);
            (hnsw, HashMap::new(), 0)
        };

        // Rebuild reverse map
        let mut id_map = HashMap::new();
        for (internal_id, doc) in &documents {
            id_map.insert(doc.id.clone(), *internal_id);
        }

        Ok(Self {
            hnsw: RwLock::new(hnsw),
            documents: RwLock::new(documents),
            id_map: RwLock::new(id_map),
            next_id: RwLock::new(next_id),
            storage_path: path.to_path_buf(),
        })
    }
}

impl VectorStore for HnswVectorStore {
    fn add_document(&mut self, doc: Document) -> Result<()> {
        let hnsw = self.hnsw.write().unwrap();
        let mut documents = self.documents.write().unwrap();
        let mut id_map = self.id_map.write().unwrap();
        let mut next_id = self.next_id.write().unwrap();

        // Check if exists
        let internal_id = if let Some(&id) = id_map.get(&doc.id) {
            // Re-using ID. 
            // Note: hnsw_rs insert usually allows updating if ID exists, 
            // but older points might linger in graph connectivity until rewrite/optimization.
            id
        } else {
            let id = *next_id;
            *next_id += 1;
            id
        };

        // Insert into HNSW
        // Tuple (data, id)
        hnsw.insert((&doc.embedding, internal_id));
        
        // Update maps
        documents.insert(internal_id, doc.clone());
        id_map.insert(doc.id, internal_id);

        Ok(())
    }

    fn search(&self, query_embedding: &[f32], user_id: &str, top_k: usize, min_threshold: f32) -> Result<Vec<(Document, f32)>> {
        let hnsw = self.hnsw.read().unwrap();
        let documents = self.documents.read().unwrap();

        let ef_search = top_k * 2; 

        // Search returns Vec<Neighbour>
        let neighbors = hnsw.search(query_embedding, top_k, ef_search);
        
        let mut results = Vec::new();

        for neighbor in neighbors {
            if let Some(doc) = documents.get(&neighbor.d_id) {
                // Filter by user_id
                if doc.user_id == user_id {
                    // DistCosine in hnsw_rs: distance = 1.0 - similarity (usually)
                    // Let's assume this based on common practice and crate name.
                    let similarity = 1.0 - neighbor.distance;
                    
                    if similarity >= min_threshold {
                        results.push((doc.clone(), similarity));
                    }
                }
            }
        }
        
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        if top_k > 0 && results.len() > top_k {
            results.truncate(top_k);
        }

        Ok(results)
    }

    fn get_all(&self) -> Result<Vec<Document>> {
        let documents = self.documents.read().unwrap();
        Ok(documents.values().cloned().collect())
    }

    fn count(&self) -> usize {
        self.documents.read().unwrap().len()
    }

    fn clear(&mut self) -> Result<()> {
        let mut hnsw = self.hnsw.write().unwrap();
        let mut documents = self.documents.write().unwrap();
        let mut id_map = self.id_map.write().unwrap();
        let mut next_id = self.next_id.write().unwrap();

        *hnsw = Hnsw::new(24, 10000, 16, 200, DistCosine);
        documents.clear();
        id_map.clear();
        *next_id = 0;
        
        // Need to save to clear files on disk too
        // We drop lock to call save which re-acquires read lock
        drop(hnsw);
        drop(documents);
        drop(next_id);
        
        self.save()
    }

    fn save(&self) -> Result<()> {
        let hnsw = self.hnsw.read().unwrap();
        let documents = self.documents.read().unwrap();
        let next_id = *self.next_id.read().unwrap();

        let data_path = self.storage_path.with_extension("data");

        // Save HNSW
        // Hnsw::file_dump expects directory and basename
        let directory = self.storage_path.parent().unwrap_or(Path::new("."));
        let basename = self.storage_path.file_stem().unwrap().to_str().unwrap();
        
        hnsw.file_dump(directory, basename).context("Failed to save HNSW index")?;

        // Save Data
        let data = StoredData {
            documents: documents.clone(),
            next_id,
        };
        
        let file = File::create(&data_path)?;
        let writer = BufWriter::new(file);
        bincode::serialize_into(writer, &data)?;

        Ok(())
    }


    fn storage_path(&self) -> String {
        self.storage_path.to_string_lossy().to_string()
    }

    fn store_type(&self) -> String {
        "HNSW (Approximate)".to_string()
    }

    fn get_stats(&self) -> StoreStats {
        let documents = self.documents.read().unwrap();
        
        let mut docs_by_type: HashMap<String, usize> = HashMap::new();
        let mut total_content_bytes: usize = 0;
        let mut total_embedding_dims: usize = 0;
        
        for doc in documents.values() {
            total_content_bytes += doc.content.len();
            total_embedding_dims = doc.embedding.len();
            
            let doc_type = doc.metadata.get("type").cloned().unwrap_or_else(|| "unknown".to_string());
            *docs_by_type.entry(doc_type).or_insert(0) += 1;
        }
        
        let file_size_bytes = std::fs::metadata(self.storage_path.with_extension("hnsw"))
            .map(|m| m.len())
            .unwrap_or(0) + 
            std::fs::metadata(self.storage_path.with_extension("data"))
            .map(|m| m.len())
            .unwrap_or(0);
            
        StoreStats {
            document_count: documents.len(),
            docs_by_type,
            total_content_bytes,
            embedding_dimensions: total_embedding_dims,
            file_size_bytes,
        }
    }
}
