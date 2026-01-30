# PoliRag: Retrieval-Augmented Generation over PoliformaT

_By 0xbiel_

## Abstract

PoliRag is a Retrieval-Augmented Generation (RAG) system designed to turn a user’s PoliformaT course content (subjects, announcements, lesson pages, teaching guides, and downloaded resources like PDFs) into a searchable personal knowledge base. At query time, PoliRag retrieves the most relevant snippets from that knowledge base and provides them as grounded context to a language model, improving factuality and reducing hallucinations compared to prompting without retrieval.

## What is PoliRag?

PoliRag combines three components:

1. **Ingestion (scraping + extraction)**. Content is collected from PoliformaT via a headless browser flow and via authenticated HTTP requests using imported session cookies. Downloaded resources (e.g., zip bundles of course files) are unpacked and PDFs are extracted to clean text.
2. **Indexing (embeddings + vector store)**. Each document is embedded into a fixed-length vector using a local embedding model. Documents, metadata, and embeddings are stored on disk in a compact serialized vector index.
3. **Retrieval (semantic search + snippets)**. A user query is embedded, compared against stored document embeddings using cosine similarity, and the top matches are turned into concise snippets suitable for LLM context.

The design goal is a personal, offline-first RAG workflow: embeddings are computed locally, and the on-disk index can be rebuilt or re-embedded when the embedding model changes.

## Setup

### Download the Embedding Model

PoliRag requires a local embedding model to function. Download the `embeddinggemma-300m-Q4_0.gguf` file and place it in the root directory of the project:

```bash
# Download from Hugging Face
wget https://huggingface.co/unsloth/embeddinggemma-300m-GGUF/resolve/main/embeddinggemma-300m-Q4_0.gguf

# Or use curl
curl -L -o embeddinggemma-300m-Q4_0.gguf https://huggingface.co/unsloth/embeddinggemma-300m-GGUF/resolve/main/embeddinggemma-300m-Q4_0.gguf
```

> [!NOTE]
> This file is approximately 265 MB and is required for the RAG system to generate embeddings.

## Configuration, CLI, and Operations

### Configuration (`config.rs`)

PoliRag persists user settings and cached credentials in a JSON config file stored under the OS application data directory (e.g., via `dirs::data_dir()`). Credentials are stored as an encrypted blob (XOR + base64) so they are not plain-text on disk.

The configuration also tracks which LLM backend to use (LM Studio local endpoint vs OpenRouter), plus the most recently selected model.

```rust
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
    pub last_model: Option<String>,
    pub cached_credentials: Option<EncryptedCredentials>,
    pub llm_provider: LlmProvider,
    pub openrouter_api_key: Option<String>,
    pub openrouter_model: Option<String>,
}
```

### Entrypoint and Commands (`main.rs`)

The application exposes a small CLI with `clap`:

- **Menu**: The default interactive terminal UI.
- **Sync**: A headless scrape + index build suitable for cron/automation.
- **ExtractPdf**: An internal subcommand used to isolate PDF text extraction into a subprocess.

On startup, PoliRag initializes logging, loads the vector index path from the global config directory, and initializes the RAG, scraper, and LLM client.

```rust
#[derive(Subcommand, Clone)]
enum Commands {
    Sync,
    Menu,
    #[command(hide = true)]
    ExtractPdf { path: String },
}
```

### Sync Pipeline (`ops.rs`)

The sync operation is responsible for ensuring authentication (cached credentials or environment variables), clearing prior state, scraping subjects, extracting resources, and finally adding documents to the vector index.

```rust
pub async fn run_sync(rag: Arc<rag::RagSystem>, poliformat: Arc<scrapper::PoliformatClient>)
    -> anyhow::Result<()>
{
    if !poliformat.check_connection().await.unwrap_or(false) {
        // try cached credentials; fall back to env vars
        // perform headless login (blocking)
    }
    rag.clear()?;
    let subjects = poliformat.get_subjects().await?;
    let detailed = poliformat.scrape_subject_content(subjects).await?;
    
    for (sub, dir_path) in detailed {
        // read summary.md, process PDFs, add documents with metadata
        // rag.add_document(...).await?;
    }
    Ok(())
}
```

## Limitations and Next Steps

- **Index scalability**: The current approach is linear scan; a future upgrade could add ANN (HNSW) for very large corpora.
- **Chunking quality**: Word-based chunking is a heuristic; sentence/semantic chunking often improves retrieval.
- **Grounding UX**: Adding explicit per-snippet source identifiers and timestamps would make answers easier to trust.
