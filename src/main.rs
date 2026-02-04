use std::sync::{Arc, Mutex};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Layer};
use clap::{Parser, Subcommand};

mod rag;
mod scrapper;
mod llm;
mod tui;
mod config;
mod ops;

use llm::LlmClient;

#[derive(Parser)]
#[command(name = "polirag")]
#[command(version = "1.0")]
#[command(about = "PoliformaT RAG Assistant", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Clone)]
enum Commands {
    /// Run synchronization (headless scrape & index)
    Sync,
    /// Open the Interactive Menu (Default)
    Menu,
    /// Internal: Extract PDF text (hidden)
    #[command(hide = true)]
    ExtractPdf {
        path: String,
    },
}

pub struct AppState {
    pub rag: Arc<rag::RagSystem>,
    pub poliformat: Arc<scrapper::PoliformatClient>,
    pub llm: Arc<Mutex<LlmClient>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();
    let cli = Cli::parse();
    
    // Check for internal commands to skip full setup
    if let Some(Commands::ExtractPdf { path }) = &cli.command {
        // Run extraction and exit immediately
        let path = std::path::PathBuf::from(path);
        match std::panic::catch_unwind(|| pdf_extract::extract_text(&path)) {
            Ok(Ok(text)) => {
                // Print with delimiters to separate from potential library noise
                println!("<<<START_CONTENT>>>{}<<<END_CONTENT>>>", text);
                std::process::exit(0);
            },
            Ok(Err(e)) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            },
            Err(_) => {
                eprintln!("Panic during extraction");
                std::process::exit(2);
            }
        }
    }
    
    // Ensure APP Data Dir exists
    let app_dir = config::Config::get_app_data_dir();
    
    // Setup logging
    // let log_file = app_dir.join("debug.log");
    let file_appender = tracing_appender::rolling::never(app_dir, "debug.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_filter(tracing_subscriber::EnvFilter::new("debug,headless_chrome=info")) 
        )
        // Only log errors to stderr to avoid messing up TUI
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(tracing_subscriber::EnvFilter::new("error")) 
        )
        .init();

    // Initialize Systems using Global Path
    let index_path = config::Config::get_index_path();
    let index_path_str = index_path.to_string_lossy();
    
    let rag = Arc::new(rag::RagSystem::new(&index_path_str)?);
    let poliformat = Arc::new(scrapper::PoliformatClient::new());
    let mut llm_client = LlmClient::new(None, None, None); // Defaults to localhost:1234
    
    // Try to load saved model from config first
    if let Some(saved_model) = config::Config::get_last_model() {
        tracing::info!("Loaded saved model from config: {}", saved_model);
        llm_client.set_model(&saved_model);
    } else {
        // Auto-detect model on startup if no saved model
        if let Ok(models) = llm_client.fetch_models().await {
            if let Some(first) = models.first() {
                tracing::info!("Auto-detected LLM Model: {}", first);
                llm_client.set_model(first);
                let _ = config::Config::save_model(first);
            }
        }
    }

    let llm = Arc::new(Mutex::new(llm_client));
    let state = Arc::new(AppState { 
        rag: rag.clone(), 
        poliformat: poliformat.clone(),
        llm: llm.clone()
    });

    // Determine command
    let command = cli.command.unwrap_or(Commands::Menu);

    match command {
        Commands::Sync => {
             println!("Starting Sync (Detailed logs in debug.log)...");
             ops::run_sync(rag, poliformat).await?;
        },
        Commands::Menu => {
             tui::run_app(state).await?;
        },
        Commands::ExtractPdf { .. } => unreachable!(), // Handled above
    }

    // Drop guard to flush and close the log file
    drop(_guard);
    
    // Clean up debug log on clean exit
    let log_file = config::Config::get_app_data_dir().join("debug.log");
    if log_file.exists() {
        let _ = std::fs::remove_file(log_file);
    }

    Ok(())
}
