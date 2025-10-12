use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use rand::seq::SliceRandom;
use regex::Regex;
use reqwest::Client;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex; // Async mutex for rate limiting queue
use tracing::debug;
use url::Url;

// Constants
const RESULTS_PER_PAGE: u32 = 10;
const MAX_CACHE_PAGES: usize = 5;
const CACHE_DURATION: Duration = Duration::from_secs(5 * 60); // 5 minutes

// Rotating User Agents
static USER_AGENTS: &[&str] = &[
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Edge/120.0.0.0",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.2.1 Safari/605.1.15",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:122.0) Gecko/20100101 Firefox/122.0",
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
];

// Cache for search results
static RESULTS_CACHE: Lazy<Mutex<HashMap<String, CacheEntry>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone)]
struct CacheEntry {
    results: Vec<SearchResult>,
    timestamp: Instant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub favicon: String,
    pub display_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlMetadata {
    pub title: String,
    pub description: String,
    pub og_image: Option<String>,
    pub favicon: Option<String>,
    pub url: String,
}

// HTTP client
static HTTP_CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to create HTTP client")
});

// ---------------------------------------------------------------------------
// Global DuckDuckGo rate limiting (FIFO queue, min 5s between outbound calls)
// ---------------------------------------------------------------------------
// Even with multiple concurrent search tool executions we must
// ensure at least 5 seconds between actual HTTP requests to DuckDuckGo.
// Implemented this as a lazily-initialized global async mutex holding the
// timestamp of the last performed request. Tasks acquire the mutex in the
// order they reach .await (Tokio's mutex wake-up is fair enough for FIFO
// semantics here) and, if needed, sleep while holding the lock to preserve
// ordering and guarantee spacing (not releasing the lock before sleep avoids
// a race that could allow multiple sleepers to wake and fire simultaneously).
// Cache hits bypass this limiter entirely since they do not perform a network
// request.
const DDG_BASE_INTERVAL: Duration = Duration::from_secs(5);
static LAST_DDG_REQUEST: Lazy<AsyncMutex<Option<Instant>>> = Lazy::new(|| AsyncMutex::new(None));
static DDG_WAITING_COUNT: AtomicUsize = AtomicUsize::new(0);

async fn wait_for_duckduckgo_rate_limit() {
    // Register this task in the queue and capture its position (1-based)
    let position = DDG_WAITING_COUNT.fetch_add(1, Ordering::SeqCst) + 1;

    // Compute required minimal interval based on queue position:
    // 1..=3 => 5s, 4 =>10s, 5 =>15s, 6 =>20s, etc.
    let required_interval = if position <= 3 {
        DDG_BASE_INTERVAL
    } else {
        // (position - 2) * 5s yields: 4->10s, 5->15s, 6->20s ...
        DDG_BASE_INTERVAL * (position as u32 - 2)
    };

    let mut last_guard = LAST_DDG_REQUEST.lock().await;
    if let Some(prev) = *last_guard {
        let elapsed = prev.elapsed();
        if elapsed < required_interval {
            tokio::time::sleep(required_interval - elapsed).await;
        }
    }
    *last_guard = Some(Instant::now());

    // Done; remove from waiting count
    DDG_WAITING_COUNT.fetch_sub(1, Ordering::SeqCst);
}

/// Get a random user agent from the list
fn get_random_user_agent() -> &'static str {
    let mut rng = rand::thread_rng();
    USER_AGENTS.choose(&mut rng).unwrap_or(&USER_AGENTS[0])
}

/// Generate a cache key for a search query and page
fn get_cache_key(query: &str, page: u32) -> String {
    format!("{}-{}", query, page)
}

/// Clear old entries from the cache
fn clear_old_cache() {
    let mut cache = RESULTS_CACHE.lock().unwrap();
    let now = Instant::now();
    cache.retain(|_, entry| now.duration_since(entry.timestamp) < CACHE_DURATION);
}

