use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;
use serde_json::json;
use tracing::{error, info};

use crate::mcp::types::{CallToolResult, ToolAnnotations, ToolDefinition};
use crate::utils::exa_search::{search_exa, ExaSearchRequest, ExaSearchResponse};

pub static EXA_SEARCH_TOOL_DEFINITION: Lazy<ToolDefinition> = Lazy::new(|| ToolDefinition {
    name: "exa_search".to_string(),
    description: "Search the web via Exa API and return relevant results in Markdown.".to_string(),
    input_schema: json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Natural-language search query."
            },
            "maxResults": {
                "type": "integer",
                "description": "Number of results to return (default: 10, max: 25).",
                "default": 10,
                "minimum": 1,
                "maximum": 25
            },
            "publishedDateRange": {
                "type": "object",
                "description": "Published date range filter (optional). Accepts ISO 8601 date-time (RFC3339) or YYYY-MM-DD.",
                "properties": {
                    "fromDate": {
                        "type": "string",
                        "description": "Start published date (inclusive)."
                    },
                    "toDate": {
                        "type": "string",
                        "description": "End published date (inclusive)."
                    }
                }
            },
            "crawlDateRange": {
                "type": "object",
                "description": "Crawl date range filter (optional). Accepts ISO 8601 date-time (RFC3339) or YYYY-MM-DD.",
                "properties": {
                    "fromDate": {
                        "type": "string",
                        "description": "Start crawl date (inclusive)."
                    },
                    "toDate": {
                        "type": "string",
                        "description": "End crawl date (inclusive)."
                    }
                }
            },
            "userLocation": {
                "type": "string",
                "description": "Two-letter ISO country code of the user (e.g., US) (optional)."
            },
            "includeText": {
                "type": "string",
                "description": "Results must contain this exact phrase in webpage text (optional, max 5 words)."
            },
            "excludeText": {
                "type": "string",
                "description": "Exclude results with this exact phrase in webpage text (optional, max 5 words)."
            },
            "domain": {
                "type": "string",
                "description": "Domain filter (optional). Only return results from this domain (e.g., \"arxiv.org\")."
            }
        },
        "required": ["query"]
    }),
    annotations: Some(ToolAnnotations {
        title: Some("Exa Search".to_string()),
        read_only_hint: Some(true),
        open_world_hint: Some(true),
    }),
});

#[derive(Debug, Deserialize)]
struct ExaDateRangeInput {
    #[serde(rename = "fromDate")]
    from_date: Option<String>,
    #[serde(rename = "toDate")]
    to_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExaSearchParams {
    query: String,
    #[serde(default = "default_max_results", rename = "maxResults")]
    max_results: u32,
    #[serde(rename = "publishedDateRange")]
    published_date_range: Option<ExaDateRangeInput>,
    #[serde(rename = "crawlDateRange")]
    crawl_date_range: Option<ExaDateRangeInput>,
    #[serde(rename = "userLocation")]
    user_location: Option<String>,
    #[serde(rename = "includeText")]
    include_text: Option<String>,
    #[serde(rename = "excludeText")]
    exclude_text: Option<String>,
    domain: Option<String>,
}

fn default_max_results() -> u32 {
    10
}

fn date_only_regex() -> &'static Regex {
    static DATE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap());
    &DATE_RE
}

fn rfc3339_regex() -> &'static Regex {
    static RFC3339_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$")
            .unwrap()
    });
    &RFC3339_RE
}

fn country_code_regex() -> &'static Regex {
    static CC_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Za-z]{2}$").unwrap());
    &CC_RE
}

fn normalize_query(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Err("Missing required parameter: query".to_string());
    }

    Ok(value.to_string())
}

fn normalize_max_results(raw: u32) -> u32 {
    raw.clamp(1, 25)
}

fn normalize_domain(raw: Option<String>) -> Option<String> {
    let value = raw?.trim().to_string();
    if value.is_empty() {
        return None;
    }

    let mut domain = value
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .to_string();

    // Cut path/query/fragment
    if let Some((head, _)) = domain.split_once('/') {
        domain = head.to_string();
    }
    if let Some((head, _)) = domain.split_once('?') {
        domain = head.to_string();
    }
    if let Some((head, _)) = domain.split_once('#') {
        domain = head.to_string();
    }

    domain = domain.trim_end_matches('/').to_string();

    if domain.is_empty() {
        None
    } else {
        Some(domain)
    }
}

fn normalize_user_location(raw: Option<String>) -> Result<Option<String>, String> {
    let value = match raw {
        Some(value) => value.trim().to_string(),
        None => return Ok(None),
    };

    if value.is_empty() {
        return Ok(None);
    }

    if !country_code_regex().is_match(&value) {
        return Err("userLocation must be a two-letter ISO country code (e.g., US)".to_string());
    }

    Ok(Some(value.to_uppercase()))
}

