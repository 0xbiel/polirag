use std::sync::Arc;
use crate::{rag, scrapper, config};
use text_splitter::TextSplitter;


pub async fn run_sync(rag: Arc<rag::RagSystem>, poliformat: Arc<scrapper::PoliformatClient>) -> anyhow::Result<()> {
    tracing::info!("Starting Sync...");

    // Check connection first
    if !poliformat.check_connection().await.unwrap_or(false) {
        tracing::warn!("Not authenticated. Checking for credentials...");
        
        // First, try cached credentials from config
        let cached_creds = config::Config::get_credentials().map(|c| (c.username, c.pin));
        
        // Then try environment variables
        let env_creds = {
            let username = std::env::var("POLIFORMAT_USER").or_else(|_| std::env::var("POLIFORMAT_DNI"));
            let pin = std::env::var("POLIFORMAT_PIN").or_else(|_| std::env::var("POLIFORMAT_PASSWORD"));
            if let (Ok(u), Ok(p)) = (username, pin) {
                Some((u, p))
            } else {
                None
            }
        };
        
        // Prefer cached credentials, fallback to env
        if let Some((u, p)) = cached_creds.or(env_creds) {
            tracing::info!("Credentials found. Attempting automatic login for user: {}", u);
            let creds = scrapper::auth::AuthCredentials {
                username: u.clone(),
                pin: p.clone(),
            };
            
            // Perform login in blocking task since headless_chrome is sync
            let client = poliformat.clone();
            match tokio::task::spawn_blocking(move || {
                client.login_headless(&creds)
            }).await? {
                Ok(_) => {
                    tracing::info!("Login successful!");
                    // Save credentials to config for future use
                    if let Err(e) = config::Config::save_credentials(&u, &p) {
                        tracing::warn!("Failed to cache credentials: {}", e);
                    }
                },
                Err(e) => {
                    tracing::error!("Auto-login failed: {}", e);
                    // Clear bad cached credentials
                    let _ = config::Config::clear_credentials();
                    anyhow::bail!("Login failed. Please login via the Menu first.");
                }
            }
        } else {
             tracing::error!("No credentials found in config or .env.");
             tracing::warn!("Please login via the Menu first.");
             anyhow::bail!("No credentials available. Please login via the Menu first.");
        }
    }
    
    // Clear old RAG data before syncing - NO! We want incremental sync now.
    // tracing::info!("Clearing old RAG index...");
    // rag.clear()?;
    
    // Clear old scraped data logic using global path - NO! We want to keep it.
    let data_dir = config::Config::get_scraped_data_dir();
    
    if !data_dir.exists() {
        tracing::info!("Creating data directory: {:?}", data_dir);
        std::fs::create_dir_all(&data_dir)?;
    }
    
    // 1. Fetch Subjects
    tracing::info!("Fetching subjects...");
    let subjects = poliformat.get_subjects().await?;
    tracing::info!("Found {} subjects. Starting content scrape...", subjects.len());
    
    // 2. Fetch Deep Content
    let detailed_subjects = poliformat.scrape_subject_content(subjects).await?;
    
    for (sub, dir_path) in detailed_subjects {
        tracing::info!("Indexing subject: {} (Path: {})", sub.name, dir_path);
        
        let summary_path = std::path::Path::new(&dir_path).join("summary.md");
        let mut content = if summary_path.exists() {
             std::fs::read_to_string(&summary_path).unwrap_or_default()
        } else {
             tracing::warn!("No summary.md found for {}", sub.name);
             continue; 
        };
        
        // Append list of found resources
        let resources_path = std::path::Path::new(&dir_path).join("resources");
        if resources_path.exists() {
             use std::fmt::Write;
             let mut file_list = String::new();
             writeln!(&mut file_list, "\n\n[Local Files]:").unwrap();
             if let Ok(entries) = std::fs::read_dir(&resources_path) {
                 for entry in entries.flatten() {
                      if let Ok(name) = entry.file_name().into_string() {
                          writeln!(&mut file_list, "- {}", name).unwrap();
                      }
                 }
             }
             content.push_str(&file_list);
        }
        
        // --- Process Resources (Unzip & PDF Extract) ---
        tracing::info!("Processing resources for {}...", sub.name);
        
        // Only process resources if we haven't indexed them yet? 
        // Not trivial to know, but we can check if documents exist in RAG.
        // But processing resources is cheap if PDFs are already extracted.
        // See: scrapper::processing::process_resources.
        // For now, let's run processing, it usually just scans PDFs.
        
        let extracted_docs = match scrapper::processing::process_resources(std::path::Path::new(&dir_path)) {
            Ok(d) => d,
            Err(e) => {
                tracing::error!("Error processing resources for {}: {}", sub.name, e);
                Vec::new()
            }
        };
        
        let full_text = format!("Subject: {}\nURL: {}\n\n{}", sub.name, sub.url, content);
        
        // Add Summary Doc
        if !rag.contains(&sub.id) {
            tracing::info!("Adding NEW subject summary: {}", sub.name);
            rag.add_document(
                &sub.id,
                &full_text,
                "user",
                [
                    ("type".to_string(), "subject".to_string()),
                    ("name".to_string(), sub.name.clone())
                ].into()
            ).await?;
        } else {
            tracing::debug!("Skipping existing subject summary: {}", sub.name);
        }
        
        // Add PDF Docs
        for (rel_path, text) in extracted_docs {
            let doc_id = format!("{}/{}", sub.id, rel_path);
            
            // Chunking Strategy
            let chunk_0_id = format!("{}#0", doc_id);
            
            if !rag.contains(&chunk_0_id) {
                tracing::info!("Indexing NEW PDF (chunked): {} (Length: {})", rel_path, text.len());
                
                let splitter = TextSplitter::new(1000);
                let chunks: Vec<&str> = splitter.chunks(&text).collect();
                
                let filename = std::path::Path::new(&rel_path).file_name().and_then(|n| n.to_str()).unwrap_or(&rel_path);
                
                if chunks.is_empty() {
                    let pdf_text = format!("### DOC: {}\nSubject: {}\n\n{}", filename, sub.name, text);
                    let final_id = format!("{}#0", doc_id);
                    rag.add_document(
                        &final_id,
                        &pdf_text,
                        "user",
                        [("type".to_string(), "pdf".to_string()), ("filename".to_string(), rel_path.clone())].into()
                    ).await?;
                } else {
                    for (i, chunk) in chunks.iter().enumerate() {
                        let chunk_id = format!("{}#{}", doc_id, i);
                        let pdf_text = format!("### DOC: {} (Part {}/{})\nCourse: {}\n\n{}", filename, i+1, chunks.len(), sub.name, chunk);
                        
                         rag.add_document(
                            &chunk_id,
                            &pdf_text,
                            "user",
                            [("type".to_string(), "pdf".to_string()), ("filename".to_string(), rel_path.clone())].into()
                        ).await?;
                    }
                }
            } else {
                tracing::debug!("Skipping existing PDF: {}", rel_path);
            }
        }
        
        // Save intermittently (good for large scrapes)
        let _ = rag.save();
    }
    
    tracing::info!("Saving RAG index...");
    rag.save()?;
    
    tracing::info!("Sync Complete.");
    Ok(())
}


