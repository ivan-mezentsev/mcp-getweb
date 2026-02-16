use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;
use tracing::{error, info};

use crate::mcp::types::{CallToolResult, ToolAnnotations, ToolDefinition};
use crate::utils::linkup_search::{search_linkup, LinkupSearchRequest, LinkupSearchResponse};

pub static LINKUP_SEARCH_TOOL_DEFINITION: Lazy<ToolDefinition> = Lazy::new(|| ToolDefinition {
    name: "linkup_search".to_string(),
    description: "Search the web via Linkup API and return relevant results in Markdown.".to_string(),
    input_schema: json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Natural-language search query."
            },
            "onlySearchTheseDomains": {
                "type": "array",
                "description": "Only search these domains (optional).",
                "items": {
                    "type": "string"
                }
            },
            "dateFilter": {
                "type": "object",
                "description": "Optional date filter.",
                "properties": {
                    "fromDate": {
                        "type": "string",
                        "description": "Start date in YYYY-MM-DD format."
                    },
                    "toDate": {
                        "type": "string",
                        "description": "End date in YYYY-MM-DD format."
                    }
                }
            },
            "maxResults": {
                "type": "integer",
                "description": "Maximum number of results to return (default: 5).",
                "default": 5,
                "minimum": 1
            }
        },
        "required": ["query"]
    }),
    annotations: Some(ToolAnnotations {
        title: Some("Linkup Search".to_string()),
        read_only_hint: Some(true),
        open_world_hint: Some(true),
    }),
});

#[derive(Debug, Deserialize)]
struct LinkupDateFilterInput {
    #[serde(rename = "fromDate")]
    from_date: Option<String>,
    #[serde(rename = "toDate")]
    to_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LinkupSearchParams {
    query: String,
    #[serde(rename = "onlySearchTheseDomains")]
    only_search_these_domains: Option<Vec<String>>,
    #[serde(rename = "dateFilter")]
    date_filter: Option<LinkupDateFilterInput>,
    #[serde(default = "default_max_results", rename = "maxResults")]
    max_results: u32,
    #[serde(rename = "fromDate")]
    from_date: Option<String>,
    #[serde(rename = "toDate")]
    to_date: Option<String>,
}

fn default_max_results() -> u32 {
    5
}

fn date_regex() -> &'static Regex {
    static DATE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap());
    &DATE_RE
}

fn normalize_query(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Err("Missing required parameter: query".to_string());
    }

    Ok(value.to_string())
}

fn normalize_max_results(raw: u32) -> u32 {
    raw.max(1)
}