fn normalize_phrase(raw: Option<String>, field: &str) -> Result<Option<String>, String> {
    let value = match raw {
        Some(value) => value.trim().to_string(),
        None => return Ok(None),
    };

    if value.is_empty() {
        return Ok(None);
    }

    let word_count = value.split_whitespace().count();
    if word_count == 0 {
        return Ok(None);
    }

    if word_count > 5 {
        return Err(format!(
            "{} must be an exact phrase of up to 5 words (got {} words)",
            field, word_count
        ));
    }

    Ok(Some(value))
}

fn normalize_date_time(raw: Option<String>, field: &str) -> Result<Option<String>, String> {
    let value = match raw {
        Some(value) => value.trim().to_string(),
        None => return Ok(None),
    };

    if value.is_empty() {
        return Ok(None);
    }

    if date_only_regex().is_match(&value) {
        // Convert YYYY-MM-DD to RFC3339 midnight UTC.
        return Ok(Some(format!("{}T00:00:00.000Z", value)));
    }

    if rfc3339_regex().is_match(&value) {
        return Ok(Some(value));
    }

    Err(format!(
        "Invalid {} format. Expected RFC3339 (e.g., 2023-01-01T00:00:00.000Z) or YYYY-MM-DD",
        field
    ))
}

fn normalize_date_range(
    range: Option<ExaDateRangeInput>,
    from_field: &str,
    to_field: &str,
) -> Result<(Option<String>, Option<String>), String> {
    let Some(range) = range else {
        return Ok((None, None));
    };

    let from = normalize_date_time(range.from_date, from_field)?;
    let to = normalize_date_time(range.to_date, to_field)?;

    Ok((from, to))
}

fn escape_markdown(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace('*', "\\*")
        .replace('_', "\\_")
        .replace('{', "\\{")
        .replace('}', "\\}")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('(', "\\(")
        .replace(')', "\\)")
        .replace('#', "\\#")
        .replace('+', "\\+")
        .replace('!', "\\!")
        .replace('|', "\\|")
        .replace('>', "\\>")
}

fn escape_inline_code(value: &str) -> String {
    // For inline code spans we avoid over-escaping to keep output readable.
    // Backticks inside code spans are extremely unlikely for our values (dates/domains/IDs),
    // so we only defensively escape backslashes.
    value.replace('\\', "\\\\")
}

fn display_date(raw: &str) -> String {
    // Exa returns RFC3339 for publishedDate. For display we prefer YYYY-MM-DD.
    // If it's not RFC3339-like, return as-is.
    static RFC3339_DATE_PREFIX: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^(\d{4}-\d{2}-\d{2})T").unwrap()
    });

    let value = raw.trim();
    if let Some(caps) = RFC3339_DATE_PREFIX.captures(value) {
        return caps
            .get(1)
            .map(|m| m.as_str().to_string())
            .unwrap_or_else(|| value.to_string());
    }

    value.to_string()
}

struct ExaSearchFormatParams<'a> {
    query: &'a str,
    max_results: u32,
    domain: Option<&'a str>,
    user_location: Option<&'a str>,
    published_from: Option<&'a str>,
    published_to: Option<&'a str>,
    crawl_from: Option<&'a str>,
    crawl_to: Option<&'a str>,
    include_text: Option<&'a str>,
    exclude_text: Option<&'a str>,
}

fn format_results_markdown(
    params: &ExaSearchFormatParams<'_>,
    response: &ExaSearchResponse,
) -> String {
    let mut lines = Vec::new();

    lines.push(format!(
        "### Exa search results for \"{}\"",
        escape_markdown(params.query)
    ));
    lines.push(String::new());

    if let Some(request_id) = response
        .request_id
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        lines.push(format!(
            "- Request ID: `{}`",
            escape_inline_code(request_id)
        ));
    }

    lines.push(format!("- Max results: `{}`", params.max_results));

    if let Some(domain) = params.domain {
        lines.push(format!("- Domain: `{}`", escape_inline_code(domain)));
    }

    if let Some(user_location) = params.user_location {
        lines.push(format!(
            "- User location: `{}`",
            escape_inline_code(user_location)
        ));
    }

    if params.published_from.is_some() || params.published_to.is_some() {
        let from_display = params
            .published_from
            .map(display_date)
            .unwrap_or_else(|| "-".to_string());
        let to_display = params
            .published_to
            .map(display_date)
            .unwrap_or_else(|| "-".to_string());

        lines.push(format!(
            "- Published date range: from `{}` to `{}`",
            escape_inline_code(&from_display),
            escape_inline_code(&to_display)
        ));
    }

    if params.crawl_from.is_some() || params.crawl_to.is_some() {
        let from_display = params
            .crawl_from
            .map(display_date)
            .unwrap_or_else(|| "-".to_string());
        let to_display = params
            .crawl_to
            .map(display_date)
            .unwrap_or_else(|| "-".to_string());

        lines.push(format!(
            "- Crawl date range: from `{}` to `{}`",
            escape_inline_code(&from_display),
            escape_inline_code(&to_display)
        ));
    }

    if let Some(include_text) = params.include_text {
        lines.push(format!(
            "- Include text: \"{}\"",
            escape_markdown(include_text)
        ));
    }

    if let Some(exclude_text) = params.exclude_text {
        lines.push(format!(
            "- Exclude text: \"{}\"",
            escape_markdown(exclude_text)
        ));
    }

    lines.push(String::new());

    if response.results.is_empty() {
        lines.push("No results found.".to_string());
        return lines.join("\n");
    }

    for (index, item) in response.results.iter().enumerate() {
        let url = item
            .url
            .as_deref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or("#");

        let title = item
            .title
            .as_deref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or(url);

        lines.push(format!(
            "{}. [{}]({})",
            index + 1,
            escape_markdown(title),
            url
        ));

        let mut meta_bits = Vec::new();

        if let Some(published) = item
            .published_date
            .as_deref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            let published_display = display_date(published);
            meta_bits.push(format!(
                "published: `{}`",
                escape_inline_code(&published_display)
            ));
        }

        if let Some(author) = item
            .author
            .as_deref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            meta_bits.push(format!("author: {}", escape_markdown(author)));
        }

        if let Some(score) = item.score {
            meta_bits.push(format!("score: {:.3}", score));
        }

        if !meta_bits.is_empty() {
            lines.push(format!("   {}", meta_bits.join(" | ")));
        }

        lines.push(String::new());
    }

    lines.join("\n")
}

