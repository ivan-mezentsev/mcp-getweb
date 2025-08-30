use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::json;
use tracing::{error, info};

use crate::mcp::types::{CallToolResult, ToolAnnotations, ToolDefinition};
use crate::utils::google_search::{GoogleSearchFilters, GoogleSearchService};

pub static GOOGLE_SEARCH_TOOL_DEFINITION: Lazy<ToolDefinition> = Lazy::new(|| {
    ToolDefinition {
    name: "google-search".to_string(),
    description: "Search Google and return relevant results from the web. This tool finds web pages, articles, and information on specific topics using Google's search engine. Results include titles, snippets, and URLs that can be analyzed further.".to_string(),
    input_schema: json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Search query - be specific and use quotes for exact matches. For best results, use clear keywords and avoid very long queries."
            },
            "num_results": {
                "type": "integer",
                "description": "Number of results to return (default: 5, max: 10). Increase for broader coverage, decrease for faster response.",
                "default": 5,
                "minimum": 1,
                "maximum": 10
            },
            "site": {
                "type": "string",
                "description": "Limit search results to a specific website domain (e.g., \"wikipedia.org\" or \"nytimes.com\")."
            },
            "language": {
                "type": "string",
                "description": "Filter results by language using ISO 639-1 codes (e.g., \"en\" for English, \"es\" for Spanish, \"fr\" for French)."
            },
            "dateRestrict": {
                "type": "string",
                "description": "Filter results by date using Google's date restriction format: \"d[number]\" for past days, \"w[number]\" for past weeks, \"m[number]\" for past months, or \"y[number]\" for past years. Example: \"m6\" for results from the past 6 months."
            },
            "exactTerms": {
                "type": "string",
                "description": "Search for results that contain this exact phrase. This is equivalent to putting the terms in quotes in the search query."
            },
            "resultType": {
                "type": "string",
                "description": "Specify the type of results to return. Options include \"image\" (or \"images\"), \"news\", and \"video\" (or \"videos\"). Default is general web results."
            },
            "page": {
                "type": "integer",
                "description": "Page number for paginated results (starts at 1). Use in combination with resultsPerPage to navigate through large result sets.",
                "default": 1,
                "minimum": 1
            },
            "resultsPerPage": {
                "type": "integer",
                "description": "Number of results to show per page (default: 5, max: 10). Controls how many results are returned for each page.",
                "default": 5,
                "minimum": 1,
                "maximum": 10
            },
            "sort": {
                "type": "string",
                "description": "Sorting method for search results. Options: \"relevance\" (default) or \"date\" (most recent first)."
            }
        },
        "required": ["query"]
    }),
    annotations: Some(ToolAnnotations {
        title: Some("Google Search".to_string()),
        read_only_hint: Some(true),
        open_world_hint: Some(true),
    }),
}
});

#[derive(Debug, Deserialize)]
struct GoogleSearchParams {
    query: String,
    #[serde(default = "default_num_results")]
    num_results: u32,
    site: Option<String>,
    language: Option<String>,
    #[serde(rename = "dateRestrict")]
    date_restrict: Option<String>,
    #[serde(rename = "exactTerms")]
    exact_terms: Option<String>,
    #[serde(rename = "resultType")]
    result_type: Option<String>,
    #[serde(default = "default_page")]
    page: u32,
    #[serde(default = "default_results_per_page", rename = "resultsPerPage")]
    results_per_page: u32,
    sort: Option<String>,
}

fn default_num_results() -> u32 {
    5
}

fn default_page() -> u32 {
    1
}

fn default_results_per_page() -> u32 {
    5
}

pub struct GoogleSearchTool {
    service: Option<GoogleSearchService>,
}

impl GoogleSearchTool {
    pub fn new(api_key: Option<String>, search_engine_id: Option<String>) -> Self {
        let service = if let (Some(key), Some(id)) = (api_key, search_engine_id) {
            Some(GoogleSearchService::new(key, id))
        } else {
            None
        };

        Self { service }
    }

