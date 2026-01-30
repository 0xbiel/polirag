use std::sync::Arc;
use crate::{rag, scrapper, config};

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
    
    // Clear old RAG data before syncing
    tracing::info!("Clearing old RAG index...");
    rag.clear()?;
    
    // Clear old scraped data logic using global path
    let data_dir = config::Config::get_scraped_data_dir();
    
    if data_dir.exists() {
        tracing::info!("Removing old data directory: {:?}", data_dir);
        if let Err(e) = std::fs::remove_dir_all(&data_dir) {
            tracing::warn!("Failed to remove data directory: {}", e);
        }
    }
    // Recreate it to ensure it exists for writing (though poliformat client might do it, 
    // passing the path to scrape_subject_content is better)
    // Actually `scrape_subject_content` probably assumes "data" relative path or needs to be updated?
    // Let's check `scrape_subject_content` in `scrapper/mod.rs` later. 
    // For now assuming we pass the path or it uses a default? 
    // In original main.rs, it didn't pass path to `scrape_subject_content`.
    // Let's check `scrapper::PoliformatClient::scrape_subject_content`. 
    // Use `view_code_item`? 
    // I'll assume for now `scrape_subject_content` writes to "data". I might need to update that too.
    
    // 1. Fetch Subjects
    tracing::info!("Fetching subjects...");
    let subjects = poliformat.get_subjects().await?;
    tracing::info!("Found {} subjects. Starting content scrape...", subjects.len());
    
    // 2. Fetch Deep Content
    // We need to tell scrapper where to save. If `scrape_subject_content` hardcodes "data", we need to change it.
    // I will check scrapper code next.
    let detailed_subjects = poliformat.scrape_subject_content(subjects).await?;
    
    for (sub, dir_path) in detailed_subjects {
        tracing::info!("Indexing subject: {} (Path: {})", sub.name, dir_path);
        
        let summary_path = std::path::Path::new(&dir_path).join("summary.md");
        let mut content = if summary_path.exists() {
             std::fs::read_to_string(&summary_path).unwrap_or_default()
        } else {
             // Maybe it failed to write?
             tracing::warn!("No summary.md found for {}", sub.name);
             continue; 
        };
        
        // Append list of found resources?
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
        let extracted_docs = match scrapper::processing::process_resources(std::path::Path::new(&dir_path)) {
            Ok(d) => d,
            Err(e) => {
                tracing::error!("Error processing resources for {}: {}", sub.name, e);
                Vec::new()
            }
        };
        
        let full_text = format!("Subject: {}\nURL: {}\n\n{}", sub.name, sub.url, content);
        
        // Add Summary Doc
        rag.add_document(
            &sub.id,
            &full_text,
            "user",
            [
                ("type".to_string(), "subject".to_string()),
                ("name".to_string(), sub.name.clone())
            ].into()
        ).await?;
        
        // Add PDF Docs
        for (rel_path, text) in extracted_docs {
            tracing::info!("Indexing PDF: {} (Length: {})", rel_path, text.len());
            let doc_id = format!("{}/{}", sub.id, rel_path);
            let pdf_text = format!("Subject: {}\nFile: {}\n\n{}", sub.name, rel_path, text);
             rag.add_document(
                &doc_id,
                &pdf_text,
                "user",
                [("type".to_string(), "pdf".to_string()), ("filename".to_string(), rel_path)].into()
            ).await?;
        }
    }
    
    tracing::info!("Sync Complete.");
    Ok(())
}

