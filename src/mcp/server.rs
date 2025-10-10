use anyhow::Result;
use tracing::{debug, info, warn};

use super::transport::StdioTransport;
use super::types::*;
use crate::tools::{
    duckduckgo_search_tool::{DuckDuckGoSearchTool, DUCKDUCKGO_SEARCH_TOOL_DEFINITION},
    felo_tool::{FeloTool, FELO_TOOL_DEFINITION},
    fetch_url_tool::{FetchUrlTool, FETCH_URL_TOOL_DEFINITION},
    google_search_tool::{GoogleSearchTool, GOOGLE_SEARCH_TOOL_DEFINITION},
    jina_reader_tool::{JinaReaderTool, JINA_READER_TOOL_DEFINITION},
    metadata_tool::{MetadataTool, METADATA_TOOL_DEFINITION},
    url_fetch_tool::{UrlFetchTool, URL_FETCH_TOOL_DEFINITION},
};

#[derive(Debug, Clone)]
pub struct GoogleSearchConfig {
    pub api_key: String,
    pub search_engine_id: String,
}

pub struct McpServer {
    transport: StdioTransport,
    google_config: Option<GoogleSearchConfig>,
    jina_api_key: Option<String>,
    initialized: bool,
}

impl McpServer {
    pub fn new(google_config: Option<GoogleSearchConfig>, jina_api_key: Option<String>) -> Self {
        Self {
            transport: StdioTransport::new(),
            initialized: false,
            google_config,
            jina_api_key,
        }
    }

    pub async fn start(&mut self) -> Result<()> {
        info!("MCP server started and listening on stdio");

        loop {
            match self.transport.read_message().await? {
                Some(message) => match message {
                    McpMessage::Request(request) => {
                        let response = self.handle_request(request).await;
                        self.transport.write_response(response).await?;
                    }
                    McpMessage::Notification(notification) => {
                        self.handle_notification(notification).await;
                    }
                },
                None => {
                    info!("Client disconnected");
                    break;
                }
            }
        }

        Ok(())
    }

    async fn handle_request(&mut self, request: McpRequest) -> McpResponse {
        let id = Self::ensure_valid_id(request.id.clone());

        match request.method.as_str() {
            "initialize" => self.handle_initialize(request).await,
            "tools/list" => self.handle_list_tools(request).await,
            "tools/call" => self.handle_call_tool(request).await,
            "ping" => self.handle_ping(request).await,
            _ => McpResponse {
                result: None,
                error: Some(McpError {
                    code: -32601,
                    message: "Method not found".to_string(),
                    data: None,
                }),
                jsonrpc: "2.0".to_string(),
                id,
            },
        }
    }

    async fn handle_notification(&mut self, notification: McpNotification) {
        debug!("Received notification: {}", notification.method);

        match notification.method.as_str() {
            "notifications/initialized" => {
                info!("Client initialization completed");
                self.initialized = true;
            }
            "notifications/cancelled" => {
                debug!("Request cancelled notification received");
            }
            _ => {
                warn!("Unknown notification method: {}", notification.method);
            }
        }
    }

    fn ensure_valid_id(id: Option<serde_json::Value>) -> serde_json::Value {
        match id {
            Some(value) => match value {
                serde_json::Value::Null => serde_json::Value::String("0".to_string()),
                _ => value,
            },
            None => serde_json::Value::String("0".to_string()),
        }
    }

    async fn handle_initialize(&mut self, request: McpRequest) -> McpResponse {
        let id = Self::ensure_valid_id(request.id.clone());

        match request.params {
            Some(params) => match serde_json::from_value::<InitializeParams>(params) {
                Ok(_init_params) => {
                    let result = InitializeResult {
                            protocol_version: "2024-11-05".to_string(),
                            server_info: ServerInfo {
                                name: "DuckDuckGo, Google Search & Felo AI Search MCP".to_string(),
                                version: "1.1.1".to_string(),
                                description: Some("A Model Context Protocol server for web search using DuckDuckGo, Google Search, and Felo AI".to_string()),
                            },
                            capabilities: ServerCapabilities {
                                tools: Some(ToolsCapability {
                                    list_changed: Some(true),
                                }),
                                logging: Some(serde_json::json!({})),
                            },
                        };

                    McpResponse {
                        result: Some(serde_json::to_value(result).unwrap()),
                        error: None,
                        jsonrpc: "2.0".to_string(),
                        id,
                    }
                }
                Err(e) => McpResponse {
                    result: None,
                    error: Some(McpError {
                        code: -32602,
                        message: format!("Invalid params: {}", e),
                        data: None,
                    }),
                    jsonrpc: "2.0".to_string(),
                    id,
                },
            },
            None => McpResponse {
                result: None,
                error: Some(McpError {
                    code: -32602,
                    message: "Missing params".to_string(),
                    data: None,
                }),
                jsonrpc: "2.0".to_string(),
                id,
            },
        }
    }

