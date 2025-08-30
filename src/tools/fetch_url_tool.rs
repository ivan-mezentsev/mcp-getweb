use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::json;
use tracing::{error, info};

use crate::mcp::types::{CallToolResult, ToolAnnotations, ToolDefinition};
use crate::utils::duckduckgo_search::{fetch_url_content, ContentExtractionOptions};

pub static FETCH_URL_TOOL_DEFINITION: Lazy<ToolDefinition> = Lazy::new(|| ToolDefinition {
    name: "fetch-url".to_string(),
    description:
        "Fetch the content of a URL and return it as text, with options to control extraction"
            .to_string(),
    input_schema: json!({
        "type": "object",
        "properties": {
            "url": {
                "type": "string",
                "description": "The URL to fetch"
            },
            "maxLength": {
                "type": "integer",
                "description": "Maximum length of content to return (default: 10000)",
                "default": 10000,
                "minimum": 1000,
                "maximum": 50000
            },
            "extractMainContent": {
                "type": "boolean",
                "description": "Whether to attempt to extract main content (default: true)",
                "default": true
            },
            "includeLinks": {
                "type": "boolean",
                "description": "Whether to include link text (default: true)",
                "default": true
            },
            "includeImages": {
                "type": "boolean",
                "description": "Whether to include image alt text (default: true)",
                "default": true
            },
            "excludeTags": {
                "type": "array",
                "description": "Tags to exclude from extraction (default: script, style, etc.)",
                "items": {
                    "type": "string"
                }
            }
        },
        "required": ["url"]
    }),
    annotations: Some(ToolAnnotations {
        title: Some("Fetch URL Content".to_string()),
        read_only_hint: Some(true),
        open_world_hint: Some(true),
    }),
});

#[derive(Debug, Deserialize)]
struct FetchUrlParams {
    url: String,
    #[serde(default = "default_max_length", rename = "maxLength")]
    max_length: usize,
    #[serde(default = "default_true", rename = "extractMainContent")]
    extract_main_content: bool,
    #[serde(default = "default_true", rename = "includeLinks")]
    include_links: bool,
    #[serde(default = "default_true", rename = "includeImages")]
    include_images: bool,
    #[serde(default = "default_exclude_tags", rename = "excludeTags")]
    exclude_tags: Vec<String>,
}

fn default_max_length() -> usize {
    10000
}

fn default_true() -> bool {
    true
}

fn default_exclude_tags() -> Vec<String> {
    vec![
        "script".to_string(),
        "style".to_string(),
        "noscript".to_string(),
        "iframe".to_string(),
        "svg".to_string(),
        "nav".to_string(),
        "footer".to_string(),
        "header".to_string(),
        "aside".to_string(),
    ]
}

pub struct FetchUrlTool;

impl FetchUrlTool {
    pub fn new() -> Self {
        Self
    }

    pub async fn execute(&self, arguments: Option<serde_json::Value>) -> CallToolResult {
        let params = match arguments {
            Some(args) => match serde_json::from_value::<FetchUrlParams>(args) {
                Ok(params) => params,
                Err(e) => {
                    error!("Invalid fetch URL parameters: {}", e);
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

        info!(
            "Fetching content from URL: {} (maxLength: {})",
            params.url, params.max_length
        );

        let options = ContentExtractionOptions {
            extract_main_content: params.extract_main_content,
            include_links: params.include_links,
            include_images: params.include_images,
            exclude_tags: params.exclude_tags,
        };

        match fetch_url_content(&params.url, &options).await {
            Ok(content) => {
                // Truncate content if it's too long
                let truncated_content = if content.len() > params.max_length {
                    format!(
                        "{}... [Content truncated due to length]",
                        &content[..params.max_length]
                    )
                } else {
                    content.clone()
                };

                // Add metadata about the extraction
                let metadata = format!(
                    "\n---\nExtraction settings:\n- URL: {}\n- Main content extraction: {}\n- Links included: {}\n- Images included: {}\n- Content length: {} characters{}\n---",
                    params.url,
                    if params.extract_main_content { "Enabled" } else { "Disabled" },
                    if params.include_links { "Yes" } else { "No" },
                    if params.include_images { "Yes (as alt text)" } else { "No" },
                    content.len(),
                    if content.len() > params.max_length {
                        format!(" (truncated to {})", params.max_length)
                    } else {
                        String::new()
                    }
                );

                CallToolResult::success(format!("{}{}", truncated_content, metadata))
            }
            Err(e) => {
                error!("Error fetching URL {}: {}", params.url, e);
                CallToolResult::error(format!("Error fetching URL: {}", e))
            }
        }
    }
}
