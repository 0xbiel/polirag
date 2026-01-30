pub mod auth;
pub mod processing;

use reqwest_cookie_store::CookieStoreMutex;
use reqwest::Client;
use std::sync::Arc;
use url::Url;

pub struct PoliformatClient {
    client: Client,
    cookie_store: Arc<CookieStoreMutex>,
    base_url: Url,
}

impl PoliformatClient {
    pub fn new() -> Self {
        let cookie_store = Arc::new(CookieStoreMutex::new(cookie_store::CookieStore::default()));

        let client = Client::builder()
            .cookie_provider(cookie_store.clone())
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .timeout(std::time::Duration::from_secs(10)) 
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .expect("Failed to build reqwest client");
        
        Self { client, cookie_store, base_url: Url::parse("https://poliformat.upv.es").unwrap() }
    }
    
    pub fn login_headless(&self, creds: &auth::AuthCredentials) -> anyhow::Result<()> {
        let cookie_str = auth::headless_login(creds)?;
        self.import_cookies(&cookie_str);
        tracing::info!("Cookies imported. Testing connection...");
        std::thread::sleep(std::time::Duration::from_millis(2000));
        Ok(())
    }

    pub fn import_cookies(&self, cookie_string: &str) {
        let mut store = self.cookie_store.lock().unwrap();
        let base_url = &self.base_url;
        for pair in cookie_string.split(';') {
            let pair = pair.trim();
            if let Some((k, v)) = pair.split_once('=') {
                 let c = cookie::Cookie::build((k, v)).domain("poliformat.upv.es").path("/").secure(true).build();
                 let _ = store.parse(&c.to_string(), base_url);
                 let c2 = cookie::Cookie::build((k, v)).domain("upv.es").path("/").secure(true).build();
                 let _ = store.parse(&c2.to_string(), base_url);
            }
        }
    }
    
    pub async fn check_connection(&self) -> anyhow::Result<bool> {
        let resp = tokio::time::timeout(std::time::Duration::from_secs(5), self.client.get(self.base_url.clone()).send()).await??;
        let url = resp.url().as_str();
        let is_login = url.contains("login") || url.contains("est_aute") || url.contains("gateway");
        Ok(!is_login)
    }