    async fn handle_list_tools(&self, request: McpRequest) -> McpResponse {
        let mut tools = vec![
            DUCKDUCKGO_SEARCH_TOOL_DEFINITION.clone(),
            FETCH_URL_TOOL_DEFINITION.clone(),
            METADATA_TOOL_DEFINITION.clone(),
            FELO_TOOL_DEFINITION.clone(),
            URL_FETCH_TOOL_DEFINITION.clone(),
        ];

        // Add Google Search tool if configured
        if self.google_config.is_some() {
            tools.push(GOOGLE_SEARCH_TOOL_DEFINITION.clone());
        }

        // Add Jina Reader tool if configured
        if self.jina_api_key.is_some() {
            tools.push(JINA_READER_TOOL_DEFINITION.clone());
        }

        let result = ListToolsResult { tools };

        McpResponse {
            result: Some(serde_json::to_value(result).unwrap()),
            error: None,
            jsonrpc: "2.0".to_string(),
            id: Self::ensure_valid_id(request.id),
        }
    }

    async fn handle_call_tool(&self, request: McpRequest) -> McpResponse {
        let id = Self::ensure_valid_id(request.id.clone());

        match request.params {
            Some(params) => match serde_json::from_value::<CallToolParams>(params) {
                Ok(call_params) => {
                    let result = self.execute_tool(call_params).await;
                    McpResponse {
                        result: Some(serde_json::to_value(result).unwrap()),
                        error: None,
                        jsonrpc: "2.0".to_string(),
                        id,
                    }
                }
                Err(e) => McpResponse {
                    result: None,
                    error: Some(McpError {
                        code: -32602,
                        message: format!("Invalid params: {}", e),
                        data: None,
                    }),
                    jsonrpc: "2.0".to_string(),
                    id,
                },
            },
            None => McpResponse {
                result: None,
                error: Some(McpError {
                    code: -32602,
                    message: "Missing params".to_string(),
                    data: None,
                }),
                jsonrpc: "2.0".to_string(),
                id,
            },
        }
    }

    async fn handle_ping(&self, request: McpRequest) -> McpResponse {
        let id = Self::ensure_valid_id(request.id.clone());

        McpResponse {
            result: Some(serde_json::json!({})),
            error: None,
            jsonrpc: "2.0".to_string(),
            id,
        }
    }

    async fn execute_tool(&self, params: CallToolParams) -> CallToolResult {
        match params.name.as_str() {
            "duckduckgo-search" => {
                let tool = DuckDuckGoSearchTool::new();
                tool.execute(params.arguments).await
            }
            "google-search" => {
                let (api_key, search_engine_id) = if let Some(ref config) = self.google_config {
                    (
                        Some(config.api_key.clone()),
                        Some(config.search_engine_id.clone()),
                    )
                } else {
                    (None, None)
                };
                let tool = GoogleSearchTool::new(api_key, search_engine_id);
                tool.execute(params.arguments).await
            }
            "fetch-url" => {
                let tool = FetchUrlTool::new();
                tool.execute(params.arguments).await
            }
            "url-metadata" => {
                let tool = MetadataTool::new();
                tool.execute(params.arguments).await
            }
            "felo-search" => {
                let tool = FeloTool::new();
                tool.execute(params.arguments).await
            }
            "jina-reader" => {
                let tool = JinaReaderTool::new(self.jina_api_key.clone());
                tool.execute(params.arguments).await
            }
            "url-fetch" => {
                let tool = UrlFetchTool::new();
                tool.execute(params.arguments).await
            }
            _ => CallToolResult::error(format!("Tool not found: {}", params.name)),
        }
    }
}
