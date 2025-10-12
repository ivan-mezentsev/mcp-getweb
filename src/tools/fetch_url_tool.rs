use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::json;
use tracing::{error, info, warn};

use crate::mcp::types::{CallToolResult, ToolAnnotations, ToolDefinition};
use crate::utils::content_guard::{build_error_payload, safe_truncate_utf8};
use crate::utils::readability_extract::{fetch_url_content, ExtractionKind};

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
                "description": "Maximum length of content to return (default: 30000)",
                "default": 30000,
                "minimum": 1000,
                "maximum": 500000
            },
            "extractMainContent": {
                "type": "boolean",
                "description": "Whether to attempt to extract main content (default: true)",
                "default": true
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
}

fn default_max_length() -> usize {
    30000
}

fn default_true() -> bool {
    true
}

// legacy defaults removed

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
        let parsed_url = url::Url::parse(&params.url).expect("validated above");
        match fetch_url_content(&parsed_url, params.extract_main_content).await {
            Ok(res) => {
                let content = &res.text;
                // Truncate content if it's too long
                let truncated_content = if content.len() > params.max_length {
                    safe_truncate_utf8(
                        content,
                        params.max_length,
                        "... [Content truncated due to length]",
                    )
                } else {
                    content.clone()
                };

                // Add metadata about the extraction
                let kind_str = match res.kind {
                    ExtractionKind::HtmlMain => "HTML/Main",
                    ExtractionKind::HtmlFull => "HTML/Full",
                    ExtractionKind::Pdf => "PDF",
                    ExtractionKind::PlainText => "PlainText",
                };
                let ct = res.content_type.as_deref().unwrap_or("unknown");
                let metadata = format!(
                    "\n---\nExtraction settings:\n- URL: {}\n- Main content extraction: {}\n- Main fragment detected: {}\n- Detected content type: {}\n- Extraction kind: {}\n- Content length: {} characters{}\n---",
                    params.url,
                    if params.extract_main_content { "Enabled" } else { "Disabled" },
                    if res.main_fragment_used { "Yes" } else { "No" },
                    ct,
                    kind_str,
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
                let err_str = e.to_string();
                // Detect standardized payloads by inspecting the trailing JSON line starting with '{'
                let mut code: Option<String> = None;
                if let Some(json_line) = err_str
                    .lines()
                    .rev()
                    .find(|l| l.trim_start().starts_with('{'))
                {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_line) {
                        code = v
                            .get("code")
                            .and_then(|c| c.as_str())
                            .map(|s| s.to_string());
                    }
                }

                match code.as_deref() {
                    Some("ERR_FETCH_UNSUPPORTED_BINARY") => {
                        warn!("Refused fetch due to unsupported binary content (policy=binary-guard) for URL {}", params.url);
                        CallToolResult::error(err_str)
                    }
                    Some("ERR_FETCH_HTTP")
                    | Some("ERR_FETCH_PDF_PARSE")
                    | Some("ERR_FETCH_PDF_ENCRYPTED") => {
                        // Pass-through standardized HTTP/PDF errors as-is
                        warn!(
                            "Standardized fetch error for URL {}: {}",
                            params.url,
                            code.unwrap()
                        );
                        CallToolResult::error(err_str)
                    }
                    _ => {
                        // Unknown error — do not leak internal traces in external message
                        error!("Error fetching URL {}: {}", params.url, err_str);
                        let payload = build_error_payload(
                            "ERR_FETCH_UNKNOWN",
                            "An unknown error occurred while fetching the content",
                            json!({
                                "url": params.url,
                                "hint": "Please try again later or provide a different URL."
                            }),
                        );
                        CallToolResult::error(payload)
                    }
                }
            }
        }
    }
}