    pub async fn execute(&self, arguments: Option<serde_json::Value>) -> CallToolResult {
        // Check if Google Search is configured
        let service = match &self.service {
            Some(service) => service,
            None => {
                return CallToolResult::error(
                    "Google Search is not configured. Please set GOOGLE_API_KEY and GOOGLE_SEARCH_ENGINE_ID environment variables or use --google-api-key and --google-search-engine-id command line arguments.".to_string()
                );
            }
        };

        let params = match arguments {
            Some(args) => match serde_json::from_value::<GoogleSearchParams>(args) {
                Ok(params) => params,
                Err(e) => {
                    error!("Invalid Google search parameters: {}", e);
                    return CallToolResult::error(format!("Invalid parameters: {}", e));
                }
            },
            None => {
                return CallToolResult::error("Missing required parameters".to_string());
            }
        };

        // Validate parameters
        if params.num_results > 10 {
            return CallToolResult::error("num_results cannot exceed 10".to_string());
        }

        if params.results_per_page > 10 {
            return CallToolResult::error("resultsPerPage cannot exceed 10".to_string());
        }

        info!(
            "Performing Google search for: {} (page {}, {} results)",
            params.query, params.page, params.num_results
        );

        let filters = GoogleSearchFilters {
            site: params.site,
            language: params.language,
            date_restrict: params.date_restrict,
            exact_terms: params.exact_terms,
            result_type: params.result_type,
            page: Some(params.page),
            results_per_page: Some(params.results_per_page),
            sort: params.sort,
        };

        match service
            .search(&params.query, Some(params.num_results), Some(filters))
            .await
        {
            Ok(response) => {
                info!("Found {} results", response.results.len());

                if response.results.is_empty() {
                    return CallToolResult::success(
                        "No results found. Try:\n- Using different keywords\n- Removing quotes from non-exact phrases\n- Using more general terms".to_string()
                    );
                }

                // Format results in a more AI-friendly way
                let mut response_text = format!("Search results for \"{}\":\n\n", params.query);

                // Add category summary if available
                if let Some(ref categories) = response.categories {
                    if !categories.is_empty() {
                        let category_summary: Vec<String> = categories
                            .iter()
                            .map(|c| format!("{} ({})", c.name, c.count))
                            .collect();
                        response_text
                            .push_str(&format!("Categories: {}\n\n", category_summary.join(", ")));
                    }
                }

                // Add pagination info
                if let Some(ref pagination) = response.pagination {
                    if let Some(total_results) = pagination.total_results {
                        response_text.push_str(&format!(
                            "Showing page {} of approximately {} results\n\n",
                            pagination.current_page, total_results
                        ));
                    } else {
                        response_text
                            .push_str(&format!("Showing page {}\n\n", pagination.current_page));
                    }
                }

                // Add each result in a readable format
                for (index, result) in response.results.iter().enumerate() {
                    response_text.push_str(&format!("{}. {}\n", index + 1, result.title));
                    response_text.push_str(&format!("   URL: {}\n", result.link));
                    response_text.push_str(&format!("   {}\n\n", result.snippet));
                }

                // Add navigation hints if pagination exists
                if let Some(ref pagination) = response.pagination {
                    if pagination.has_next_page || pagination.has_previous_page {
                        response_text.push_str("Navigation: ");
                        if pagination.has_previous_page {
                            response_text.push_str(&format!(
                                "Use 'page: {}' for previous results. ",
                                pagination.current_page - 1
                            ));
                        }
                        if pagination.has_next_page {
                            response_text.push_str(&format!(
                                "Use 'page: {}' for more results.",
                                pagination.current_page + 1
                            ));
                        }
                        response_text.push('\n');
                    }
                }

                CallToolResult::success(response_text)
            }
            Err(e) => {
                error!("Google search error: {}", e);
                CallToolResult::error(format!("Google search failed: {}", e))
            }
        }
    }
}
