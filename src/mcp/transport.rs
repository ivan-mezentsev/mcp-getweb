use anyhow::Result;
use futures::{SinkExt, StreamExt};
use serde_json;
use tokio::io::BufReader;
use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec};
use tracing::{debug, error};

use super::types::{McpMessage, McpRequest, McpResponse, McpNotification};

pub struct StdioTransport {
    reader: FramedRead<BufReader<tokio::io::Stdin>, LinesCodec>,
    writer: FramedWrite<tokio::io::Stdout, LinesCodec>,
}

impl StdioTransport {
    pub fn new() -> Self {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();

        let reader = FramedRead::new(BufReader::new(stdin), LinesCodec::new());
        let writer = FramedWrite::new(stdout, LinesCodec::new());

        Self { reader, writer }
    }

    pub async fn read_message(&mut self) -> Result<Option<McpMessage>> {
        match self.reader.next().await {
            Some(Ok(line)) => {
                debug!("Received: {}", line);

                // Parse as generic JSON first
                match serde_json::from_str::<serde_json::Value>(&line) {
                    Ok(value) => {
                        // Check if this is a request (has id) or notification (no id)
                        if let Some(obj) = value.as_object() {
                            if obj.contains_key("id") {
                                // This is a request
                                match serde_json::from_value::<McpRequest>(value) {
                                    Ok(request) => Ok(Some(McpMessage::Request(request))),
                                    Err(e) => {
                                        error!("Failed to parse request: {}", e);
                                        Err(anyhow::anyhow!("Invalid JSON-RPC request: {}", e))
                                    }
                                }
                            } else {
                                // This is a notification
                                match serde_json::from_value::<McpNotification>(value) {
                                    Ok(notification) => Ok(Some(McpMessage::Notification(notification))),
                                    Err(e) => {
                                        error!("Failed to parse notification: {}", e);
                                        Err(anyhow::anyhow!("Invalid JSON-RPC notification: {}", e))
                                    }
                                }
                            }
                        } else {
                            error!("Invalid JSON-RPC message structure");
                            Err(anyhow::anyhow!("Invalid JSON-RPC message structure"))
                        }
                    }
                    Err(e) => {
                        error!("Failed to parse JSON: {}", e);
                        Err(anyhow::anyhow!("Invalid JSON: {}", e))
                    }
                }
            }
            Some(Err(e)) => {
                error!("Error reading from stdin: {}", e);
                Err(anyhow::anyhow!("Transport error: {}", e))
            }
            None => {
                debug!("EOF reached");
                Ok(None)
            }
        }
    }

    pub async fn write_response(&mut self, response: McpResponse) -> Result<()> {
        let json = serde_json::to_string(&response)?;
        debug!("Sending: {}", json);

        self.writer.send(json).await?;

        Ok(())
    }
}