    pub async fn get_subjects(&self) -> anyhow::Result<Vec<Subject>> {
        tracing::info!("Starting Browser-based Subject Extraction...");
        let subjects = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<Subject>> {
            use headless_chrome::{Browser, LaunchOptions};
            let options = LaunchOptions { headless: true, window_size: Some((1280, 800)), idle_browser_timeout: std::time::Duration::from_secs(180), ..Default::default() };
            let browser = Browser::new(options)?;
            let tab = browser.new_tab()?;
            tab.set_default_timeout(std::time::Duration::from_secs(60));
            tab.navigate_to("https://poliformat.upv.es/portal")?;
            std::thread::sleep(std::time::Duration::from_secs(2));
            
            // Initial Login Logic (Shared)
            // Robust Login Logic
            std::thread::sleep(std::time::Duration::from_secs(4));
            let current_url = tab.get_url();
            let body_text = tab.evaluate("document.body.innerText", true).ok().and_then(|r| r.value).and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_default();
            tracing::info!("DEBUG: get_subjects URL: {}", current_url);
            tracing::info!("DEBUG: Body text len: {}", body_text.len());
            
            if current_url.contains("login") || current_url.contains("gateway") || current_url.contains("xlogin") || body_text.contains("Identificación obligatoria") || body_text.contains("Identificarse") {
                 tracing::info!("DEBUG: Login required. Starting login flow...");
                 // Try env vars first, then fall back to cached credentials
                 let env_username = std::env::var("POLIFORMAT_USER").or_else(|_| std::env::var("POLIFORMAT_DNI"));
                 let env_pin = std::env::var("POLIFORMAT_PIN").or_else(|_| std::env::var("POLIFORMAT_PASSWORD"));
                 let creds = match (env_username, env_pin) {
                     (Ok(u), Ok(p)) => Some((u, p)),
                     _ => crate::config::Config::get_credentials().map(|c| (c.username, c.pin)),
                 };
                 if let Some((u, p)) = creds {
                     // Explicitly navigate to login page to avoid button/link issues
                     tracing::info!("DEBUG: Navigating to portable/login...");
                     if let Err(e) = tab.navigate_to("https://poliformat.upv.es/portal/login") {
                         tracing::warn!("DEBUG: Failed to navigate to login: {}", e);
                     }
                     std::thread::sleep(std::time::Duration::from_secs(5));

                     // Input Retry Loop
                     let mut inputs_found = false;
                     let start_wait = std::time::Instant::now();
                     tracing::info!("DEBUG: Waiting for inputs...");
                     while start_wait.elapsed().as_secs() < 20 {
                         if tab.find_element("#username, input[name='dni'], input[name='username']").is_ok() { 
                             inputs_found = true; 
                             tracing::info!("DEBUG: Inputs found!");
                             break; 
                         }
                         std::thread::sleep(std::time::Duration::from_millis(500));
                     }

                     if inputs_found {
                         if let Ok(el) = tab.find_element("#username, input[name='dni'], input[name='username']") { let _ = el.type_into(&u); }
                         if let Ok(el) = tab.find_element("#password, input[name='clau'], input[name='password']") { let _ = el.type_into(&p); }
                         if let Ok(el) = tab.find_element(".btn-submit, input[type='submit'], button[type='submit'], button[name='submit']") { 
                             let _ = el.click(); 
                             tracing::info!("DEBUG: Submitted credentials.");
                         }
                         let _ = tab.wait_for_element_with_custom_timeout("#toolMenu", std::time::Duration::from_secs(20));
                     } else {
                         tracing::warn!("DEBUG: Inputs NOT found after 20s.");
                     }
                 }
            } else {
                 tracing::info!("DEBUG: No login requirement detected.");
            }

            if let Ok(btn) = tab.find_element("#sakai-view-all-sites") { 
                tracing::info!("DEBUG: Found #sakai-view-all-sites. Clicking...");
                let _ = btn.click(); 
                std::thread::sleep(std::time::Duration::from_secs(4)); 
            } else {
                tracing::warn!("DEBUG: #sakai-view-all-sites NOT found!");
            }

            let js_script = r#"
                (function() {
                    let subjects = [];
                    let links = Array.from(document.querySelectorAll('a[href*="/portal/site/"]:not([href*="!gateway"])'));
                    let seen = new Set();
                    links.forEach(a => {
                        let href = a.href;
                        if (!href || seen.has(href) || href.includes("/tool/") || href.includes("~")) return;
                        let text = (a.innerText || a.title || "").trim();
                        if (!text || ["Home", "Inici", "Castellano", "English", "Valencià"].includes(text)) return;
                        seen.add(href);
                        subjects.push({ id: href, name: text, url: href });
                    });
                    return JSON.stringify(subjects);
                })()
            "#;
            let remote_object = tab.evaluate(js_script, true)?;
            let raw_json = remote_object.value.unwrap_or(serde_json::json!([]));
            let raw_subjects: Vec<Subject> = serde_json::from_str(raw_json.as_str().unwrap_or("[]")).unwrap_or_default();
            Ok(raw_subjects)
        }).await??;
        
