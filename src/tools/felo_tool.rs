use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::json;
use tracing::{error, info};

use crate::mcp::types::{CallToolResult, ToolAnnotations, ToolDefinition};
use crate::utils::search_felo::search_felo;

pub static FELO_TOOL_DEFINITION: Lazy<ToolDefinition> = Lazy::new(|| {
    ToolDefinition {
    name: "felo-search".to_string(),
    description: "Search the web for up-to-date technical information like latest releases, security advisories, migration guides, benchmarks, and community insights".to_string(),
    input_schema: json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "The search query or prompt"
            },
            "stream": {
                "type": "boolean",
                "description": "Whether to stream the response (default: false)",
                "default": false
            }
        },
        "required": ["query"]
    }),
    annotations: Some(ToolAnnotations {
        title: Some("Felo AI Search".to_string()),
        read_only_hint: Some(true),
        open_world_hint: Some(true),
    }),
}
});

#[derive(Debug, Deserialize)]
struct FeloParams {
    query: String,
    #[serde(default = "default_false")]
    stream: bool,
}

fn default_false() -> bool {
    false
}

pub struct FeloTool;

impl FeloTool {
    pub fn new() -> Self {
        Self
    }

    pub async fn execute(&self, arguments: Option<serde_json::Value>) -> CallToolResult {
        let params = match arguments {
            Some(args) => match serde_json::from_value::<FeloParams>(args) {
                Ok(params) => params,
                Err(e) => {
                    error!("Invalid Felo parameters: {}", e);
                    return CallToolResult::error(format!("Invalid parameters: {}", e));
                }
            },
            None => {
                return CallToolResult::error("Missing required parameters");
            }
        };

        info!(
            "Searching Felo AI for: \"{}\" (stream: {})",
            params.query, params.stream
        );

        match search_felo(&params.query, params.stream).await {
            Ok(response) => {
                if response.is_empty() {
                    CallToolResult::success("No results found.")
                } else {
                    CallToolResult::success(response)
                }
            }
            Err(e) => {
                error!("Error in Felo search: {}", e);
                CallToolResult::error(format!("Error searching Felo: {}", e))
            }
        }
    }
}
