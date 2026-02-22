use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::debug;

const EXA_SEARCH_URL: &str = "https://api.exa.ai/search";

static HTTP_CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to create HTTP client")
});

#[derive(Debug, Clone)]
pub struct ExaSearchRequest {
    pub query: String,
    pub num_results: u32,
    pub domain: Option<String>,
    pub user_location: Option<String>,
    pub start_published_date: Option<String>,
    pub end_published_date: Option<String>,
    pub start_crawl_date: Option<String>,
    pub end_crawl_date: Option<String>,
    pub include_text: Option<String>,
    pub exclude_text: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExaSearchResultItem {
    pub title: Option<String>,
    pub url: Option<String>,
    pub score: Option<f64>,
    #[serde(rename = "publishedDate")]
    pub published_date: Option<String>,
    pub author: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExaSearchResponse {
    #[serde(default)]
    pub results: Vec<ExaSearchResultItem>,
    #[serde(rename = "requestId")]
    pub request_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct ExaSearchPayload<'a> {
    query: &'a str,
    #[serde(rename = "type")]
    search_type: &'a str,
    #[serde(rename = "numResults")]
    num_results: u32,

    #[serde(rename = "userLocation", skip_serializing_if = "Option::is_none")]
    user_location: Option<&'a str>,

    #[serde(rename = "includeDomains", skip_serializing_if = "Option::is_none")]
    include_domains: Option<Vec<&'a str>>,

    #[serde(rename = "startPublishedDate", skip_serializing_if = "Option::is_none")]
    start_published_date: Option<&'a str>,
    #[serde(rename = "endPublishedDate", skip_serializing_if = "Option::is_none")]
    end_published_date: Option<&'a str>,

    #[serde(rename = "startCrawlDate", skip_serializing_if = "Option::is_none")]
    start_crawl_date: Option<&'a str>,
    #[serde(rename = "endCrawlDate", skip_serializing_if = "Option::is_none")]
    end_crawl_date: Option<&'a str>,

    #[serde(rename = "includeText", skip_serializing_if = "Option::is_none")]
    include_text: Option<Vec<&'a str>>,
    #[serde(rename = "excludeText", skip_serializing_if = "Option::is_none")]
    exclude_text: Option<Vec<&'a str>>,
}

pub async fn search_exa(api_key: &str, request: &ExaSearchRequest) -> Result<ExaSearchResponse> {
    // Hardcoded per requirements:
    // - category: omitted
    // - type: auto
    // - contents: omitted (no highlights/text)
    // - livecrawl/maxAgeHours: omitted (no content retrieval requested)
    let payload = ExaSearchPayload {
        query: &request.query,
        search_type: "auto",
        num_results: request.num_results.clamp(1, 25),
        user_location: request.user_location.as_deref(),
        include_domains: request.domain.as_deref().map(|d| vec![d]),
        start_published_date: request.start_published_date.as_deref(),
        end_published_date: request.end_published_date.as_deref(),
        start_crawl_date: request.start_crawl_date.as_deref(),
        end_crawl_date: request.end_crawl_date.as_deref(),
        include_text: request.include_text.as_deref().map(|s| vec![s]),
        exclude_text: request.exclude_text.as_deref().map(|s| vec![s]),
    };

    debug!(
        "Sending Exa search request: query='{}', numResults={}, domain={:?}, userLocation={:?}, published=({:?}..{:?}), crawl=({:?}..{:?}), includeText={:?}, excludeText={:?}",
        request.query,
        payload.num_results,
        request.domain,
        request.user_location,
        request.start_published_date,
        request.end_published_date,
        request.start_crawl_date,
        request.end_crawl_date,
        request.include_text,
        request.exclude_text
    );

    let response = HTTP_CLIENT
        .post(EXA_SEARCH_URL)
        .header("x-api-key", api_key)
        .header("Content-Type", "application/json")
        .header("User-Agent", "mcp-getweb")
        .json(&payload)
        .send()
        .await?;

    let status = response.status();
    let raw_text = response.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err(anyhow!(
            "Exa API request failed: {} - {}",
            status,
            raw_text
        ));
    }

    serde_json::from_str::<ExaSearchResponse>(&raw_text)
        .map_err(|e| anyhow!("Failed to parse Exa response: {}", e))
}
