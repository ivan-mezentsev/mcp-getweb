use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::json;
use tracing::{error, info};

use crate::mcp::types::{CallToolResult, ToolAnnotations, ToolDefinition};
use crate::utils::duckduckgo_search::duckduckgo_search;

pub static DUCKDUCKGO_SEARCH_TOOL_DEFINITION: Lazy<ToolDefinition> = Lazy::new(|| ToolDefinition {
    name: "duckduckgo-search".to_string(),
    description: "Search the web using DuckDuckGo and return results".to_string(),
    input_schema: json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "The search query"
            },
            "page": {
                "type": "integer",
                "description": "Page number (default: 1)",
                "default": 1,
                "minimum": 1
            },
            "numResults": {
                "type": "integer",
                "description": "Number of results to return (default: 10)",
                "default": 10,
                "minimum": 1,
                "maximum": 20
            }
        },
        "required": ["query"]
    }),
    annotations: Some(ToolAnnotations {
        title: Some("DuckDuckGo Search".to_string()),
        read_only_hint: Some(true),
        open_world_hint: Some(true),
    }),
});

#[derive(Debug, Deserialize)]
struct DuckDuckGoSearchParams {
    query: String,
    #[serde(default = "default_page")]
    page: u32,
    #[serde(default = "default_num_results", rename = "numResults")]
    num_results: u32,
}

fn default_page() -> u32 {
    1
}

fn default_num_results() -> u32 {
    10
}

pub struct DuckDuckGoSearchTool;

impl DuckDuckGoSearchTool {
    pub fn new() -> Self {
        Self
    }

    pub async fn execute(&self, arguments: Option<serde_json::Value>) -> CallToolResult {
        let params = match arguments {
            Some(args) => match serde_json::from_value::<DuckDuckGoSearchParams>(args) {
                Ok(params) => params,
                Err(e) => {
                    error!("Invalid search parameters: {}", e);
                    return CallToolResult::error(format!("Invalid parameters: {}", e));
                }
            },
            None => {
                return CallToolResult::error("Missing required parameters");
            }
        };

        // Validate parameters
        if params.num_results > 20 {
            return CallToolResult::error("numResults cannot exceed 20");
        }

        info!(
            "Searching for: {} (page {}, {} results)",
            params.query, params.page, params.num_results
        );

        match duckduckgo_search(&params.query, params.page, params.num_results).await {
            Ok(results) => {
                info!("Found {} results", results.len());

                if results.is_empty() {
                    return CallToolResult::success("No results found.");
                }

                // Format the results for display
                let formatted_results = results
                    .iter()
                    .enumerate()
                    .map(|(index, result)| {
                        format!(
                            "{}. [{}]({})\n   {}",
                            index + 1,
                            result.title,
                            result.url,
                            result.snippet
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");

                CallToolResult::success(formatted_results)
            }
            Err(e) => {
                error!("Search error: {}", e);
                CallToolResult::error(format!("Search failed: {}", e))
            }
        }
    }
}