        let mut unique_subjects = subjects;
        unique_subjects.sort_by(|a, b| a.name.cmp(&b.name));
        unique_subjects.dedup_by(|a, b| a.id == b.id);
        tracing::info!("Found {} unique subjects", unique_subjects.len());
        Ok(unique_subjects)
    }

    pub async fn scrape_subject_content(&self, subjects: Vec<Subject>) -> anyhow::Result<Vec<(Subject, String)>> {
        tracing::info!("Starting Parallel Content Extraction for {} subjects...", subjects.len());
        
        // Get cached credentials
        let cached_creds = crate::config::Config::get_credentials();
        let env_creds = {
            let u = std::env::var("POLIFORMAT_USER").or_else(|_| std::env::var("POLIFORMAT_DNI")).ok();
            let p = std::env::var("POLIFORMAT_PIN").or_else(|_| std::env::var("POLIFORMAT_PASSWORD")).ok();
            match (u, p) {
                (Some(u), Some(p)) => Some((u, p)),
                _ => None,
            }
        };
        let creds = cached_creds.map(|c| (c.username, c.pin)).or(env_creds);

        let results = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<(Subject, String)>> {
            use headless_chrome::{Browser, LaunchOptions};
            use std::sync::{Arc, Mutex};
            
            // Launch a single browser instance
            tracing::info!("Launching browser for parallel scraping...");
            let browser = Browser::new(LaunchOptions { 
                headless: true, 
                window_size: Some((1280, 800)), 
                idle_browser_timeout: std::time::Duration::from_secs(600), // 10 min timeout
                ..Default::default() 
            })?;
            let browser = Arc::new(browser);
            
            let results: Arc<Mutex<Vec<(Subject, String)>>> = Arc::new(Mutex::new(Vec::new()));
            let total = subjects.len();
            
            // Process subjects SEQUENTIALLY because Chrome's SetDownloadBehavior is browser-wide
            // Parallel downloads would cause files to go to wrong directories
            tracing::info!("Processing {} subjects sequentially (downloads require exclusive access)...", total);
            
            for (idx, sub) in subjects.into_iter().enumerate() {
                tracing::info!("Progress: [{}/{}] Processing: {}", idx + 1, total, sub.name);
                
                match scrape_single_subject(&browser, &sub, creds.as_ref()) {
                    Ok(path) => {
                        results.lock().unwrap().push((sub, path));
                    }
                    Err(e) => {
                        tracing::error!("Error scraping {}: {:?}", sub.name, e);
                    }
                }
            }
            
            let final_results = match Arc::try_unwrap(results) {
                Ok(mutex) => mutex.into_inner().unwrap(),
                Err(arc) => arc.lock().unwrap().clone(),
            };
            
            Ok(final_results)
        }).await??;
        
        Ok(results)
    }
}

