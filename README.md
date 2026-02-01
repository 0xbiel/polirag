# PoliRag: Retrieval-Augmented Generation over PoliformaT

_By 0xbiel_

![PoliRag TUI](https://raw.githubusercontent.com/0xbiel/polirag/main/assets/demo.png)

## Overview

**PoliRag** turns your university course content into an interactive, queriable knowledge base. It automates the extraction of subjects, announcements, lessons, and resources from the **PoliformaT** platform (UPV) and powers a local RAG (Retrieval-Augmented Generation) system.

Chat with your subjects directly from the terminal, with answers grounded in your specific teaching guides and uploaded documents.

## Key Features

- **🚀 Automated Scraping**: Headless browser automation (Chrome) handles UPV SSO login and extracts content from all your enrolled subjects.
- **🧠 Local RAG Pipeline**:
  - **Embeddings**: Uses `embeddinggemma-300m` locally (no API costs).
  - **Vector Store**: High-performance **HNSW** (Hierarchical Navigable Small World) index for instant retrieval.
  - **Privacy**: All documents and embeddings stay on your machine.
- **🖥️ Advanced TUI**:
  - Built with `ratatui` for a responsive, keyboard-driven experience.
  - **Markdown Streaming**: Smooth text rendering with syntax highlighting.
  - **Thinking Process**: Native support for reasoning models (e.g., DeepSeek R1) with collapsible thought blocks.
  - **Async Architecture**: UI never freezes during scraping or inference.
- **🤖 LLM Flexibility**: Connects to:
  - **LM Studio** (Local inference)
  - **OpenRouter** (Cloud inference)

## Installation & Setup

### 1. Prerequisites
- **Rust Toolchain**: `cargo` installed.
- **Google Chrome**: Required for the headless scraper.

### 2. Download the Embedding Model
PoliRag uses a local GGUF model for embedding generation. Download `embeddinggemma-300m-Q4_0.gguf` to the project root:

```bash
# Using wget
wget https://huggingface.co/unsloth/embeddinggemma-300m-GGUF/resolve/main/embeddinggemma-300m-Q4_0.gguf

# Or using curl
curl -L -o embeddinggemma-300m-Q4_0.gguf https://huggingface.co/unsloth/embeddinggemma-300m-GGUF/resolve/main/embeddinggemma-300m-Q4_0.gguf
```
> [!NOTE]
> File size: ~265 MB.

### 3. Build and Run
```bash
cargo run --release
```

## Usage

### 🔄 Sync Data
Select **Sync Data** from the main menu. PoliRag will:
1. Launch a headless browser.
2. Log in to PoliformaT (you may need to approve 2FA on your phone).
3. Scrape your subjects and download PDF/ZIP resources.
4. Process and index all text into the local HNSW vector store.

### 💬 Chat
Select **Chat with Assistant**.
- **Ask questions** about your subjects (e.g., "What is the evaluation method for IAP?").
- **Collapsible Thinking**: Toggle the "Thinking" block visibility with `Ctrl+T` (if using a reasoning model).
- **History**: Scroll up/down to view past context.

### ⚙️ Configuration
Credentials and settings are stored locally in your OS data directory.
- **Provider**: Toggle between Local (LM Studio) and Cloud (OpenRouter).
- **Models**: Enter your preferred model name (e.g., `deepseek/deepseek-r1`).

## Technical Architecture

For a deep dive into the system internals, see [project_docs.tex](./project_docs.tex).

- **Scraper**: `headless_chrome` + `reqwest` (cookie sharing).
- **Vector DB**: `hnsw_rs`.
- **UI**: `ratatui` + `crossterm` + Custom Markdown Renderer.

## License
MIT
