use serde::{Deserialize, Serialize};
use reqwest::Client;
use anyhow::Result;
use futures::Stream;
use std::pin::Pin;

#[derive(Clone)]
pub struct LlmClient {
    client: Client,
    base_url: String,
    pub model: String,
    pub api_key: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct ModelListResponse {
    pub data: Vec<ModelInfo>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default)]
    pub context_length: Option<usize>,
}

#[derive(Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip)]
    #[serde(default)]
    pub thinking_collapsed: bool, 
}

#[derive(Deserialize)]
pub struct ChatResponse {
    pub choices: Vec<ChatChoice>,
    pub usage: Option<Usage>,
}

#[derive(Deserialize)]
pub struct ChatChoice {
    pub message: ChatMessage,
}

#[derive(Deserialize, Debug)]
pub struct ChatStreamResponse {
    pub choices: Vec<ChatStreamChoice>,
    pub usage: Option<Usage>, 
}

#[derive(Deserialize, Debug)]
pub struct ChatStreamChoice {
    pub delta: ChatStreamDelta,
}

#[derive(Deserialize, Debug)]
pub struct ChatStreamDelta {
    pub content: Option<String>,
}

#[derive(Deserialize, Debug, Default, Clone)]
pub struct Usage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
}

impl LlmClient {
    pub fn new(base_url: Option<String>, model: Option<String>, api_key: Option<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.unwrap_or_else(|| "http://localhost:1234/v1".to_string()),
            model: model.unwrap_or_else(|| "local-model".to_string()),
            api_key,
        }
    }

    pub fn set_model(&mut self, model: &str) {
        self.model = model.to_string();
    }
    
    pub fn set_auth(&mut self, base_url: &str, api_key: Option<String>) {
        self.base_url = base_url.to_string();
        self.api_key = api_key;
    }

    pub async fn fetch_models(&self) -> Result<Vec<String>> {
        let url = format!("{}/models", self.base_url);
        let mut builder = self.client.get(&url);
        
        if let Some(key) = &self.api_key {
            builder = builder.header("Authorization", format!("Bearer {}", key));
        }
        
        let resp = builder.send().await?;
        
        if !resp.status().is_success() {
             anyhow::bail!("Failed to fetch models: {}", resp.status());
        }

        let body: ModelListResponse = resp.json().await?;
        Ok(body.data.into_iter().map(|m| m.id).collect())
    }
    
    /// Fetch context length for the current model
    pub async fn fetch_context_length(&self) -> Result<usize> {
        let url = format!("{}/models", self.base_url);
        let mut builder = self.client.get(&url);
        
        if let Some(key) = &self.api_key {
            builder = builder.header("Authorization", format!("Bearer {}", key));
        }
        
        let resp = builder.send().await?;
        
        if !resp.status().is_success() {
            return Ok(32768); // Default fallback
        }

        let body: ModelListResponse = resp.json().await?;
        
        // Find current model and get its context length
        for model in body.data {
            if model.id == self.model {
                if let Some(ctx_len) = model.context_length {
                    return Ok(ctx_len);
                }
            }
        }
        
        Ok(32768) // Default fallback
    }

    pub async fn chat(&self, messages: &[ChatMessage]) -> Result<(String, Option<Usage>)> {
        let url = format!("{}/chat/completions", self.base_url);
        
        let req = ChatRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            temperature: 0.7,
            stream: None,
        };

        let mut builder = self.client.post(&url).json(&req);
        
        if let Some(key) = &self.api_key {
            builder = builder.header("Authorization", format!("Bearer {}", key));
            // OpenRouter specific headers
            if self.base_url.contains("openrouter") {
                builder = builder.header("HTTP-Referer", "http://localhost:8080")
                               .header("X-Title", "PoliRag");
            }
        }

        let resp = builder.send().await?;

        if !resp.status().is_success() {
             let err_text = resp.text().await.unwrap_or_default();
             anyhow::bail!("Chat request failed: {}", err_text);
        }

        let body: ChatResponse = resp.json().await?;
        
        let content = body.choices.first()
            .map(|c| c.message.content.clone())
            .ok_or_else(|| anyhow::anyhow!("No choices in response"))?;
            
        Ok((content, body.usage))
    }

    pub async fn chat_stream(&self, messages: &[ChatMessage]) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let url = format!("{}/chat/completions", self.base_url);
        
        // Ensure stream is true
        let req = ChatRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            temperature: 0.7,
            stream: Some(true),
        };

        let mut builder = self.client.post(&url).json(&req);
        
        if let Some(key) = &self.api_key {
            builder = builder.header("Authorization", format!("Bearer {}", key));
            if self.base_url.contains("openrouter") {
                builder = builder.header("HTTP-Referer", "http://localhost:8080")
                               .header("X-Title", "PoliRag");
            }
        }

        let resp = builder.send().await?;

        if !resp.status().is_success() {
             let err_text = resp.text().await.unwrap_or_default();
             anyhow::bail!("Chat request failed: {}", err_text);
        }

        // Create stream
        let stream = resp.bytes_stream();
        
        // Transform the stream of bytes/strings into a stream of content deltas
        let processed_stream = async_stream::try_stream! {
            let mut buffer = String::new();
            
            for await chunk_res in stream {
                let bytes = chunk_res.map_err(|e| anyhow::anyhow!("Stream error: {}", e))?;
                let chunk_str = String::from_utf8_lossy(&bytes);
                buffer.push_str(&chunk_str);
                
                while let Some(pos) = buffer.find('\n') {
                    let line = buffer[..pos].trim().to_string();
                    if pos + 1 < buffer.len() {
                        buffer = buffer[pos + 1..].to_string();
                    } else {
                        buffer.clear();
                    }
                    
                    if line.starts_with("data: ") {
                        let data = line[6..].trim();
                        if data == "[DONE]" {
                            break;
                        }
                        
                        // Try parsing as ChatStreamResponse
                        if let Ok(resp) = serde_json::from_str::<ChatStreamResponse>(data) {
                            if let Some(choice) = resp.choices.first() {
                                if let Some(content) = &choice.delta.content {
                                    yield StreamEvent::Content(content.clone());
                                }
                            }
                            if let Some(usage) = resp.usage {
                                yield StreamEvent::Usage(usage);
                            }
                        }
                    }
                }
            }
        };

        Ok(Box::pin(processed_stream))
    }
}

pub enum StreamEvent {
    Content(String),
    Usage(Usage),
}
