use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::json;
use tracing::{error, info};

use crate::mcp::types::{CallToolResult, ToolAnnotations, ToolDefinition};
use crate::utils::duckduckgo_search::extract_url_metadata;

pub static METADATA_TOOL_DEFINITION: Lazy<ToolDefinition> = Lazy::new(|| ToolDefinition {
    name: "url-metadata".to_string(),
    description: "Extract metadata from a URL (title, description, etc.)".to_string(),
    input_schema: json!({
        "type": "object",
        "properties": {
            "url": {
                "type": "string",
                "description": "The URL to extract metadata from"
            }
        },
        "required": ["url"]
    }),
    annotations: Some(ToolAnnotations {
        title: Some("URL Metadata".to_string()),
        read_only_hint: Some(true),
        open_world_hint: Some(true),
    }),
});

#[derive(Debug, Deserialize)]
struct MetadataParams {
    url: String,
}

pub struct MetadataTool;

impl MetadataTool {
    pub fn new() -> Self {
        Self
    }

    pub async fn execute(&self, arguments: Option<serde_json::Value>) -> CallToolResult {
        let params = match arguments {
            Some(args) => match serde_json::from_value::<MetadataParams>(args) {
                Ok(params) => params,
                Err(e) => {
                    error!("Invalid metadata parameters: {}", e);
                    return CallToolResult::error(format!("Invalid parameters: {}", e));
                }
            },
            None => {
                return CallToolResult::error("Missing required parameters");
            }
        };

        // Validate URL
        if let Err(e) = url::Url::parse(&params.url) {
            return CallToolResult::error(format!("Invalid URL: {}", e));
        }

        info!("Extracting metadata from URL: {}", params.url);

        match extract_url_metadata(&params.url).await {
            Ok(metadata) => {
                // Format the metadata for display
                let formatted_metadata = format!(
                    "## URL Metadata for {}\n\n**Title:** {}\n\n**Description:** {}\n\n**Image:** {}\n\n**Favicon:** {}",
                    params.url,
                    metadata.title,
                    metadata.description,
                    metadata.og_image.as_deref().unwrap_or("None"),
                    metadata.favicon.as_deref().unwrap_or("None")
                );

                CallToolResult::success(formatted_metadata)
            }
            Err(e) => {
                error!("Error extracting metadata from {}: {}", params.url, e);
                CallToolResult::error(format!("Error extracting metadata: {}", e))
            }
        }
    }
}
