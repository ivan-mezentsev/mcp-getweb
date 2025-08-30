use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::{debug, error};
use url::Url;

// Constants
const CACHE_DURATION: Duration = Duration::from_secs(5 * 60); // 5 minutes
const MAX_CACHE_ENTRIES: usize = 100;

// Cache for search results
static RESULTS_CACHE: Lazy<Mutex<HashMap<String, CacheEntry>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone)]
struct CacheEntry {
    results: GoogleSearchResponse,
    timestamp: Instant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleSearchFilters {
    pub site: Option<String>,
    pub language: Option<String>,
    pub date_restrict: Option<String>,
    pub exact_terms: Option<String>,
    pub result_type: Option<String>,
    pub page: Option<u32>,
    pub results_per_page: Option<u32>,
    pub sort: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleSearchResult {
    pub title: String,
    pub link: String,
    pub snippet: String,
    pub pagemap: serde_json::Value,
    pub date_published: String,
    pub source: String,
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryInfo {
    pub name: String,
    pub count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchPaginationInfo {
    pub current_page: u32,
    pub total_results: Option<u64>,
    pub results_per_page: u32,
    pub total_pages: Option<u32>,
    pub has_next_page: bool,
    pub has_previous_page: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleSearchResponse {
    pub results: Vec<GoogleSearchResult>,
    pub pagination: Option<SearchPaginationInfo>,
    pub categories: Option<Vec<CategoryInfo>>,
}

// HTTP client
static HTTP_CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to create HTTP client")
});

// Google Custom Search API response structures
#[derive(Debug, Deserialize)]
struct GoogleApiResponse {
    items: Option<Vec<GoogleApiItem>>,
    #[serde(rename = "searchInformation")]
    search_information: Option<GoogleSearchInformation>,
}

#[derive(Debug, Deserialize)]
struct GoogleApiItem {
    title: Option<String>,
    link: Option<String>,
    snippet: Option<String>,
    pagemap: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct GoogleSearchInformation {
    #[serde(rename = "totalResults")]
    total_results: Option<String>,
}

pub struct GoogleSearchService {
    api_key: String,
    search_engine_id: String,
}

impl GoogleSearchService {
    pub fn new(api_key: String, search_engine_id: String) -> Self {
        Self {
            api_key,
            search_engine_id,
        }
    }

    /// Generate a cache key from search parameters
    fn generate_cache_key(
        &self,
        query: &str,
        num_results: u32,
        filters: &Option<GoogleSearchFilters>,
    ) -> String {
        format!("{}-{}-{:?}", query, num_results, filters)
    }

    /// Check if a cache entry is still valid
    fn is_cache_valid(entry: &CacheEntry) -> bool {
        let now = Instant::now();
        now.duration_since(entry.timestamp) < CACHE_DURATION
    }

    /// Store search results in cache
    fn cache_search_results(&self, cache_key: String, response: GoogleSearchResponse) {
        let mut cache = RESULTS_CACHE.lock().unwrap();
        cache.insert(
            cache_key,
            CacheEntry {
                results: response,
                timestamp: Instant::now(),
            },
        );

        // Limit cache size to prevent memory issues
        if cache.len() > MAX_CACHE_ENTRIES {
            // Delete oldest entry
            let oldest_key = cache
                .iter()
                .min_by_key(|(_, entry)| entry.timestamp)
                .map(|(key, _)| key.clone());

            if let Some(key) = oldest_key {
                cache.remove(&key);
            }
        }
    }

    pub async fn search(
        &self,
        query: &str,
        num_results: Option<u32>,
        filters: Option<GoogleSearchFilters>,
    ) -> Result<GoogleSearchResponse> {
        let num_results = num_results.unwrap_or(5).min(10);

        // Generate cache key
        let cache_key = self.generate_cache_key(query, num_results, &filters);

        // Check cache first
        {
            let cache = RESULTS_CACHE.lock().unwrap();
            if let Some(cached_result) = cache.get(&cache_key) {
                if Self::is_cache_valid(cached_result) {
                    debug!("Using cached search results for query: {}", query);
                    return Ok(cached_result.results.clone());
                }
            }
        }

        debug!("Performing Google search for query: {}", query);

        let mut formatted_query = query.to_string();
        let page = filters.as_ref().and_then(|f| f.page).unwrap_or(1);
        let results_per_page = filters
            .as_ref()
            .and_then(|f| f.results_per_page)
            .unwrap_or(num_results)
            .min(10);

        // Apply site filter if provided
        if let Some(ref filters) = filters {
            if let Some(ref site) = filters.site {
                formatted_query.push_str(&format!(" site:{}", site));
            }

            // Apply exact terms if provided
            if let Some(ref exact_terms) = filters.exact_terms {
                formatted_query.push_str(&format!(" \"{}\"", exact_terms));
            }
        }

        // Calculate start index for pagination (Google uses 1-based indexing)
        let start_index = (page - 1) * results_per_page + 1;

        // Convert numbers to strings to avoid temporary value issues
        let num_str = results_per_page.to_string();
        let start_str = start_index.to_string();

        // Apply result type filter if provided - modify query before creating params
        if let Some(ref filters) = filters {
            if let Some(ref result_type) = filters.result_type {
                match result_type.to_lowercase().as_str() {
                    "news" => {
                        formatted_query.push_str(" source:news");
                    }
                    "video" | "videos" => {
                        formatted_query.push_str(" filetype:video OR inurl:video OR inurl:watch");
                    }
                    _ => {}
                }
            }
        }

        // Prepare all string parameters to avoid temporary value issues
        let lang_param;
        let mut params = vec![
            ("key", self.api_key.as_str()),
            ("cx", self.search_engine_id.as_str()),
            ("q", formatted_query.as_str()),
            ("num", num_str.as_str()),
            ("start", start_str.as_str()),
        ];

        // Apply other filters
        if let Some(ref filters) = filters {
            // Apply language filter if provided
            if let Some(ref language) = filters.language {
                lang_param = format!("lang_{}", language);
                params.push(("lr", lang_param.as_str()));
            }

            // Apply date restriction if provided
            if let Some(ref date_restrict) = filters.date_restrict {
                params.push(("dateRestrict", date_restrict.as_str()));
            }

            // Apply result type filter for images
            if let Some(ref result_type) = filters.result_type {
                if matches!(result_type.to_lowercase().as_str(), "image" | "images") {
                    params.push(("searchType", "image"));
                }
            }

            // Apply sorting if provided
            if let Some(ref sort) = filters.sort {
                if sort.to_lowercase() == "date" {
                    params.push(("sort", "date"));
                }
            }
        }

        let url = "https://www.googleapis.com/customsearch/v1";

        debug!(
            "Making request to Google Custom Search API with {} parameters",
            params.len()
        );

        let response = HTTP_CLIENT.get(url).query(&params).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            error!(
                "Google API request failed with status {}: {}",
                status, error_text
            );
            return Err(anyhow!(
                "Google Search API error: {} - {}",
                status,
                error_text
            ));
        }

        let api_response: GoogleApiResponse = response.json().await?;

        // If no items are found, return empty results with pagination info
        let items = api_response.items.unwrap_or_default();

        if items.is_empty() {
            return Ok(GoogleSearchResponse {
                results: vec![],
                pagination: Some(SearchPaginationInfo {
                    current_page: page,
                    results_per_page,
                    total_results: Some(0),
                    total_pages: Some(0),
                    has_next_page: false,
                    has_previous_page: page > 1,
                }),
                categories: Some(vec![]),
            });
        }

        // Map the search results and categorize them
        let results: Vec<GoogleSearchResult> = items
            .into_iter()
            .map(|item| {
                let mut result = GoogleSearchResult {
                    title: item.title.unwrap_or_default(),
                    link: item.link.unwrap_or_default(),
                    snippet: item.snippet.unwrap_or_default(),
                    pagemap: item.pagemap.unwrap_or(serde_json::Value::Null),
                    date_published: String::new(),
                    source: "google_search".to_string(),
                    category: None,
                };

                // Extract date from pagemap if available
                if let Some(metatags) = result.pagemap.get("metatags") {
                    if let Some(metatags_array) = metatags.as_array() {
                        if let Some(first_meta) = metatags_array.first() {
                            if let Some(published_time) = first_meta.get("article:published_time") {
                                if let Some(time_str) = published_time.as_str() {
                                    result.date_published = time_str.to_string();
                                }
                            }
                        }
                    }
                }

                // Add category to the result
                result.category = Some(self.categorize_result(&result));

                result
            })
            .collect();

        // Generate category statistics
        let categories = self.generate_category_stats(&results);

        // Create pagination information
        let total_results = api_response
            .search_information
            .and_then(|info| info.total_results)
            .and_then(|total| total.parse::<u64>().ok())
            .unwrap_or(0);

        let total_pages = if total_results > 0 {
            Some(((total_results as f64) / (results_per_page as f64)).ceil() as u32)
        } else {
            Some(0)
        };

        let pagination = SearchPaginationInfo {
            current_page: page,
            results_per_page,
            total_results: Some(total_results),
            total_pages,
            has_next_page: total_pages.is_some_and(|tp| page < tp),
            has_previous_page: page > 1,
        };

        let response = GoogleSearchResponse {
            results,
            pagination: Some(pagination),
            categories: Some(categories),
        };

        // Cache the results before returning
        self.cache_search_results(cache_key, response.clone());

        Ok(response)
    }

    /// Categorizes a search result based on its content
    fn categorize_result(&self, result: &GoogleSearchResult) -> String {
        if let Ok(url) = Url::parse(&result.link) {
            if let Some(domain) = url.host_str() {
                let domain = domain.replace("www.", "");

                // Check if this is a social media site
                if domain.contains("facebook.com")
                    || domain.contains("twitter.com")
                    || domain.contains("instagram.com")
                    || domain.contains("linkedin.com")
                    || domain.contains("pinterest.com")
                    || domain.contains("tiktok.com")
                    || domain.contains("reddit.com")
                {
                    return "Social Media".to_string();
                }

                // Check if this is a video site
                if domain.contains("youtube.com")
                    || domain.contains("vimeo.com")
                    || domain.contains("dailymotion.com")
                    || domain.contains("twitch.tv")
                {
                    return "Video".to_string();
                }

                // Check if this is a news site
                if domain.contains("news")
                    || domain.contains("cnn.com")
                    || domain.contains("bbc.com")
                    || domain.contains("nytimes.com")
                    || domain.contains("wsj.com")
                    || domain.contains("reuters.com")
                    || domain.contains("bloomberg.com")
                {
                    return "News".to_string();
                }

                // Check if this is an educational site
                if domain.ends_with(".edu")
                    || domain.contains("wikipedia.org")
                    || domain.contains("khan")
                    || domain.contains("course")
                    || domain.contains("learn")
                    || domain.contains("study")
                    || domain.contains("academic")
                {
                    return "Educational".to_string();
                }

                // Check if this is a documentation site
                if domain.contains("docs")
                    || domain.contains("documentation")
                    || domain.contains("developer")
                    || domain.contains("github.com")
                    || domain.contains("gitlab.com")
                    || domain.contains("bitbucket.org")
                    || domain.contains("stackoverflow.com")
                    || result.title.to_lowercase().contains("docs")
                    || result.title.to_lowercase().contains("documentation")
                    || result.title.to_lowercase().contains("api")
                    || result.title.to_lowercase().contains("reference")
                    || result.title.to_lowercase().contains("manual")
                {
                    return "Documentation".to_string();
                }

                // Check if this is a shopping site
                if domain.contains("amazon.com")
                    || domain.contains("ebay.com")
                    || domain.contains("etsy.com")
                    || domain.contains("walmart.com")
                    || domain.contains("shop")
                    || domain.contains("store")
                    || domain.contains("buy")
                {
                    return "Shopping".to_string();
                }

                // Default category based on domain
                let domain_parts: Vec<&str> = domain.split('.').collect();
                if domain_parts.len() >= 2 {
                    let main_domain = domain_parts[domain_parts.len() - 2];
                    return format!(
                        "{}{}",
                        main_domain.chars().next().unwrap_or('O').to_uppercase(),
                        main_domain.chars().skip(1).collect::<String>()
                    );
                }
            }
        }

        "Other".to_string()
    }

    /// Generates category statistics from search results
    fn generate_category_stats(&self, results: &[GoogleSearchResult]) -> Vec<CategoryInfo> {
        let mut category_counts: HashMap<String, u32> = HashMap::new();

        for result in results {
            let category = result
                .category
                .as_ref()
                .unwrap_or(&"Other".to_string())
                .clone();
            *category_counts.entry(category).or_insert(0) += 1;
        }

        let mut categories: Vec<CategoryInfo> = category_counts
            .into_iter()
            .map(|(name, count)| CategoryInfo { name, count })
            .collect();

        // Sort by count in descending order
        categories.sort_by(|a, b| b.count.cmp(&a.count));

        categories
    }
}
