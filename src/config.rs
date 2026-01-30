use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use anyhow::Result;

const CONFIG_FILE: &str = ".polirag.json";
const ENCRYPTION_KEY: &[u8] = b"PoliRag2026SecretKey!@#$%";

#[derive(Serialize, Deserialize, Clone, Default, PartialEq)]
pub enum LlmProvider {
    #[default]
    LmStudio,
    OpenRouter,
}

impl LlmProvider {
    pub fn base_url(&self) -> &'static str {
        match self {
            LlmProvider::LmStudio => "http://localhost:1234/v1",
            LlmProvider::OpenRouter => "https://openrouter.ai/api/v1",
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub last_model: Option<String>,
    #[serde(default)]
    pub cached_credentials: Option<EncryptedCredentials>,
    #[serde(default)]
    pub llm_provider: LlmProvider,
    #[serde(default)]
    pub openrouter_api_key: Option<String>,
    #[serde(default)]
    pub openrouter_model: Option<String>,
}

/// Encrypted credentials stored in config
#[derive(Serialize, Deserialize, Clone)]
pub struct EncryptedCredentials {
    pub username_encrypted: String,
    pub pin_encrypted: String,
}

/// Decrypted credentials for use
#[derive(Clone)]
pub struct CachedCredentials {
    pub username: String,
    pub pin: String,
}

// Simple XOR encryption with base64 encoding
fn encrypt(data: &str) -> String {
    let encrypted: Vec<u8> = data
        .bytes()
        .zip(ENCRYPTION_KEY.iter().cycle())
        .map(|(b, k)| b ^ k)
        .collect();
    base64_encode(&encrypted)
}

fn decrypt(encrypted: &str) -> Option<String> {
    let bytes = base64_decode(encrypted)?;
    let decrypted: Vec<u8> = bytes
        .iter()
        .zip(ENCRYPTION_KEY.iter().cycle())
        .map(|(b, k)| b ^ k)
        .collect();
    String::from_utf8(decrypted).ok()
}

// Simple base64 encoding (no external dependency)
fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    
    for chunk in data.chunks(3) {
        let mut n: u32 = 0;
        for (i, &byte) in chunk.iter().enumerate() {
            n |= (byte as u32) << (16 - 8 * i);
        }
        
        let padding = 3 - chunk.len();
        for i in 0..(4 - padding) {
            let idx = ((n >> (18 - 6 * i)) & 0x3F) as usize;
            result.push(ALPHABET[idx] as char);
        }
        for _ in 0..padding {
            result.push('=');
        }
    }
    result
}

fn base64_decode(data: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = Vec::new();
    let data = data.trim_end_matches('=');
    let bytes: Vec<u8> = data.bytes().collect();
    
    for chunk in bytes.chunks(4) {
        let mut n: u32 = 0;
        for (i, &b) in chunk.iter().enumerate() {
            let idx = ALPHABET.iter().position(|&c| c == b)? as u32;
            n |= idx << (18 - 6 * i);
        }
        
        let num_bytes = match chunk.len() {
            4 => 3,
            3 => 2,
            2 => 1,
            _ => 0,
        };
        for i in 0..num_bytes {
            result.push(((n >> (16 - 8 * i)) & 0xFF) as u8);
        }
    }
    Some(result)
}

impl Config {
    /// Get the application data directory
    pub fn get_app_data_dir() -> PathBuf {
        let mut path = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
        path.push("polirag");
        
        if !path.exists() {
            let _ = std::fs::create_dir_all(&path);
        }
        path
    }

    fn config_path() -> Option<PathBuf> {
        let path = Self::get_app_data_dir().join("config.json");
        Some(path)
    }

    pub fn get_index_path() -> PathBuf {
        Self::get_app_data_dir().join("polirag.index")
    }

    pub fn get_scraped_data_dir() -> PathBuf {
        Self::get_app_data_dir().join("data")
    }

    pub fn load() -> Config {
        // Check legacy path first (home dir)
        if let Some(home) = dirs::home_dir() {
            let legacy_path = home.join(CONFIG_FILE);
            if legacy_path.exists() {
                if let Ok(contents) = std::fs::read_to_string(&legacy_path) {
                    if let Ok(config) = serde_json::from_str(&contents) {
                        return config;
                    }
                }
            }
        }
        
        if let Some(path) = Self::config_path() {
            if path.exists() {
                if let Ok(contents) = std::fs::read_to_string(&path) {
                    if let Ok(config) = serde_json::from_str(&contents) {
                        return config;
                    }
                }
            }
        }
        Config::default()
    }

    pub fn save(&self) -> Result<()> {
        if let Some(path) = Self::config_path() {
            let contents = serde_json::to_string_pretty(self)?;
            std::fs::write(&path, contents)?;
        }
        Ok(())
    }

    pub fn save_model(model: &str) -> Result<()> {
        let mut config = Config::load();
        config.last_model = Some(model.to_string());
        config.save()
    }

    pub fn get_last_model() -> Option<String> {
        Config::load().last_model
    }

    /// Save credentials (encrypted)
    pub fn save_credentials(username: &str, pin: &str) -> Result<()> {
        let mut config = Config::load();
        config.cached_credentials = Some(EncryptedCredentials {
            username_encrypted: encrypt(username),
            pin_encrypted: encrypt(pin),
        });
        config.save()
    }

    /// Get cached credentials (decrypted)
    pub fn get_credentials() -> Option<CachedCredentials> {
        let config = Config::load();
        let enc = config.cached_credentials?;
        
        let username = decrypt(&enc.username_encrypted)?;
        let pin = decrypt(&enc.pin_encrypted)?;
        
        Some(CachedCredentials { username, pin })
    }

    pub fn clear_credentials() -> Result<()> {
        let mut config = Config::load();
        config.cached_credentials = None;
        config.save()
    }

    pub fn save_provider_config(provider: LlmProvider, api_key: Option<String>, model: Option<String>) -> Result<()> {
        let mut config = Config::load();
        config.llm_provider = provider;
        if let Some(key) = api_key {
            config.openrouter_api_key = Some(key);
        }
        if let Some(m) = model {
            config.openrouter_model = Some(m);
        }
        config.save()
    }
}