/// Extract the direct URL from a DuckDuckGo redirect URL
fn extract_direct_url(duckduckgo_url: &str) -> String {
    // Handle relative URLs from DuckDuckGo
    let url_str = if duckduckgo_url.starts_with("//") {
        format!("https:{}", duckduckgo_url)
    } else if duckduckgo_url.starts_with('/') {
        format!("https://duckduckgo.com{}", duckduckgo_url)
    } else {
        duckduckgo_url.to_string()
    };

    match Url::parse(&url_str) {
        Ok(url) => {
            // Extract direct URL from DuckDuckGo redirect
            if url.host_str() == Some("duckduckgo.com") && url.path() == "/l/" {
                if let Some(uddg) = url.query_pairs().find(|(key, _)| key == "uddg") {
                    return urlencoding::decode(&uddg.1).unwrap_or_default().to_string();
                }
            }

            // Handle ad redirects
            if url.host_str() == Some("duckduckgo.com") && url.path() == "/y.js" {
                if let Some(u3) = url.query_pairs().find(|(key, _)| key == "u3") {
                    if let Ok(decoded_u3) = urlencoding::decode(&u3.1) {
                        if let Ok(u3_url) = Url::parse(&decoded_u3) {
                            if let Some(click_url) =
                                u3_url.query_pairs().find(|(key, _)| key == "ld")
                            {
                                return urlencoding::decode(&click_url.1)
                                    .unwrap_or_default()
                                    .to_string();
                            }
                        }
                        return decoded_u3.to_string();
                    }
                }
            }

            url_str
        }
        Err(_) => {
            // If URL parsing fails, try to extract URL from a basic string match
            static URL_REGEX: Lazy<Regex> =
                Lazy::new(|| Regex::new(r#"https?://[^\s<>""]+"#).unwrap());

            if let Some(captures) = URL_REGEX.find(duckduckgo_url) {
                captures.as_str().to_string()
            } else {
                duckduckgo_url.to_string()
            }
        }
    }
}

/// Get a favicon URL for a given website URL
fn get_favicon_url(url: &str) -> String {
    match Url::parse(url) {
        Ok(parsed_url) => {
            if let Some(host) = parsed_url.host_str() {
                format!("https://www.google.com/s2/favicons?domain={}&sz=32", host)
            } else {
                String::new()
            }
        }
        Err(_) => String::new(),
    }
}

/// Search DuckDuckGo and return results
pub async fn duckduckgo_search(
    query: &str,
    page: u32,
    num_results: u32,
) -> Result<Vec<SearchResult>> {
    // Clear old cache entries
    clear_old_cache();

    // Calculate start index for pagination
    let start_index = (page - 1) * RESULTS_PER_PAGE;

    // Check cache first
    let cache_key = get_cache_key(query, page);
    {
        let cache = RESULTS_CACHE.lock().unwrap();
        if let Some(cached_results) = cache.get(&cache_key) {
            if Instant::now().duration_since(cached_results.timestamp) < CACHE_DURATION {
                let end_index = std::cmp::min(num_results as usize, cached_results.results.len());
                return Ok(cached_results.results[..end_index].to_vec());
            }
        }
    }

    // Get a random user agent
    let user_agent = get_random_user_agent();

    // Fetch results
    let url = format!(
        "https://duckduckgo.com/html/?q={}&s={}",
        urlencoding::encode(query),
        start_index
    );

    debug!("Fetching search results from: {}", url);

    // Enforce global DuckDuckGo rate limit (only on cache miss path)
    wait_for_duckduckgo_rate_limit().await;

    let response = HTTP_CLIENT
        .get(&url)
        .header("User-Agent", user_agent)
        // Added browser-like headers to reduce rate limiting / bot detection
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Accept-Encoding", "gzip, deflate, br")
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "Failed to fetch search results: {}",
            response.status()
        ));
    }

    let html = response.text().await?;
    debug!("Received HTML length: {}", html.len());

    // Save HTML for debugging
    if std::env::var("DEBUG_HTML").is_ok() {
        std::fs::write("/tmp/ddg_debug.html", &html).ok();
        debug!("HTML saved to /tmp/ddg_debug.html");
    }

    // Check if we got a CAPTCHA or error page
    if html.contains("Unfortunately, bots use DuckDuckGo too")
        || html.contains("anomaly-modal")
        || html.contains("challenge-form")
        || html.contains("captcha")
        || html.contains("blocked")
        || html.len() < 1000
    {
        debug!("CAPTCHA or rate limit detected");
        return Err(anyhow!("Request limit exceeded, try other tool for search"));
    }

    let document = Html::parse_document(&html);

    // Parse results - try multiple selectors
    let result_selectors = [
        ".result",
        "[data-testid='result']",
        ".web-result",
        ".result-snippet",
        "article",
        ".serp-result",
    ];

    let mut results = Vec::new();
    let mut result_elements = Vec::new();

    for selector_str in &result_selectors {
        if let Ok(selector) = Selector::parse(selector_str) {
            let elements: Vec<_> = document.select(&selector).collect();
            if !elements.is_empty() {
                debug!(
                    "Found {} elements with selector: {}",
                    elements.len(),
                    selector_str
                );
                result_elements = elements;
                break;
            }
        }
    }

    debug!("Total result elements found: {}", result_elements.len());

    for result_element in result_elements {
        // Try multiple title selectors
        let title_selectors = [
            ".result__title a",
            "h2 a",
            "h3 a",
            "a[data-testid='result-title-a']",
            ".result-title a",
            "a.result-link",
        ];

        let mut title = String::new();
        let mut raw_link = String::new();

        for selector_str in &title_selectors {
            if let Ok(selector) = Selector::parse(selector_str) {
                if let Some(element) = result_element.select(&selector).next() {
                    title = clean_html_text(&element.inner_html()).trim().to_string();
                    raw_link = element.value().attr("href").unwrap_or_default().to_string();
                    if !title.is_empty() && !raw_link.is_empty() {
                        break;
                    }
                }
            }
        }

        // Try multiple snippet selectors
        let snippet_selectors = [
            ".result__snippet",
            ".result-snippet",
            ".snippet",
            "[data-testid='result-snippet']",
            ".result-description",
            "p",
        ];

        let mut snippet = String::new();
        for selector_str in &snippet_selectors {
            if let Ok(selector) = Selector::parse(selector_str) {
                if let Some(element) = result_element.select(&selector).next() {
                    snippet = clean_html_text(&element.inner_html()).trim().to_string();
                    if !snippet.is_empty() {
                        break;
                    }
                }
            }
        }

        // Try multiple URL display selectors
        let url_selectors = [
            ".result__url",
            ".result-url",
            ".url",
            "[data-testid='result-url']",
            "cite",
        ];

        let mut display_url = String::new();
        for selector_str in &url_selectors {
            if let Ok(selector) = Selector::parse(selector_str) {
                if let Some(element) = result_element.select(&selector).next() {
                    display_url = clean_html_text(&element.inner_html()).trim().to_string();
                    if !display_url.is_empty() {
                        break;
                    }
                }
            }
        }

        if !title.is_empty() && !raw_link.is_empty() {
            let direct_link = extract_direct_url(&raw_link);
            let favicon = get_favicon_url(&direct_link);

            results.push(SearchResult {
                title,
                url: direct_link,
                snippet,
                favicon,
                display_url,
            });
        }
    }

    // Get paginated results
    let end_index = std::cmp::min(num_results as usize, results.len());
    let paginated_results = results[..end_index].to_vec();

    // Cache the results
    {
        let mut cache = RESULTS_CACHE.lock().unwrap();
        cache.insert(
            cache_key,
            CacheEntry {
                results: paginated_results.clone(),
                timestamp: Instant::now(),
            },
        );

        // If cache is too big, remove oldest entries
        if cache.len() > MAX_CACHE_PAGES {
            let oldest_key = cache
                .iter()
                .min_by_key(|(_, entry)| entry.timestamp)
                .map(|(key, _)| key.clone());

            if let Some(key) = oldest_key {
                cache.remove(&key);
            }
        }
    }

    Ok(paginated_results)
}

