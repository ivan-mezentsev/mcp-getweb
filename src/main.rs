use clap::{Arg, Command};
use std::env;
use std::process;
use tracing::{error, info, warn};

mod mcp;
mod tools;
mod utils;

use mcp::server::{GoogleSearchConfig, McpServer};

/// Prints a formatted box with the given lines
/// Empty strings create empty lines, other strings are centered within the box
fn print_box(lines: &[&str]) {
    const BOX_WIDTH: usize = 60; // Total width including borders
    const CONTENT_WIDTH: usize = BOX_WIDTH - 4; // Width for content (excluding "║  " and "  ║")

    eprintln!("\n\x1b[36m╔{}╗", "═".repeat(BOX_WIDTH - 2));

    for line in lines {
        if line.is_empty() {
            // Empty line
            eprintln!("║{}║", " ".repeat(BOX_WIDTH - 2));
        } else {
            // Calculate visible length (excluding ANSI escape codes)
            let visible_len = strip_ansi_codes(line).len();

            if visible_len < CONTENT_WIDTH {
                // Center the text
                let total_padding = CONTENT_WIDTH - visible_len;
                let left_padding = total_padding / 2;
                let right_padding = total_padding - left_padding;

                eprintln!(
                    "║  {}{}{}\x1b[36m║",
                    " ".repeat(left_padding),
                    line,
                    " ".repeat(right_padding)
                );
            } else {
                // Text is too long, just fit it
                eprintln!("║  {}\x1b[36m  ║", line);
            }
        }
    }

    eprintln!("╚{}╝\x1b[0m\n", "═".repeat(BOX_WIDTH - 2));
}

/// Strips ANSI escape codes to calculate visible text length
fn strip_ansi_codes(text: &str) -> String {
    let mut result = String::new();
    let mut chars = text.chars();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // Skip escape sequence
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            result.push(ch);
        }
    }

    result
}

#[tokio::main]
async fn main() {
    // Parse command line arguments first
    let matches = Command::new("mcp-getweb")
        .version("1.2.2")
        .about("A Model Context Protocol server for web search")
        .author("Ivan Mezentsev")
        .long_about(
            "This MCP server provides the following tools:\n\
            - duckduckgo-search: Search the web using DuckDuckGo\n\
            - google-search: Search the web using Google\n\
            - fetch-url: Fetch and extract content from a URL\n\
            - url-metadata: Extract metadata from a URL\n\
            - url-fetch: Fetch web pages and convert them to markdown\n\
            - felo-search: Search using Felo AI for AI-generated responses",
        )
        .arg(
            Arg::new("google-api-key")
                .long("google-api-key")
                .value_name("KEY")
                .help("Google Custom Search API key")
                .action(clap::ArgAction::Set),
        )
        .arg(
            Arg::new("google-search-engine-id")
                .long("google-search-engine-id")
                .value_name("ID")
                .help("Google Custom Search Engine ID")
                .action(clap::ArgAction::Set),
        )
        .arg(
            Arg::new("jina-api-key")
                .long("jina-api-key")
                .value_name("KEY")
                .help("Jina Reader API key")
                .action(clap::ArgAction::Set),
        )
        .arg(
            Arg::new("quiet")
                .long("quiet")
                .short('q')
                .help("Suppress promotional messages (for MCP clients)")
                .action(clap::ArgAction::SetTrue),
        )
        .get_matches();

    // Initialize tracing to stderr only (stdout is reserved for JSON-RPC)
    let log_level = if std::env::var("RUST_LOG").is_ok() {
        // Use RUST_LOG if set
        None
    } else if matches.get_flag("quiet") {
        // In quiet mode, only show errors
        Some("error")
    } else {
        // Default to info level
        Some("info")
    };

    let subscriber = tracing_subscriber::fmt().with_writer(std::io::stderr);

    if let Some(level) = log_level {
        std::env::set_var("RUST_LOG", level);
    }

    subscriber.init();

    // Get Google Search configuration from environment variables or command line arguments
    let google_api_key = matches
        .get_one::<String>("google-api-key")
        .cloned()
        .or_else(|| env::var("GOOGLE_API_KEY").ok());

    let google_search_engine_id = matches
        .get_one::<String>("google-search-engine-id")
        .cloned()
        .or_else(|| env::var("GOOGLE_SEARCH_ENGINE_ID").ok());

    // Get Jina API key from command line or environment
    let jina_api_key = matches
        .get_one::<String>("jina-api-key")
        .cloned()
        .or_else(|| std::env::var("JINA_API_KEY").ok());

    // Log Google Search configuration status (without exposing secrets)
    match (&google_api_key, &google_search_engine_id) {
        (Some(_), Some(_)) => {
            info!("Google Search tool enabled");
        }
        (Some(_), None) => {
            warn!("Google Search API key found but Search Engine ID missing - Google Search tool will be disabled");
        }
        (None, Some(_)) => {
            warn!("Google Search Engine ID found but API key missing - Google Search tool will be disabled");
        }
        (None, None) => {
            // Both are None, no action needed
        }
    }

    // Log Jina Reader configuration status (without exposing secrets)
    match &jina_api_key {
        Some(_) => {
            info!("Jina Reader tool enabled");
        }
        None => {
            info!("Jina Reader API key not found - Jina Reader tool will be disabled");
        }
    }

    let google_config =
        if let (Some(api_key), Some(engine_id)) = (google_api_key, google_search_engine_id) {
            Some(GoogleSearchConfig {
                api_key,
                search_engine_id: engine_id,
            })
        } else {
            None
        };

    // Display promotional message (unless quiet mode)
    if !matches.get_flag("quiet") {
        print_box(&[
            "",
            "\x1b[1m\x1b[31m MCP-GetWeb: Web Search Server \x1b[0m",
            "",
            "\x1b[0m Model Context Protocol server for web search \x1b[0m",
            "",
            "\x1b[90m https://github.com/ivan-mezentsev/mcp-getweb \x1b[0m",
            "",
        ]);
    }

    // Start the MCP server
    info!("Starting MCP server...");

    let mut server = McpServer::new(google_config, jina_api_key);
    if let Err(e) = server.start().await {
        error!("Failed to start server: {}", e);
        process::exit(1);
    }
}
