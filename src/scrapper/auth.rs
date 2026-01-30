use anyhow::{Context, Result};
use headless_chrome::{Browser, LaunchOptions};


pub struct AuthCredentials {
    pub username: String,
    pub pin: String,
}

// Helper function to perform headless login and extract the JSESSIONID or relevant cookies.
pub fn headless_login(creds: &AuthCredentials) -> Result<String> {
    tracing::info!("Starting headless login (Optimized)...");

    // Optimized Launch Options
    let options = LaunchOptions {
        headless: true,
        enable_logging: false, // Reduce noise
        window_size: Some((1280, 800)), 
        ..Default::default()
    };
    
    tracing::info!("Launching browser...");
    let browser = Browser::new(options).context("Failed to launch headless browser")?;
    let tab = browser.new_tab().context("Failed to open new tab")?;

    // 1. Navigate to Login
    tracing::info!("Navigating to Login Portal...");
    // Direct link to the Auth portal to skip redirects if possible.
    // However, the safest is still the main entry point.
    tab.navigate_to("https://poliformat.upv.es/portal/login")?;
    
    // 2. Race: Check for Button OR Input
    // We poll quickly
    let start = std::time::Instant::now();
    let mut found_input = false;
    
    tracing::info!("Waiting for interaction elements...");
    while start.elapsed().as_secs() < 15 { // Increased initial wait to 15s
        let current_url = tab.get_url();
        let current_title = tab.get_title().unwrap_or_default();
        tracing::debug!("DEBUG polling: URL={} Title={}", current_url, current_title);

        // Check for DNI Input (common in Poliformat) OR Username (CAS)
        if let Ok(_) = tab.find_element("input[name='dni']") {
            tracing::info!("FOUND: DNI Input field (PoliformaT style).");
            found_input = true;
            break;
        }
        
        if let Ok(_) = tab.find_element("input[name='username']") {
             tracing::info!("FOUND: Username Input field (CAS style).");
             found_input = true;
             break;
        }
        
        // Sometimes the input has id="username" but name is different, or typical CAS structure
        if let Ok(_) = tab.find_element("#username") {
             tracing::info!("FOUND: #username Input field.");
             found_input = true;
             break;
        }
        
        // Check for 'Identificarse' button
        // UPV Portal often has this button if not redirected
        if let Ok(btn) = tab.find_element("#loginLink1") {
            tracing::info!("FOUND: 'Identificarse' button. Clicking it to force login...");
            if let Err(e) = btn.click() {
                tracing::warn!("Failed to click Identificarse button: {}", e);
            }
            // After click, we loop again to wait for input
            std::thread::sleep(std::time::Duration::from_millis(1000));
            continue;
        }
        
        // Check for 'Entrar' button (sakai-login-tool)
        if let Ok(_btn) = tab.find_element("input[name='eventSubmit_doLogin']") {
             // This means we might be on a different type of login page (older Sakai)
             // But usually this goes with username inputs.
             // Just logging for now.
             tracing::debug!("Found Sakai login button (legacy?)");
        }

        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    if !found_input {
         // Debug: Take a screenshot to see where we are stuck
         tracing::error!("Timeout! Taking screenshot to 'debug_screenshot.png'...");
         tracing::error!("Timeout! (Screenshot skipped due to compilation error)");
    
         // Final check
         if tab.find_element("input[name='dni']").is_err() {
             anyhow::bail!("Timed out waiting for login form inputs. URL: {}", tab.get_url());
         }
    }

    tracing::info!("Form detected. Typing credentials...");
    
    // Type fast
    // Try to find the username input again using the same hierarchy
    let user_input = tab.find_element("input[name='dni']")
        .or_else(|_| tab.find_element("input[name='username']"))
        .or_else(|_| tab.find_element("#username"))
        .context("Lost username input field after detection")?;
        
    user_input.type_into(&creds.username)?;
    
    let pass_input = tab.find_element("input[name='clau']")
        .or_else(|_| tab.find_element("input[name='password']"))
        .or_else(|_| tab.find_element("#password"))
        .context("Could not find password/pin input field.")?;
        
    pass_input.type_into(&creds.pin)?;

    // Submit
    let submit = tab.find_element("input[type='submit']")
        .or_else(|_| tab.find_element("button[type='submit']"))
        .or_else(|_| tab.find_element(".btn-submit")) // Common in CAS
        .or_else(|_| tab.find_element("button[name='submit']"))?;
        
    tracing::info!("Submitting...");
    submit.click()?;

    // 4. Wait for redirection success
    tracing::info!("Waiting for authenticated session...");
    
    // Try multiple selectors that indicate successful login
    // The PoliformaT/Sakai UI may have changed over time
    let success_selectors = [
        "#toolMenu",           // Classic Sakai sidebar
        ".Mrphs-toolsNav",     // Morpheus theme navigation
        ".sakai-sitesAndToolsNav", // Another Sakai variant
        "#siteNav",            // Site navigation
        ".portal-neochat",     // Neo chat portal
        "#portal",             // Generic portal container
        ".Mrphs-sites",        // Sites container
    ];
    
    let login_start = std::time::Instant::now();
    let mut login_success = false;
    
    while login_start.elapsed().as_secs() < 20 {
        // Check URL-based success (if we're redirected to main portal)
        let current_url = tab.get_url();
        if current_url.contains("/portal/site/") || 
           current_url.contains("/portal/pda/") ||
           (current_url.contains("poliformat.upv.es/portal") && !current_url.contains("/login")) {
            tracing::info!("Login successful! Detected authenticated URL: {}", current_url);
            login_success = true;
            break;
        }
        
        // Check element-based success
        for selector in &success_selectors {
            if tab.find_element(selector).is_ok() {
                tracing::info!("Login successful! Found element: {}", selector);
                login_success = true;
                break;
            }
        }
        
        if login_success {
            break;
        }
        
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    
    if !login_success {
        let final_url = tab.get_url();
        tracing::error!("Login detection failed. Final URL: {}", final_url);
        anyhow::bail!("Login failed: Could not detect authenticated session after 20s. Final URL: {}", final_url);
    }

    tracing::info!("Session active! Extracting cookies...");

    let cookies = tab.get_cookies()?;
    let mut cookie_string = String::new();

    for cookie in cookies {
        if cookie.name == "JSESSIONID" || cookie.domain.contains("upv.es") {
            if !cookie_string.is_empty() {
                cookie_string.push_str("; ");
            }
            cookie_string.push_str(&format!("{}={}", cookie.name, cookie.value));
        }
    }

    if cookie_string.is_empty() {
        anyhow::bail!("No session cookies found after login!");
    }

    Ok(cookie_string)
}
