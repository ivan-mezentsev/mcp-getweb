# MCP-GetWeb

[![npm version](https://img.shields.io/npm/v/mcp-getweb)](https://www.npmjs.com/package/mcp-getweb) [![npm downloads](https://img.shields.io/npm/dm/mcp-getweb)](https://www.npmjs.com/package/mcp-getweb)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![GitHub issues](https://img.shields.io/github/issues/ivan-mezentsev/mcp-getweb.svg)](https://github.com/ivan-mezentsev/mcp-getweb/issues)

A Model Context Protocol (MCP) server that provides web search and content extraction capabilities.

## Quick Start

- [Google API](https://support.google.com/googleapi/answer/6158862?hl=en)
- [Google Search Engine ID](https://support.google.com/programmable-search/answer/12499034?hl=en)
- [Jina AI](https://jina.ai)

```json
{
  "mcpServers": {
    "getweb": {
      "command": "npx",
      "args": [
        "mcp-getweb"
      ],
      "type": "stdio",
      "env": {
        "GOOGLE_API_KEY": "XXXXXXXXX",
        "GOOGLE_SEARCH_ENGINE_ID": "XXXXXXXXX",
        "JINA_API_KEY": "jina_XXXXXXXXX"
      }
    }
  }
}
```

## Features

### 1) DuckDuckGo Search (`duckduckgo-search`)

Search the web using DuckDuckGo with HTML scraping.

Parameters:

- `query` (string, required): The search query
- `page` (integer, optional): Page number (default: 1, min: 1)
- `numResults` (integer, optional): Number of results to return (default: 10, min: 1, max: 20)

### 2) Google Search (`google-search`)

Search Google and return relevant results using the Programmable Search Engine.

Parameters:

- `query` (string, required): Search query; quotes enable exact matches
- `num_results` (integer, optional): Total results to return (default: 5, max: 10)
- `site` (string, optional): Restrict to a specific site/domain (e.g., `wikipedia.org`)
- `language` (string, optional): ISO 639-1 language code (e.g., `en`, `es`)
- `dateRestrict` (string, optional): Date filter, e.g., `d7`, `w4`, `m6`, `y1`
- `exactTerms` (string, optional): Exact phrase that must appear
- `resultType` (string, optional): Result type: `image`|`images`|`news`|`video`|`videos`
- `page` (integer, optional): Page number for pagination (default: 1, min: 1)
- `resultsPerPage` (integer, optional): Results per page (default: 5, max: 10)
- `sort` (string, optional): Sort order, `relevance` (default) or `date`

Note: Requires `GOOGLE_API_KEY` and `GOOGLE_SEARCH_ENGINE_ID` to be set.

### 3) Felo AI Search (`felo-search`)

AI-powered search with contextual responses for up-to-date technical information (releases, advisories, migrations, benchmarks, community insights).

Parameters:

- `query` (string, required): The search query or prompt
- `stream` (boolean, optional): Whether to stream the response (default: false)

### 4) URL Content Fetcher (`fetch-url`)

Fetch the clean content of a URL and return it as text.

Parameters:

- `url` (string, required): The URL to fetch
- `maxLength` (integer, optional): Maximum content length (default: 30000, min: 1000, max: 500000)
- `extractMainContent` (boolean, optional): Attempt to extract main content when HTML (default: true)

### 5) URL Metadata Extractor (`url-metadata`)

Extract metadata (title, description, image, favicon) from a URL.

Parameters:

- `url` (string, required): The URL to extract metadata from

### 6) URL Fetch to Markdown (`url-fetch`)

Fetch web pages and convert them to Markdown. Handles HTML, plaintext, and JSON (pretty-printed in a fenced block).

Parameters:

- `url` (string, required): The URL to fetch and convert to Markdown

### 7) Jina Reader (`jina-reader`)

Retrieve LLM-friendly content from a URL using Jina r.reader with optional summaries and formats.

Parameters:

- `url` (string, required): The URL to fetch and parse
- `maxLength` (integer, optional): Maximum output length (default: 10000, min: 1000, max: 50000)
- `withLinksummary` (boolean, optional): Include links summary (default: false)
- `withImagesSummary` (boolean, optional): Include images summary (default: false)
- `withGeneratedAlt` (boolean, optional): Generate alt text for images (default: false)
- `returnFormat` (string, optional): `markdown` (default) | `html` | `text` | `screenshot` | `pageshot`
- `noCache` (boolean, optional): Bypass cache (default: false)
- `timeout` (integer, optional): Max seconds to wait (default: 10, min: 5, max: 30)

Note: Requires `JINA_API_KEY` to be set.

## Acknowledgments

- Model Context Protocol specification by Anthropic
- DuckDuckGo for providing a privacy-focused web search experience
- Google Programmable Search Engine and Custom Search JSON API
- Jina AI r.reader API for high-quality content extraction
- Felo AI for up-to-date, developer-focused search insights
- Rust ecosystem and crates that power this server:
  - tokio, reqwest, serde, serde_json, tracing, tracing-subscriber, clap
  - html2text, chardetng, encoding_rs, scraper, html5ever, markup5ever_rcdom, regex, once_cell, futures, async-stream
  - url, uuid, thiserror, tokio-util, rand, urlencoding
- The broader MCP community for guidance, examples, and discussions

## Support

If you encounter any issues or have questions, please [open an issue](https://github.com/ivan-mezentsev/mcp-getweb/issues) on GitHub.