fn normalize_domains(raw: Option<Vec<String>>) -> Option<Vec<String>> {
    let domains = raw?;

    let mut seen = HashSet::new();
    let mut normalized = Vec::new();

    for domain in domains {
        let value = domain
            .trim()
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .trim_end_matches('/')
            .to_string();

        if value.is_empty() {
            continue;
        }

        if seen.insert(value.clone()) {
            normalized.push(value);
        }

        if normalized.len() >= 50 {
            break;
        }
    }

    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn normalize_date(raw: Option<String>) -> Result<Option<String>, String> {
    let value = match raw {
        Some(value) => value.trim().to_string(),
        None => return Ok(None),
    };

    if value.is_empty() {
        return Ok(None);
    }

    if !date_regex().is_match(&value) {
        return Err(format!(
            "Invalid date format: {}. Expected YYYY-MM-DD",
            value
        ));
    }

    Ok(Some(value))
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
        .replace('-', "\\-")
        .replace('.', "\\.")
        .replace('!', "\\!")
        .replace('|', "\\|")
        .replace('>', "\\>")
}

fn format_results_markdown(
    query: &str,
    max_results: u32,
    domains: Option<&[String]>,
    from_date: Option<&str>,
    to_date: Option<&str>,
    response: &LinkupSearchResponse,
) -> String {
    let mut lines = Vec::new();

    lines.push(format!(
        "### Linkup search results for \"{}\"",
        escape_markdown(query)
    ));
    lines.push(String::new());
    lines.push(format!("- Max results: `{}`", max_results));

    if let Some(domains) = domains {
        if !domains.is_empty() {
            let joined = domains
                .iter()
                .map(|d| format!("`{}`", escape_markdown(d)))
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("- Only search these domains: {}", joined));
        }
    }

    if from_date.is_some() || to_date.is_some() {
        lines.push(format!(
            "- Date filter: from `{}` to `{}`",
            from_date.unwrap_or("-"),
            to_date.unwrap_or("-")
        ));
    }

    lines.push(String::new());

    if !response.results.is_empty() {
        for (index, item) in response.results.iter().enumerate() {
            let title = item
                .name
                .as_deref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .unwrap_or("Untitled");
            let url = item
                .url
                .as_deref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .unwrap_or("#");
            let content = item
                .content
                .as_deref()
                .or(item.snippet.as_deref())
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .unwrap_or("");

            lines.push(format!(
                "{}. [{}]({})",
                index + 1,
                escape_markdown(title),
                url
            ));

            if !content.is_empty() {
                lines.push(format!("   {}", escape_markdown(content)));
            }

            lines.push(String::new());
        }

        return lines.join("\n");
    }

    if let Some(answer) = response
        .answer
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        lines.push(answer.to_string());
        lines.push(String::new());

        if let Some(sources) = &response.sources {
            if !sources.is_empty() {
                lines.push("#### Sources".to_string());
                lines.push(String::new());

                for (index, source) in sources.iter().enumerate() {
                    let name = source
                        .name
                        .as_deref()
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .unwrap_or("Source");
                    let url = source
                        .url
                        .as_deref()
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .unwrap_or("#");

                    lines.push(format!(
                        "{}. [{}]({})",
                        index + 1,
                        escape_markdown(name),
                        url
                    ));

                    if let Some(snippet) = source
                        .snippet
                        .as_deref()
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                    {
                        lines.push(format!("   {}", escape_markdown(snippet)));
                    }

                    lines.push(String::new());
                }
            }
        }

        return lines.join("\n");
    }

    lines.push("No results found.".to_string());
    lines.join("\n")
}

pub struct LinkupSearchTool {
    api_key: Option<String>,
}

impl LinkupSearchTool {
    pub fn new(api_key: Option<String>) -> Self {
        Self { api_key }
    }

    pub async fn execute(&self, arguments: Option<serde_json::Value>) -> CallToolResult {
        let api_key = match &self.api_key {
            Some(key) => key,
            None => {
                return CallToolResult::error(
                    "Linkup Search is not configured. Please set LINKUP_API_KEY environment variable or use --linkup-api-key command line argument."
                        .to_string(),
                );
            }
        };

        let params = match arguments {
            Some(args) => match serde_json::from_value::<LinkupSearchParams>(args) {
                Ok(params) => params,
                Err(e) => {
                    error!("Invalid Linkup search parameters: {}", e);
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
        let domains = normalize_domains(params.only_search_these_domains);

        let from_date_raw = params
            .date_filter
            .as_ref()
            .and_then(|f| f.from_date.clone())
            .or(params.from_date);
        let to_date_raw = params
            .date_filter
            .as_ref()
            .and_then(|f| f.to_date.clone())
            .or(params.to_date);

        let from_date = match normalize_date(from_date_raw) {
            Ok(value) => value,
            Err(err) => return CallToolResult::error(err),
        };

        let to_date = match normalize_date(to_date_raw) {
            Ok(value) => value,
            Err(err) => return CallToolResult::error(err),
        };

        info!(
            "Searching Linkup for: '{}' (maxResults {}, includeDomains {})",
            query,
            max_results,
            domains.as_ref().map(|v| v.len()).unwrap_or(0)
        );

        let request = LinkupSearchRequest {
            query: query.clone(),
            max_results,
            include_domains: domains.clone(),
            from_date: from_date.clone(),
            to_date: to_date.clone(),
        };

        match search_linkup(api_key, &request).await {
            Ok(response) => {
                let formatted = format_results_markdown(
                    &query,
                    max_results,
                    domains.as_deref(),
                    from_date.as_deref(),
                    to_date.as_deref(),
                    &response,
                );

                CallToolResult::success(formatted)
            }
            Err(e) => {
                error!("Linkup search failed: {}", e);
                CallToolResult::error(format!("Linkup search failed: {}", e))
            }
        }
    }
}