/// Clean HTML text by removing tags and decoding entities
fn clean_html_text(html: &str) -> String {
    let fragment = Html::parse_fragment(html);
    let text = fragment.root_element().text().collect::<String>();
    clean_text(&text)
}

/// Clean up text by removing excessive whitespace and normalizing line breaks
fn clean_text(text: &str) -> String {
    static WHITESPACE_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
    static LINEBREAK_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"\n\s*\n").unwrap());

    let text = WHITESPACE_REGEX.replace_all(text, " ");
    let text = LINEBREAK_REGEX.replace_all(&text, "\n\n");
    text.trim().to_string()
}

/// Extract metadata from a URL
pub async fn extract_url_metadata(url: &str) -> Result<UrlMetadata> {
    let user_agent = get_random_user_agent();

    debug!("Extracting metadata from URL: {}", url);

    let response = HTTP_CLIENT
        .get(url)
        .header("User-Agent", user_agent)
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "Failed to fetch URL: {} ({})",
            url,
            response.status()
        ));
    }

    let html = response.text().await?;
    let document = Html::parse_document(&html);

    // Extract metadata
    let title_selector = Selector::parse("title").unwrap();
    let title = document
        .select(&title_selector)
        .next()
        .map(|el| el.inner_html())
        .unwrap_or_default();

    let description_selector = Selector::parse("meta[name=\"description\"]").unwrap();
    let og_description_selector = Selector::parse("meta[property=\"og:description\"]").unwrap();
    let description = document
        .select(&description_selector)
        .next()
        .and_then(|el| el.value().attr("content"))
        .or_else(|| {
            document
                .select(&og_description_selector)
                .next()
                .and_then(|el| el.value().attr("content"))
        })
        .unwrap_or_default()
        .to_string();

    let og_image_selector = Selector::parse("meta[property=\"og:image\"]").unwrap();
    let og_image = document
        .select(&og_image_selector)
        .next()
        .and_then(|el| el.value().attr("content"))
        .map(|img_url| resolve_url(url, img_url).unwrap_or_else(|_| img_url.to_string()));

    let favicon_selector =
        Selector::parse("link[rel=\"icon\"], link[rel=\"shortcut icon\"]").unwrap();
    let favicon = document
        .select(&favicon_selector)
        .next()
        .and_then(|el| el.value().attr("href"))
        .map(|favicon_url| {
            resolve_url(url, favicon_url).unwrap_or_else(|_| favicon_url.to_string())
        })
        .or_else(|| Some(get_favicon_url(url)));

    Ok(UrlMetadata {
        title,
        description,
        og_image,
        favicon,
        url: url.to_string(),
    })
}

/// Resolve a relative URL to an absolute URL
fn resolve_url(base: &str, relative: &str) -> Result<String> {
    let base_url = Url::parse(base)?;
    let resolved = base_url.join(relative)?;
    Ok(resolved.to_string())
}
