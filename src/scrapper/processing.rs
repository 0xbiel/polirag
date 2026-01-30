
/// Normalize text extracted from PDFs - fix ligatures and other Unicode issues
fn normalize_text(text: &str) -> String {
    text
        // Common ligatures
        .replace('\u{FB00}', "ff")   // ﬀ
        .replace('\u{FB01}', "fi")   // ﬁ
        .replace('\u{FB02}', "fl")   // ﬂ
        .replace('\u{FB03}', "ffi")  // ﬃ
        .replace('\u{FB04}', "ffl")  // ﬄ
        .replace('\u{FB05}', "st")   // ﬅ (long s + t)
        .replace('\u{FB06}', "st")   // ﬆ
        // Additional ligatures
        .replace('\u{0132}', "IJ")   // Ĳ
        .replace('\u{0133}', "ij")   // ĳ
        .replace('\u{0152}', "OE")   // Œ
        .replace('\u{0153}', "oe")   // œ
        .replace('\u{00C6}', "AE")   // Æ
        .replace('\u{00E6}', "ae")   // æ
        // Common symbols
        .replace('\u{2019}', "'")    // '
        .replace('\u{2018}', "'")    // '
        .replace('\u{201C}', "\"")   // "
        .replace('\u{201D}', "\"")   // "
        .replace('\u{2013}', "-")    // – (en dash)
        .replace('\u{2014}', "-")    // — (em dash)
        .replace('\u{2026}', "...")  // …
        .replace('\u{00A0}', " ")    // Non-breaking space
        // Normalize whitespace
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn process_resources(subject_path: &std::path::Path) -> anyhow::Result<Vec<(String, String)>> {
    use std::fs;
    let mut extracted_docs = Vec::new();
    let resources_path = subject_path.join("resources");
    let extracted_path = resources_path.join("extracted");
    
    if !resources_path.exists() {
        return Ok(extracted_docs);
    }

    // 1. Unzip Logic
    if let Ok(entries) = fs::read_dir(&resources_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "zip") {
                tracing::info!("Found zip: {:?}. Extracting...", path.file_name());
                let file = fs::File::open(&path)?;
                let mut archive = zip::ZipArchive::new(file)?;
                
                let zip_name = path.file_stem().unwrap_or_default().to_string_lossy();
                let target_dir = extracted_path.join(zip_name.as_ref());
                
                for i in 0..archive.len() {
                    let mut file = archive.by_index(i)?;
                    // Sanitize path (avoid ../)
                    let outpath = match file.enclosed_name() {
                        Some(path) => target_dir.join(path),
                        None => continue,
                    };

                    if file.name().ends_with('/') {
                        fs::create_dir_all(&outpath)?;
                    } else {
                        if let Some(p) = outpath.parent() {
                            if !p.exists() { fs::create_dir_all(p)?; }
                        }
                        let mut outfile = fs::File::create(&outpath)?;
                        std::io::copy(&mut file, &mut outfile)?;
                    }
                }
            }
        }
    }

    // 2. PDF Extraction Logic
    use std::process::Command;
    
    // Scan both resources/ and resources/extracted/
    let dirs_to_scan = vec![resources_path.clone(), extracted_path];
    
    let exe = std::env::current_exe()?;
    let exe_path = exe.to_string_lossy();
    
    for dir in dirs_to_scan {
        if !dir.exists() { continue; }
        for entry in walkdir::WalkDir::new(&dir).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "pdf") {
                 tracing::info!("Processing PDF: {:?}", path.file_name());
                 
                 // Run subprocess to isolate noise
                 let output = Command::new(&*exe_path)
                     .arg("extract-pdf")
                     .arg(path.to_string_lossy().as_ref())
                     .output();
                     
                 match output {
                     Ok(out) => {
                         if out.status.success() {
                             let stdout = String::from_utf8_lossy(&out.stdout);
                             if let Some(start) = stdout.find("<<<START_CONTENT>>>") {
                                 if let Some(end) = stdout.find("<<<END_CONTENT>>>") {
                                     let text = &stdout[start + 19..end];
                                     let normalized = normalize_text(text);
                                     if !normalized.trim().is_empty() {
                                         let _name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                                         let rel_path = path.strip_prefix(subject_path).unwrap_or(path).to_string_lossy().to_string();
                                         extracted_docs.push((rel_path, normalized));
                                     }
                                 }
                             }
                         } else {
                             let stderr = String::from_utf8_lossy(&out.stderr);
                             tracing::warn!("PDF extraction failed for {:?}: {}", path, stderr);
                         }
                     },
                     Err(e) => tracing::error!("Failed to spawn extraction subprocess: {}", e),
                 }
            }
        }
    }

    Ok(extracted_docs)
}