pub struct ExaSearchTool {
    api_key: Option<String>,
}

impl ExaSearchTool {
    pub fn new(api_key: Option<String>) -> Self {
        Self { api_key }
    }

    pub async fn execute(&self, arguments: Option<serde_json::Value>) -> CallToolResult {
        let api_key = match &self.api_key {
            Some(key) => key,
            None => {
                return CallToolResult::error(
                    "Exa Search is not configured. Please set EXA_API_KEY environment variable or use --exa-api-key command line argument."
                        .to_string(),
                );
            }
        };

        let params = match arguments {
            Some(args) => match serde_json::from_value::<ExaSearchParams>(args) {
                Ok(params) => params,
                Err(e) => {
                    error!("Invalid Exa search parameters: {}", e);
                    return CallToolResult::error(format!("Invalid parameters: {}", e));
                }
            },
            None => {
                return CallToolResult::error("Missing required parameters".to_string());
            }
        };

        let query = match normalize_query(&params.query) {
            Ok(query) => query,
            Err(err) => return CallToolResult::error(err),
        };

        let max_results = normalize_max_results(params.max_results);
        let domain = normalize_domain(params.domain);

        let user_location = match normalize_user_location(params.user_location) {
            Ok(value) => value,
            Err(err) => return CallToolResult::error(err),
        };

        let include_text = match normalize_phrase(params.include_text, "includeText") {
            Ok(value) => value,
            Err(err) => return CallToolResult::error(err),
        };

        let exclude_text = match normalize_phrase(params.exclude_text, "excludeText") {
            Ok(value) => value,
            Err(err) => return CallToolResult::error(err),
        };

        let (start_published_date, end_published_date) =
            match normalize_date_range(params.published_date_range, "publishedDateRange.fromDate", "publishedDateRange.toDate") {
                Ok(value) => value,
                Err(err) => return CallToolResult::error(err),
            };

        let (start_crawl_date, end_crawl_date) =
            match normalize_date_range(params.crawl_date_range, "crawlDateRange.fromDate", "crawlDateRange.toDate") {
                Ok(value) => value,
                Err(err) => return CallToolResult::error(err),
            };

        info!(
            "Searching Exa for: '{}' (maxResults {}, domain {}, userLocation {:?}, includeText {}, excludeText {})",
            query,
            max_results,
            domain.as_deref().unwrap_or("-"),
            user_location,
            include_text.as_deref().unwrap_or("-"),
            exclude_text.as_deref().unwrap_or("-")
        );

        let request = ExaSearchRequest {
            query: query.clone(),
            num_results: max_results,
            domain: domain.clone(),
            user_location: user_location.clone(),
            start_published_date: start_published_date.clone(),
            end_published_date: end_published_date.clone(),
            start_crawl_date: start_crawl_date.clone(),
            end_crawl_date: end_crawl_date.clone(),
            include_text: include_text.clone(),
            exclude_text: exclude_text.clone(),
        };

        match search_exa(api_key, &request).await {
            Ok(response) => {
                let format_params = ExaSearchFormatParams {
                    query: &query,
                    max_results,
                    domain: domain.as_deref(),
                    user_location: user_location.as_deref(),
                    published_from: start_published_date.as_deref(),
                    published_to: end_published_date.as_deref(),
                    crawl_from: start_crawl_date.as_deref(),
                    crawl_to: end_crawl_date.as_deref(),
                    include_text: include_text.as_deref(),
                    exclude_text: exclude_text.as_deref(),
                };

                let formatted = format_results_markdown(&format_params, &response);

                CallToolResult::success(formatted)
            }
            Err(e) => {
                error!("Exa search failed: {}", e);
                CallToolResult::error(format!("Exa search failed: {}", e))
            }
        }
    }
}
