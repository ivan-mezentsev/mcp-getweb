use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use rand::seq::SliceRandom;
use regex::Regex;
use reqwest::Client;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
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

#[derive(Debug, Clone)]
pub struct ContentExtractionOptions {
    pub extract_main_content: bool,
    pub include_links: bool,
    pub include_images: bool,
    pub exclude_tags: Vec<String>,
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

/// Fetch the content of a URL and return it as text
pub async fn fetch_url_content(url: &str, options: &ContentExtractionOptions) -> Result<String> {
    let user_agent = get_random_user_agent();

    debug!("Fetching content from URL: {}", url);

    let response = HTTP_CLIENT
        .get(url)
        .header("User-Agent", user_agent)
        .timeout(Duration::from_secs(10))
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "Failed to fetch URL: {} ({})",
            url,
            response.status()
        ));
    }

    // Check content type
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|ct| ct.to_str().ok())
        .unwrap_or("");

    if !content_type.contains("text/html") {
        // For non-HTML content, return as is
        let text = response.text().await?;
        return Ok(text);
    }

    let html = response.text().await?;
    let document = Html::parse_document(&html);

    // Remove unwanted elements
    let mut unwanted_selectors = vec!["script", "style", "noscript", "iframe", "svg"];

    for tag in &options.exclude_tags {
        unwanted_selectors.push(tag.as_str());
    }

    // Remove ads and other common unwanted elements
    let _ad_selectors = [
        "[id*=\"ad\"]",
        "[class*=\"ad\"]",
        "[id*=\"banner\"]",
        "[class*=\"banner\"]",
        "[id*=\"popup\"]",
        "[class*=\"popup\"]",
        "[class*=\"cookie\"]",
        "[id*=\"cookie\"]",
        "[class*=\"newsletter\"]",
        "[id*=\"newsletter\"]",
        "[class*=\"social\"]",
        "[id*=\"social\"]",
        "[class*=\"share\"]",
        "[id*=\"share\"]",
    ];

    // Try to extract main content if requested
    if options.extract_main_content {
        let content_selectors = [
            "article",
            "main",
            "[role=\"main\"]",
            ".post-content",
            ".article-content",
            ".content",
            "#content",
            ".post",
            ".article",
            ".entry-content",
            ".page-content",
            ".post-body",
            ".post-text",
            ".story-body",
        ];

        for selector_str in &content_selectors {
            if let Ok(selector) = Selector::parse(selector_str) {
                if let Some(main_content) = document.select(&selector).next() {
                    let text = extract_text_from_element(&main_content, options);
                    return Ok(clean_text(&text));
                }
            }
        }
    }

    // If no main content found or not requested, use the body
    if let Ok(body_selector) = Selector::parse("body") {
        if let Some(body) = document.select(&body_selector).next() {
            let text = extract_text_from_element(&body, options);
            return Ok(clean_text(&text));
        }
    }

    // Fallback: return the entire document text
    Ok(clean_text(
        &document.root_element().text().collect::<String>(),
    ))
}

/// Extract text from an HTML element with options
fn extract_text_from_element(
    element: &scraper::ElementRef,
    options: &ContentExtractionOptions,
) -> String {
    let mut text_parts = Vec::new();

    // Extract text content based on options
    for node in element.children() {
        if let Some(element_ref) = scraper::ElementRef::wrap(node) {
            let tag_name = element_ref.value().name();

            match tag_name {
                "a" if options.include_links => {
                    // Include link text if enabled
                    let link_text = element_ref.text().collect::<String>();
                    if !link_text.trim().is_empty() {
                        text_parts.push(link_text);
                    }
                }
                "img" if options.include_images => {
                    // Include alt text if enabled
                    if let Some(alt_text) = element_ref.value().attr("alt") {
                        if !alt_text.trim().is_empty() {
                            text_parts.push(format!("[Image: {}]", alt_text));
                        }
                    }
                }
                "a" | "img" => {
                    // Skip links and images if not enabled
                    continue;
                }
                _ => {
                    // For other elements, recursively extract text
                    let child_text = extract_text_from_element(&element_ref, options);
                    if !child_text.trim().is_empty() {
                        text_parts.push(child_text);
                    }
                }
            }
        } else if let Some(text_node) = node.value().as_text() {
            // Direct text node
            let text = text_node.trim();
            if !text.is_empty() {
                text_parts.push(text.to_string());
            }
        }
    }

    // If no specific content found, fall back to simple text extraction
    if text_parts.is_empty() {
        element.text().collect::<String>()
    } else {
        text_parts.join(" ")
    }
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
