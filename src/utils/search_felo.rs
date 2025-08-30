use anyhow::{anyhow, Result};
use futures::StreamExt;
use once_cell::sync::Lazy;
use rand::seq::SliceRandom;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::{debug, warn};
use uuid::Uuid;

// Constants
const CACHE_DURATION: Duration = Duration::from_secs(5 * 60); // 5 minutes

// Rotating User Agents
static USER_AGENTS: &[&str] = &[
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Edge/120.0.0.0",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.2.1 Safari/605.1.15",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:122.0) Gecko/20100101 Firefox/122.0",
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
];

// Cache for Felo results
static FELO_CACHE: Lazy<Mutex<HashMap<String, CacheEntry>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone)]
struct CacheEntry {
    result: String,
    timestamp: Instant,
}

#[derive(Debug, Serialize)]
struct FeloSearchPayload {
    query: String,
    search_uuid: String,
    lang: String,
    agent_lang: String,
    search_options: FeloSearchOptions,
    search_video: bool,
    contexts_from: String,
}

#[derive(Debug, Serialize)]
struct FeloSearchOptions {
    langcode: String,
}

#[derive(Debug, Deserialize)]
struct FeloStreamData {
    #[serde(rename = "type")]
    data_type: String,
    data: FeloDataContent,
}

#[derive(Debug, Deserialize)]
struct FeloDataContent {
    text: Option<String>,
}

// HTTP client
static HTTP_CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to create HTTP client")
});

/// Get a random user agent from the list
fn get_random_user_agent() -> &'static str {
    let mut rng = rand::thread_rng();
    USER_AGENTS.choose(&mut rng).unwrap_or(&USER_AGENTS[0])
}

/// Generate a cache key for a Felo search query
fn get_cache_key(query: &str) -> String {
    format!("felo-{}", query)
}

/// Clear old entries from the cache
fn clear_old_cache() {
    let mut cache = FELO_CACHE.lock().unwrap();
    let now = Instant::now();
    cache.retain(|_, entry| now.duration_since(entry.timestamp) < CACHE_DURATION);
}

/// Search using the Felo AI API
pub async fn search_felo(prompt: &str, stream: bool) -> Result<String> {
    // Clear old cache entries
    clear_old_cache();

    // Check cache first if not streaming
    if !stream {
        let cache_key = get_cache_key(prompt);
        let cache = FELO_CACHE.lock().unwrap();
        if let Some(cached_result) = cache.get(&cache_key) {
            if Instant::now().duration_since(cached_result.timestamp) < CACHE_DURATION {
                return Ok(cached_result.result.clone());
            }
        }
    }

    // Create payload for Felo API
    let payload = FeloSearchPayload {
        query: prompt.to_string(),
        search_uuid: Uuid::new_v4().to_string(),
        lang: String::new(),
        agent_lang: "en".to_string(),
        search_options: FeloSearchOptions {
            langcode: "en-US".to_string(),
        },
        search_video: true,
        contexts_from: "google".to_string(),
    };

    // Get a random user agent
    let user_agent = get_random_user_agent();

    debug!("Sending Felo AI request with payload: {:?}", payload);

    // Make the request
    let response = HTTP_CLIENT
        .post("https://api.felo.ai/search/threads")
        .header("accept", "*/*")
        .header("accept-encoding", "gzip, deflate, br")
        .header("accept-language", "en-US,en;q=0.9")
        .header("content-type", "application/json")
        .header("cookie", "_clck=1gifk45%7C2%7Cfoa%7C0%7C1686; _clsk=1g5lv07%7C1723558310439%7C1%7C1%7Cu.clarity.ms%2Fcollect; _ga=GA1.1.877307181.1723558313; _ga_8SZPRV97HV=GS1.1.1723558313.1.1.1723558341.0.0.0; _ga_Q9Q1E734CC=GS1.1.1723558313.1.1.1723558341.0.0.0")
        .header("dnt", "1")
        .header("origin", "https://felo.ai")
        .header("referer", "https://felo.ai/")
        .header("sec-ch-ua", "\"Not)A;Brand\";v=\"99\", \"Microsoft Edge\";v=\"127\", \"Chromium\";v=\"127\"")
        .header("sec-ch-ua-mobile", "?0")
        .header("sec-ch-ua-platform", "\"Windows\"")
        .header("sec-fetch-dest", "empty")
        .header("sec-fetch-mode", "cors")
        .header("sec-fetch-site", "same-site")
        .header("user-agent", user_agent)
        .json(&payload)
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "Felo API request failed: {} - {}",
            response.status(),
            response.text().await.unwrap_or_default()
        ));
    }

    let mut full_response = String::new();
    let mut bytes_stream = response.bytes_stream();

    while let Some(chunk_result) = bytes_stream.next().await {
        let chunk = chunk_result?;
        let chunk_str = String::from_utf8_lossy(&chunk);

        // Process each line in the chunk
        for line in chunk_str.lines() {
            if line.starts_with("data:") {
                let data_part = line.strip_prefix("data:").unwrap_or("").trim();

                if data_part.is_empty() || data_part == "[DONE]" {
                    continue;
                }

                match serde_json::from_str::<FeloStreamData>(data_part) {
                    Ok(stream_data) => {
                        if stream_data.data_type == "answer" {
                            if let Some(text) = stream_data.data.text {
                                if text.len() > full_response.len() {
                                    let delta = text[full_response.len()..].to_string();
                                    full_response = text;

                                    if stream {
                                        // For streaming, we would yield the delta here
                                        // But since we're returning a single string, we'll just continue
                                        debug!("Received delta: {}", delta);
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        debug!("Failed to parse stream data: {} - {}", e, data_part);
                        // Continue processing other lines
                    }
                }
            }
        }
    }

    // Cache the complete response if not streaming
    if !stream && !full_response.is_empty() {
        let cache_key = get_cache_key(prompt);
        let mut cache = FELO_CACHE.lock().unwrap();
        cache.insert(
            cache_key,
            CacheEntry {
                result: full_response.clone(),
                timestamp: Instant::now(),
            },
        );
    }

    if full_response.is_empty() {
        warn!("Felo AI returned empty response for query: {}", prompt);
        return Ok("No response received from Felo AI.".to_string());
    }

    Ok(full_response)
}
