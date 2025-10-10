use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::json;
use tracing::{error, info};

use crate::mcp::types::{CallToolResult, ToolAnnotations, ToolDefinition};
use crate::utils::jina_reader::{JinaReaderParams as ServiceParams, JinaReaderService};

pub static JINA_READER_TOOL_DEFINITION: Lazy<ToolDefinition> = Lazy::new(|| {
    ToolDefinition {
    name: "jina-reader".to_string(),
    description: "Retrieve LLM-friendly content from a single website URL using Jina r.reader API. Useful when you know the specific source of information.".to_string(),
    input_schema: json!({
        "type": "object",
        "properties": {
            "url": {
                "type": "string",
                "description": "The URL to fetch and parse"
            },
            "maxLength": {
                "type": "integer",
                "description": "Maximum length of content to return (default: 10000)",
                "default": 10000,
                "minimum": 1000,
                "maximum": 50000
            },
            "withLinksummary": {
                "type": "boolean",
                "description": "Include links summary at the end of response (default: false)",
                "default": false
            },
            "withImagesSummary": {
                "type": "boolean",
                "description": "Include images summary at the end of response (default: false)",
                "default": false
            },
            "withGeneratedAlt": {
                "type": "boolean",
                "description": "Generate alt text for images lacking captions (default: false)",
                "default": false
            },
            "returnFormat": {
                "type": "string",
                "description": "Format of the returned content (default: markdown)",
                "enum": ["markdown", "html", "text", "screenshot", "pageshot"],
                "default": "markdown"
            },
            "noCache": {
                "type": "boolean",
                "description": "Bypass cache for fresh retrieval (default: false)",
                "default": false
            },
            "timeout": {
                "type": "integer",
                "description": "Maximum time in seconds to wait for webpage to load (default: 10)",
                "default": 10,
                "minimum": 5,
                "maximum": 30
            }
        },
        "required": ["url"]
    }),
    annotations: Some(ToolAnnotations {
        title: Some("Jina Reader".to_string()),
        read_only_hint: Some(true),
        open_world_hint: Some(true),
    }),
}
});

#[derive(Debug, Deserialize)]
struct JinaReaderParams {
    url: String,
    #[serde(default = "default_max_length", rename = "maxLength")]
    max_length: usize,
    #[serde(default = "default_false", rename = "withLinksummary")]
    with_links_summary: bool,
    #[serde(default = "default_false", rename = "withImagesSummary")]
    with_images_summary: bool,
    #[serde(default = "default_false", rename = "withGeneratedAlt")]
    with_generated_alt: bool,
    #[serde(default = "default_return_format", rename = "returnFormat")]
    return_format: String,
    #[serde(default = "default_false", rename = "noCache")]
    no_cache: bool,
    #[serde(default = "default_timeout")]
    timeout: u32,
}

fn default_max_length() -> usize {
    10000
}

fn default_false() -> bool {
    false
}

fn default_return_format() -> String {
    "markdown".to_string()
}

fn default_timeout() -> u32 {
    10
}

pub struct JinaReaderTool {
    service: Option<JinaReaderService>,
}

impl JinaReaderTool {
    pub fn new(api_key: Option<String>) -> Self {
        let service = api_key.map(JinaReaderService::new);
        Self { service }
    }

    pub async fn execute(&self, arguments: Option<serde_json::Value>) -> CallToolResult {
        let service = match &self.service {
            Some(service) => service,
            None => {
                return CallToolResult::error(
                    "Jina Reader API key not configured. Set JINA_API_KEY environment variable."
                        .to_string(),
                );
            }
        };

        let params = match arguments {
            Some(args) => match serde_json::from_value::<JinaReaderParams>(args) {
                Ok(params) => params,
                Err(e) => {
                    error!("Invalid Jina Reader parameters: {}", e);
                    return CallToolResult::error(format!("Invalid parameters: {}", e));
                }
            },
            None => {
                return CallToolResult::error("Missing required parameters".to_string());
            }
        };

        // Validate URL
        if let Err(e) = url::Url::parse(&params.url) {
            return CallToolResult::error(format!("Invalid URL: {}", e));
        }

        info!(
            "Fetching content from URL using Jina Reader: {} (format: {}, maxLength: {})",
            params.url, params.return_format, params.max_length
        );

        let service_params = ServiceParams {
            with_links_summary: params.with_links_summary,
            with_images_summary: params.with_images_summary,
            with_generated_alt: params.with_generated_alt,
            return_format: params.return_format.clone(),
            no_cache: params.no_cache,
            timeout: params.timeout,
        };

        match service.read_url(&params.url, &service_params).await {
            Ok(response) => {
                // Truncate content if it's too long
                let content = response.content.unwrap_or_default();
                let truncated_content = if content.len() > params.max_length {
                    format!(
                        "{}... [Content truncated due to length]",
                        &content[..params.max_length]
                    )
                } else {
                    content.clone()
                };

                // Build result with metadata
                let mut result = format!("# {}", response.title.unwrap_or("Untitled".to_string()));

                if let Some(description) = response.description {
                    result.push_str(&format!("\n\n**Description:** {}\n", description));
                }

                result.push_str(&format!(
                    "\n**URL:** {}\n\n",
                    response.url.unwrap_or(params.url.clone())
                ));
                result.push_str(&truncated_content);

                // Add links summary if requested and available
                if params.with_links_summary {
                    if let Some(links) = response.links {
                        result.push_str("\n\n## Links\n");
                        for (title, url) in links {
                            result.push_str(&format!("- [{}]({})\n", title, url));
                        }
                    }
                }

                // Add images summary if requested and available
                if params.with_images_summary {
                    if let Some(images) = response.images {
                        result.push_str("\n\n## Images\n");
                        for (alt, url) in images {
                            result.push_str(&format!("- ![{}]({})\n", alt, url));
                        }
                    }
                }

                // Add extraction metadata
                let metadata = format!(
                    "\n---\n**Extraction Info:**\n- Format: {}\n- Original length: {} characters{}\n- Links summary: {}\n- Images summary: {}\n---",
                    params.return_format,
                    content.len(),
                    if content.len() > params.max_length {
                        format!(" (truncated to {})", params.max_length)
                    } else {
                        String::new()
                    },
                    if params.with_links_summary { "Included" } else { "Not included" },
                    if params.with_images_summary { "Included" } else { "Not included" }
                );

                result.push_str(&metadata);
                CallToolResult::success(result)
            }
            Err(e) => {
                error!("Error reading URL with Jina Reader {}: {}", params.url, e);
                CallToolResult::error(format!("Error reading URL: {}", e))
            }
        }
    }
}
