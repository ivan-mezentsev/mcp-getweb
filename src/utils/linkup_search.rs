use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::debug;

const LINKUP_SEARCH_URL: &str = "https://api.linkup.so/v1/search";

static HTTP_CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to create HTTP client")
});

#[derive(Debug, Clone)]
pub struct LinkupSearchRequest {
    pub query: String,
    pub max_results: u32,
    pub include_domains: Option<Vec<String>>,
    pub from_date: Option<String>,
    pub to_date: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LinkupResultItem {
    pub name: Option<String>,
    pub url: Option<String>,
    pub content: Option<String>,
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LinkupSourceItem {
    pub name: Option<String>,
    pub url: Option<String>,
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LinkupSearchResponse {
    #[serde(default)]
    pub results: Vec<LinkupResultItem>,
    pub answer: Option<String>,
    pub sources: Option<Vec<LinkupSourceItem>>,
}

#[derive(Debug, Serialize)]
struct LinkupSearchPayload<'a> {
    q: &'a str,
    depth: &'a str,
    #[serde(rename = "outputType")]
    output_type: &'a str,
    #[serde(rename = "includeImages")]
    include_images: bool,
    #[serde(rename = "maxResults")]
    max_results: u32,
    #[serde(rename = "includeDomains", skip_serializing_if = "Option::is_none")]
    include_domains: Option<&'a [String]>,
    #[serde(rename = "fromDate", skip_serializing_if = "Option::is_none")]
    from_date: Option<&'a str>,
    #[serde(rename = "toDate", skip_serializing_if = "Option::is_none")]
    to_date: Option<&'a str>,
}

pub async fn search_linkup(api_key: &str, request: &LinkupSearchRequest) -> Result<LinkupSearchResponse> {
    let payload = LinkupSearchPayload {
        q: &request.query,
        depth: "standard",
        output_type: "searchResults",
        include_images: false,
        max_results: request.max_results.max(1),
        include_domains: request.include_domains.as_deref(),
        from_date: request.from_date.as_deref(),
        to_date: request.to_date.as_deref(),
    };

    debug!(
        "Sending Linkup search request: query='{}', maxResults={}, includeDomains={}, fromDate={:?}, toDate={:?}",
        request.query,
        payload.max_results,
        payload.include_domains.map(|v| v.len()).unwrap_or(0),
        payload.from_date,
        payload.to_date
    );

    let response = HTTP_CLIENT
        .post(LINKUP_SEARCH_URL)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .header("User-Agent", "mcp-getweb")
        .json(&payload)
        .send()
        .await?;

    let status = response.status();
    let raw_text = response.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err(anyhow!(
            "Linkup API request failed: {} - {}",
            status,
            raw_text
        ));
    }

    serde_json::from_str::<LinkupSearchResponse>(&raw_text)
        .map_err(|e| anyhow!("Failed to parse Linkup response: {}", e))
}