// Similar to run_sync but only scans local files, no network
pub async fn scan_local_data(rag: Arc<rag::RagSystem>, log_callback: impl Fn(String)) -> anyhow::Result<Vec<String>> {
    log_callback("🔍 Scanning local data directory...".to_string());
    
    let data_dir = config::Config::get_scraped_data_dir();
    if !data_dir.exists() {
        log_callback("⚠️  Data directory not found.".to_string());
        return Ok(Vec::new());
    }
    
    let mut added_ids = Vec::new();
    
    // Iterate over subject directories
    let entries = std::fs::read_dir(&data_dir)?;
    for entry in entries.flatten() {
        if !entry.path().is_dir() { continue; }
        
        let path = entry.path();
        let dir_name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        
        // Skip hidden folders
        if dir_name.starts_with('.') { continue; }
        
        log_callback(format!("Checking subject: {}", dir_name));
        
        // 1. Process Resources
        let extracted_docs = match scrapper::processing::process_resources(&path) {
            Ok(d) => d,
            Err(e) => {
                tracing::error!("Error processing resources for {}: {}", dir_name, e);
                Vec::new()
            }
        };
        
        // 2. Index PDFs
        for (rel_path, text) in extracted_docs {
            let summary_path = path.join("summary.md");
            let subject_id = if summary_path.exists() {
                let content = std::fs::read_to_string(&summary_path).unwrap_or_default();
                if let Some(url_line) = content.lines().find(|l| l.starts_with("URL:")) {
                    // Extract ID from URL
                    // URL: https://poliformat.upv.es/portal/site/GRA_11673_2025
                    // ID is GRA_11673_2025
                    if let Some(pos) = url_line.rfind('/') {
                         url_line[pos+1..].trim().to_string()
                    } else {
                        dir_name.clone()
                    }
                } else {
                    dir_name.clone()
                }
            } else {
                dir_name.clone()
            };
            
            let doc_id = format!("{}/{}", subject_id, rel_path);
            
            // Chunking Strategy:
            // Check if chunk 0 exists to determine if we need to index
            let chunk_0_id = format!("{}#0", doc_id);
            
            if !rag.contains(&chunk_0_id) {
                // Check if an OLD unchunked version exists and remove it
                if rag.contains(&doc_id) {
                    let _ = rag.remove_document(&doc_id);
                    log_callback(format!("  🗑️  Removing old unchunked entry for: {}", rel_path));
                }

                log_callback(format!("  ➕ Indexing new file (chunked): {}/{}", dir_name, rel_path));
                
                let splitter = TextSplitter::new(1000);
                let chunks: Vec<&str> = splitter.chunks(&text).collect();
                
                let filename = std::path::Path::new(&rel_path).file_name().and_then(|n| n.to_str()).unwrap_or(&rel_path);
                
                if chunks.is_empty() {
                    let pdf_text = format!("### DOC: {}\nSubject: {}\n\n{}", filename, dir_name, text);
                    let final_id = format!("{}#0", doc_id); 
                    rag.add_document(
                        &final_id,
                        &pdf_text,
                        "user",
                        [("type".to_string(), "pdf".to_string()), ("filename".to_string(), rel_path)].into()
                    ).await?;
                    added_ids.push(final_id);
                } else {
                    for (i, chunk) in chunks.iter().enumerate() {
                        let chunk_id = format!("{}#{}", doc_id, i);
                        let pdf_text = format!("### DOC: {} (Part {}/{})\nCourse: {}\n\n{}", filename, i+1, chunks.len(), dir_name, chunk);
                        
                        rag.add_document(
                           &chunk_id,
                           &pdf_text,
                           "user",
                           [("type".to_string(), "pdf".to_string()), ("filename".to_string(), rel_path.clone())].into()
                       ).await?;
                       added_ids.push(chunk_id);
                    }
                }
            }
        }
    }
    
    if !added_ids.is_empty() {
        rag.save()?;
    }
    
    Ok(added_ids)
}
