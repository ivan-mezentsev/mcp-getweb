use reqwest::{header, Client};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;
use tracing::{debug, error};

#[derive(Error, Debug)]
pub enum JinaReaderError {
    #[error("HTTP request error: {0}")]
    Request(#[from] reqwest::Error),

    #[error("API error: {0}")]
    Api(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[derive(Debug, Serialize)]
struct JinaReaderRequest {
    url: String,
    #[serde(rename = "withLinksummary", skip_serializing_if = "Option::is_none")]
    with_links_summary: Option<bool>,
    #[serde(rename = "withImagesSummary", skip_serializing_if = "Option::is_none")]
    with_images_summary: Option<bool>,
    #[serde(rename = "withGeneratedAlt", skip_serializing_if = "Option::is_none")]
    with_generated_alt: Option<bool>,
    #[serde(rename = "returnFormat", skip_serializing_if = "Option::is_none")]
    return_format: Option<String>,
    #[serde(rename = "noCache", skip_serializing_if = "Option::is_none")]
    no_cache: Option<bool>,
    #[serde(rename = "timeout", skip_serializing_if = "Option::is_none")]
    timeout: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct JinaReaderApiResponse {
    #[allow(dead_code)]
    pub code: u16,
    #[allow(dead_code)]
    pub status: u32,
    pub data: JinaReaderData,
}

#[derive(Debug, Deserialize)]
pub struct JinaReaderData {
    pub url: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub content: Option<String>,
    pub links: Option<HashMap<String, String>>,
    pub images: Option<HashMap<String, String>>,
    #[allow(dead_code)]
    pub usage: Option<JinaUsage>,
}

#[derive(Debug, Deserialize)]
pub struct JinaUsage {
    #[allow(dead_code)]
    pub tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct JinaReaderResponse {
    pub url: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub content: Option<String>,
    pub links: Option<HashMap<String, String>>,
    pub images: Option<HashMap<String, String>>,
}

#[derive(Debug, Default)]
pub struct JinaReaderParams {
    pub with_links_summary: bool,
    pub with_images_summary: bool,
    pub with_generated_alt: bool,
    pub return_format: String,
    pub no_cache: bool,
    pub timeout: u32,
}

pub struct JinaReaderService {
    client: Client,
    api_key: String,
    endpoint: String,
}

impl JinaReaderService {
    pub fn new(api_key: String) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            api_key,
            endpoint: "https://r.jina.ai/".to_string(),
        }
    }

    pub async fn read_url(
        &self,
        url: &str,
        params: &JinaReaderParams,
    ) -> Result<JinaReaderResponse, JinaReaderError> {
        let request_body = JinaReaderRequest {
            url: url.to_string(),
            with_links_summary: if params.with_links_summary {
                Some(true)
            } else {
                None
            },
            with_images_summary: if params.with_images_summary {
                Some(true)
            } else {
                None
            },
            with_generated_alt: if params.with_generated_alt {
                Some(true)
            } else {
                None
            },
            return_format: if params.return_format != "markdown" {
                Some(params.return_format.clone())
            } else {
                None
            },
            no_cache: if params.no_cache { Some(true) } else { None },
            timeout: if params.timeout != 10 {
                Some(params.timeout)
            } else {
                None
            },
        };

        debug!("Sending request to Jina Reader API: {:?}", request_body);

        let mut request_builder = self
            .client
            .post(&self.endpoint)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {}", self.api_key));

        // Add optional headers based on parameters
        if params.with_links_summary {
            request_builder = request_builder.header("X-With-Links-Summary", "true");
        }
        if params.with_images_summary {
            request_builder = request_builder.header("X-With-Images-Summary", "true");
        }
        if params.with_generated_alt {
            request_builder = request_builder.header("X-With-Generated-Alt", "true");
        }
        if params.return_format != "markdown" {
            request_builder = request_builder.header("X-Return-Format", &params.return_format);
        }
        if params.no_cache {
            request_builder = request_builder.header("X-No-Cache", "true");
        }
        if params.timeout != 10 {
            request_builder = request_builder.header("X-Timeout", params.timeout.to_string());
        }

        let response = request_builder
            .json(&serde_json::json!({"url": url}))
            .send()
            .await?
            .error_for_status();

        match response {
            Ok(response) => {
                let response_text = response.text().await?;
                debug!("Received response from Jina Reader API: {}", response_text);

                let api_response = serde_json::from_str::<JinaReaderApiResponse>(&response_text)?;

                // Convert to the expected response format
                let reader_response = JinaReaderResponse {
                    url: api_response.data.url,
                    title: api_response.data.title,
                    description: api_response.data.description,
                    content: api_response.data.content,
                    links: api_response.data.links,
                    images: api_response.data.images,
                };

                Ok(reader_response)
            }
            Err(err) => {
                if let Some(status) = err.status() {
                    error!("Jina Reader API error: Status {}", status);
                    return Err(JinaReaderError::Api(format!("HTTP Status: {}", status)));
                }
                Err(JinaReaderError::Request(err))
            }
        }
    }
}