/// Scrapes a single subject using a new tab from the shared browser
fn scrape_single_subject(
    browser: &std::sync::Arc<headless_chrome::Browser>,
    sub: &Subject,
    creds: Option<&(String, String)>,
) -> anyhow::Result<String> {
    use headless_chrome::protocol::cdp::Browser as BrowserProtocol;
    
    let tab = browser.new_tab()?;
    tab.set_default_timeout(std::time::Duration::from_secs(60));
    
    // Create data directory for this subject
    let clean_name = sub.name.replace("/", "-").replace(":", "").trim().to_string();
    let base_path = crate::config::Config::get_scraped_data_dir().join(&clean_name);
    std::fs::create_dir_all(&base_path)?;
    
    // Final destination for resources - use absolute path
    let final_download_path = base_path.join("resources");
    std::fs::create_dir_all(&final_download_path)?;
    let download_path_str = std::fs::canonicalize(&final_download_path)?
        .to_string_lossy()
        .to_string();
    
    // Use Browser.setDownloadBehavior (not the deprecated Page version)
    // This properly sets the download directory for the browser context
    let _ = tab.call_method(BrowserProtocol::SetDownloadBehavior { 
        behavior: BrowserProtocol::SetDownloadBehaviorBehaviorOption::Allow, 
        browser_context_id: None,
        download_path: Some(download_path_str.clone()),
        events_enabled: Some(false),
    });

    // Navigate to subject
    if tab.navigate_to(&sub.url).is_err() { 
        let _ = tab.close(true);
        return Ok("Navigation Failed".to_string()); 
    }
    
    // Check Session
    std::thread::sleep(std::time::Duration::from_secs(2)); 
    let curr_url = tab.get_url();
    let body_text = tab.evaluate("document.body.innerText", true)
        .ok()
        .and_then(|r| r.value)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_default();

    if curr_url.contains("login") || curr_url.contains("gateway") || curr_url.contains("xlogin") 
        || body_text.contains("Identificación obligatoria") || body_text.contains("Identificarse") {
        
        if let Some((u, p)) = creds {
            tracing::info!("Session expired for {}. Re-authenticating...", sub.name);
            if let Err(e) = tab.navigate_to("https://poliformat.upv.es/portal/login") {
                tracing::warn!("Failed to navigate to login: {}", e);
            }
            std::thread::sleep(std::time::Duration::from_secs(3));

            // Wait for inputs
            let start_wait = std::time::Instant::now();
            while start_wait.elapsed().as_secs() < 15 {
                if tab.find_element("#username, input[name='dni'], input[name='username']").is_ok() { 
                    break; 
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            
            if let Ok(el) = tab.find_element("#username, input[name='dni'], input[name='username']") { 
                let _ = el.type_into(u); 
            }
            if let Ok(el) = tab.find_element("#password, input[name='clau'], input[name='password']") { 
                let _ = el.type_into(p); 
            }
            if let Ok(el) = tab.find_element(".btn-submit, input[type='submit'], button[type='submit']") { 
                let _ = el.click(); 
            }
            let _ = tab.wait_for_element_with_custom_timeout("#toolMenu, .Mrphs-toolsNav", std::time::Duration::from_secs(20));
            
            // Re-navigate to subject
            let _ = tab.navigate_to(&sub.url);
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    }

    let mut content_accumulator = String::new();
    let _ = tab.wait_for_element_with_custom_timeout("#toolMenu", std::time::Duration::from_secs(10));
    std::thread::sleep(std::time::Duration::from_secs(2));
    
    // Get dashboard content
    if let Ok(ro) = tab.evaluate("document.body.innerText", true) {
        if let Some(val) = ro.value {
            let s = val.as_str().unwrap_or("");
            content_accumulator.push_str(&format!("--- DASHBOARD ---\n{}\n", if s.len() > 3000 { &s[0..3000] } else { s }));
        }
    }

    // Tools extraction
    let tool_links_script = r#"
        (function() {
            let result = {};
            let container = document.querySelector('#toolMenu') || document;
            let links = Array.from(container.querySelectorAll('a'));
            links.forEach(l => {
                let t = (l.innerText || l.title || "").toLowerCase();
                let href = l.href;
                let currentSite = window.location.pathname.match(/\/site\/([^\/]+)/);
                let linkSite = href.match(/\/site\/([^\/]+)/);
                if (currentSite && linkSite && currentSite[1] !== linkSite[1]) return;
                
                if (t.includes('anuncis') || t.includes('avisos') || t.includes('announcements')) result['announcements'] = href;
                if (t.includes('lliçons') || t.includes('lecciones') || t.includes('lessonbuilder') || t.includes('contenidos')) result['lessons'] = href;
                if (t.includes('recursos') || t.includes('resources')) result['resources'] = href;
                if (t.includes('guia') || l.querySelector('.si-es-upv-webasipublic')) result['guiaDocent'] = href;
            });
            return JSON.stringify(result);
        })()
    "#;
    
    if let Ok(ro) = tab.evaluate(tool_links_script, true) {
        if let Some(val) = ro.value {
            let map: serde_json::Value = serde_json::from_str(val.as_str().unwrap_or("{}")).unwrap_or_default();
            
            if let Some(href) = map.get("announcements").and_then(|h| h.as_str()) {
                let _ = tab.navigate_to(href);
                std::thread::sleep(std::time::Duration::from_secs(3));
                if let Ok(ro_a) = tab.evaluate("document.querySelector('.portletBody') ? document.querySelector('.portletBody').innerText : document.body.innerText", true) {
                    let content = ro_a.value.and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_default();
                    content_accumulator.push_str(&format!("\n--- ANUNCIS ---\n{}\n", content));
                }
            }

            if let Some(href) = map.get("lessons").and_then(|h| h.as_str()) {
                let _ = tab.navigate_to(href);
                std::thread::sleep(std::time::Duration::from_secs(3));
                if let Ok(ro_l) = tab.evaluate("document.body.innerText", true) {
                    let content = ro_l.value.and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_default();
                    content_accumulator.push_str(&format!("\n--- LLIÇONS ---\n{}\n", content));
                }
            }

            if let Some(href) = map.get("resources").and_then(|h| h.as_str()) {
                let _ = tab.navigate_to(href);
                std::thread::sleep(std::time::Duration::from_secs(3));
                let _ = tab.evaluate("document.getElementById('selectall') ? document.getElementById('selectall').click() : null", true);
                std::thread::sleep(std::time::Duration::from_millis(500));
                let _ = tab.evaluate("document.getElementById('zipdownload-button') ? document.getElementById('zipdownload-button').click() : null", true);
                std::thread::sleep(std::time::Duration::from_secs(2));
                let _ = tab.evaluate("document.getElementById('zipDownloadButton') ? document.getElementById('zipDownloadButton').click() : null", true);
                
                // Wait for downloads to complete
                wait_for_downloads(&final_download_path, &sub.name);
            }

            // Scrape Guia Docent (Teaching Guide / Syllabus PDF)
            if let Some(href) = map.get("guiaDocent").and_then(|h| h.as_str()) {
                tracing::info!("Scraping Guia Docent for {}", sub.name);
                let _ = tab.navigate_to(href);
                std::thread::sleep(std::time::Duration::from_secs(4));
                
                // Extract page content - the guia docent shows in an iframe or div
                let guia_content_js = r#"
                    (function() {
                        // Try iframe first (common in Sakai)
                        let iframe = document.querySelector('iframe');
                        if (iframe && iframe.contentDocument) {
                            return iframe.contentDocument.body.innerText || '';
                        }
                        // Try main content areas
                        let content = document.querySelector('.portletBody, #content, main');
                        return content ? content.innerText : document.body.innerText;
                    })()
                "#;
                if let Ok(ro_g) = tab.evaluate(guia_content_js, true) {
                    let content = ro_g.value.and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_default();
                    if !content.is_empty() {
                        content_accumulator.push_str(&format!("\n--- GUIA DOCENT ---\n{}\n", content));
            // We use the "HTML View" + "Print to PDF" strategy by navigating directly to likely URL.
            tracing::info!("Scraping Guia Docent for {}", sub.name);
            
            // Extract numeric ID from subject ID (e.g. GRA_11673_2025_DTU -> 11673)
            let parts: Vec<&str> = sub.id.split('_').collect();
            let subject_id = if parts.len() >= 2 { parts[1] } else { "" };
            let subject_year = if parts.len() >= 3 { parts[2] } else { "2025" };

            if !subject_id.is_empty() {
                // https://www.upv.es/pls/soalu/sic_gdoc.get_content?P_ASI={ID}&P_IDIOMA=c&P_VISTA=poliformat&P_TIT=&P_CACA={YEAR}
                let guia_url = format!("https://www.upv.es/pls/soalu/sic_gdoc.get_content?P_ASI={}&P_IDIOMA=c&P_VISTA=poliformat&P_TIT=&P_CACA={}", subject_id, subject_year);
                tracing::info!("Navigating to Guia Docent HTML view: {}", guia_url);

                if let Ok(_) = tab.navigate_to(&guia_url) {
                     let _ = tab.wait_until_navigated();
                     std::thread::sleep(std::time::Duration::from_secs(3));
                     
                     // Print to PDF
                     // We use the headless_chrome generic print options
                     tracing::info!("Printing Guia Docent page to PDF...");
                     match tab.print_to_pdf(None) {
                         Ok(pdf_data) => {
                             let pdf_filename = format!("{} (Guia Docent).pdf", sub.name.replace("/", "-"));
                             let pdf_path = final_download_path.join(&pdf_filename);
                             if let Err(e) = std::fs::write(&pdf_path, pdf_data) {
                                 tracing::error!("Failed to write Guia Docent PDF: {}", e);
                             } else {
                                 tracing::info!("Saved Guia Docent PDF to {:?}", pdf_path);
                             }
                         },
                         Err(e) => {
                             tracing::error!("Failed to print PDF: {}", e);
                         }
                     }
                     
                     // Scrape Description
                     let desc_url = format!("https://www.upv.es/pls/soalu/sic_gdoc.get_content?P_ASI={}&P_IDIOMA=c&P_VISTA=poliformat&P_TIT=&P_CACA={}&P_CONTENT=descripcion", subject_id, subject_year);
                     tracing::info!("Scraping Guia Docent Description: {}", desc_url);
                     if let Ok(_) = tab.navigate_to(&desc_url) {
                        let _ = tab.wait_until_navigated();
                        std::thread::sleep(std::time::Duration::from_secs(2));
                        if let Ok(ro) = tab.evaluate("document.querySelector('#contenido') ? document.querySelector('#contenido').innerText : document.body.innerText", true) {
                            let content = ro.value.and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_default();
                             if !content.is_empty() {
                                 content_accumulator.push_str(&format!("\n--- GUIA DOCENT DESCRIPTION ---\n{}\n", content));
                             }
                        }
                     }

                     // Scrape Professors
                     let prof_url = format!("https://www.upv.es/pls/soalu/sic_asi.Profesores?P_OCW=&P_ASI={}&P_CACA={}&P_IDIOMA=c&P_VISTA=poliformat", subject_id, subject_year);
                     tracing::info!("Scraping Guia Docent Professors: {}", prof_url);
                      if let Ok(_) = tab.navigate_to(&prof_url) {
                        let _ = tab.wait_until_navigated();
                        std::thread::sleep(std::time::Duration::from_secs(2));
                        if let Ok(ro) = tab.evaluate("document.querySelector('#contenido') ? document.querySelector('#contenido').innerText : document.body.innerText", true) {
                            let content = ro.value.and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_default();
                             if !content.is_empty() {
                                 content_accumulator.push_str(&format!("\n--- PROFESSORS ---\n{}\n", content));
                             }
                        }
                     }
                } else {
                    tracing::warn!("Failed to navigate to Guia Docent URL");
                }
            } else {
                 tracing::warn!("Could not extract numeric ID from subject ID: {}", sub.id);
            }
            
            // End of Guia Docent logic (No wait_for_downloads needed as we write file directly)
        }
    }
    }
    }
    }
    
    // Write summary.md
    let summary_path = base_path.join("summary.md");
    if let Err(e) = std::fs::write(&summary_path, &content_accumulator) {
        tracing::error!("Failed to write summary.md for {}: {}", sub.name, e);
    }
    
    // Close the tab when done
    let _ = tab.close(true);
    
    Ok(base_path.to_string_lossy().to_string())
}

/// Wait for downloads to complete by checking for .crdownload / .tmp files
fn wait_for_downloads(download_path: &std::path::Path, subject_name: &str) {
    use std::time::{Duration, Instant};
    
    let max_wait = Duration::from_secs(120); // Wait up to 2 minutes for downloads
    let poll_interval = Duration::from_secs(2);
    let start = Instant::now();
    
    // Initial wait to let download start
    std::thread::sleep(Duration::from_secs(5));
    
    tracing::info!("Waiting for downloads to complete for {}...", subject_name);
    
    loop {
        if start.elapsed() > max_wait {
            tracing::warn!("Download timeout for {} - continuing anyway", subject_name);
            break;
        }
        
        // Check if any incomplete downloads exist
        let has_incomplete = if let Ok(entries) = std::fs::read_dir(download_path) {
            entries.filter_map(|e| e.ok()).any(|entry| {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                // Chrome uses .crdownload, some browsers use .tmp or .part
                name.ends_with(".crdownload") || name.ends_with(".tmp") || name.ends_with(".part")
            })
        } else {
            false
        };
        
        if !has_incomplete {
            // Check if any files exist at all (download may have started)
            let has_files = std::fs::read_dir(download_path)
                .map(|d| d.count() > 0)
                .unwrap_or(false);
                
            if has_files {
                tracing::info!("Downloads complete for {}", subject_name);
            }
            break;
        }
        
        tracing::debug!("Downloads still in progress for {}...", subject_name);
        std::thread::sleep(poll_interval);
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Subject { pub id: String, pub name: String, pub url: String }

