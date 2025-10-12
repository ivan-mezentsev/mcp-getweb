use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::Client;
use scraper::{Html, Selector};
use std::borrow::Cow;
use tracing::{info, warn};

// Firefox ESR User-Agent string to reduce server-side variance
pub const FIREFOX_UA: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:115.0) Gecko/20100101 Firefox/115.0";

/// Kind of extracted textual content
#[derive(Debug, Clone, Copy)]
pub enum ExtractionKind {
    /// Main content parsed from HTML via selectors + html2text
    HtmlMain,
    /// Full HTML document converted via html2text without main selection
    HtmlFull,
    /// Text extracted from PDF bytes
    Pdf,
    /// Plain decoded text from non-HTML textual content
    PlainText,
}

/// Structured result of extraction
#[derive(Debug, Clone)]
pub struct ExtractedContent {
    pub text: String,
    pub content_type: Option<String>,
    pub kind: ExtractionKind,
    pub main_fragment_used: bool,
}

// Local HTTP client with 30s timeout
static HTTP_CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("Failed to create HTTP client")
});

/// Fetch textual content from the given URL.
/// Implements HTTP, binary/PDF guards, decoding with chardetng/encoding_rs, logging and HTML main-content extraction.
pub async fn fetch_url_content(url: &url::Url, extract_main: bool) -> Result<ExtractedContent> {
    use crate::utils::content_guard::{build_error_payload, detect_binary, BinaryDetection};
    use crate::utils::pdf::{extract_text_from_pdf_mem, is_pdf};

    // Start fetch logging
    info!(target: "readability_extract", url = %url, "Starting HTTP fetch");

    // Execute request with fixed Firefox UA
    let response = HTTP_CLIENT
        .get(url.as_str())
        .header("User-Agent", FIREFOX_UA)
        .send()
        .await
        .map_err(|e| {
            // Standardize network/timeout as HTTP error for tool layer
            let payload = build_error_payload(
                "ERR_FETCH_HTTP",
                "Network error during HTTP fetch",
                serde_json::json!({
                    "url": url.as_str(),
                    "hint": "Please verify the URL or try again later.",
                    "error": e.to_string()
                }),
            );
            warn!(target: "readability_extract", url = %url, "HTTP transport error: {}", e);
            anyhow!(payload)
        })?;

    // Non-success HTTP status => standardized error payload
    if !response.status().is_success() {
        let status = response.status();
        let code_num = status.as_u16();
        let reason = status.canonical_reason().unwrap_or("Unknown error");
        let message = format!("HTTP error {}: {}", code_num, reason);
        let payload = build_error_payload(
            "ERR_FETCH_HTTP",
            &message,
            serde_json::json!({
                "url": url.as_str(),
                "httpStatus": code_num,
                "reason": reason,
                "hint": if code_num == 404 { "The resource was not found (404)." } else { "Please verify the URL and try again." }
            }),
        );
        warn!(target: "readability_extract", url = %url, status = code_num, "HTTP non-success status");
        return Err(anyhow!(payload));
    }

    // Capture Content-Type header early
    let content_type_header = response
        .headers()
        .get("content-type")
        .and_then(|ct| ct.to_str().ok())
        .map(|s| s.to_string());

    // Read body bytes with standardized error payload and warn logging
    let body_bytes = response.bytes().await.map_err(|e| {
        let payload = build_error_payload(
            "ERR_FETCH_HTTP",
            "Failed to read HTTP response body",
            serde_json::json!({
                "url": url.as_str(),
                "hint": "The server closed connection or returned invalid body.",
                "error": e.to_string()
            }),
        );
        warn!(target: "readability_extract", url = %url, "Body read failed: {}", e);
        anyhow!(payload)
    })?;
    let size = body_bytes.len();
    info!(target: "readability_extract", url = %url, size = size, ct = ?content_type_header, "HTTP fetch completed");

    // Head slice for magic detection
    let head_len = std::cmp::min(512, body_bytes.len());
    let head = &body_bytes[..head_len];

    // PDF handling with size guard
    const PDF_LIMIT_BYTES: u64 = 500 * 1024 * 1024; // 500 MiB
    if is_pdf(content_type_header.as_deref(), head) {
        if (size as u64) > PDF_LIMIT_BYTES {
            let payload = build_error_payload(
                "ERR_FETCH_PDF_PARSE",
                "PDF exceeds the allowed size limit",
                serde_json::json!({
                    "url": url.as_str(),
                    "contentType": content_type_header.clone().unwrap_or_else(|| "unknown".to_string()),
                    "size": size,
                    "limit": PDF_LIMIT_BYTES,
                }),
            );
            info!(target: "readability_extract", url = %url, size = size, limit = PDF_LIMIT_BYTES, "PDF too large; refusing");
            return Err(anyhow!(payload));
        }

        info!(target: "readability_extract", url = %url, size = size, "Starting PDF text extraction");
        let started = std::time::Instant::now();
        match extract_text_from_pdf_mem(&body_bytes) {
            Ok(text) => {
                info!(target: "readability_extract", url = %url, elapsed_ms = started.elapsed().as_millis() as u64, "PDF extraction succeeded");
                return Ok(ExtractedContent {
                    text,
                    content_type: content_type_header.clone(),
                    kind: ExtractionKind::Pdf,
                    main_fragment_used: false,
                });
            }
            Err(err) => {
                let err_str = err.to_string().to_ascii_lowercase();
                let (code, message, hint) =
                    if err_str.contains("encrypt") || err_str.contains("password") {
                        (
                            "ERR_FETCH_PDF_ENCRYPTED",
                            "Encrypted PDF is not supported",
                            "Try providing an unencrypted PDF or remove password protection",
                        )
                    } else {
                        (
                            "ERR_FETCH_PDF_PARSE",
                            "Failed to parse PDF content",
                            "Try another file or re-save the PDF to simplify its structure",
                        )
                    };
                let payload = build_error_payload(
                    code,
                    message,
                    serde_json::json!({
                        "url": url.as_str(),
                        "contentType": content_type_header.clone().unwrap_or_else(|| "unknown".to_string()),
                        "size": size,
                        "hint": hint,
                    }),
                );
                warn!(target: "readability_extract", url = %url, code = code, "PDF extraction failed");
                return Err(anyhow!(payload));
            }
        }
    }

    // Binary guard for non-PDF binaries
    match detect_binary(content_type_header.as_deref(), head) {
        BinaryDetection::Binary { content_type } => {
            let ct_effective = content_type
                .or(content_type_header.clone())
                .unwrap_or_else(|| "unknown".to_string());
            let payload = build_error_payload(
                "ERR_FETCH_UNSUPPORTED_BINARY",
                "Fetch cannot be performed for this type of content",
                serde_json::json!({
                    "url": url.as_str(),
                    "contentType": ct_effective,
                    "size": size,
                }),
            );
            info!(target: "readability_extract", url = %url, size = size, "Binary content detected; refusing");
            return Err(anyhow!(payload));
        }
        BinaryDetection::Text => {}
    }

    // At this point, treat as textual. Decode to UTF-8 using charset param or chardetng detection.
    info!(target: "readability_extract", url = %url, "Starting textual decode");
    let decoded = decode_to_utf8(&body_bytes, content_type_header.as_deref()).map_err(|e| {
        let payload = build_error_payload(
            "ERR_FETCH_UNKNOWN",
            "Failed to decode textual content to UTF-8",
            serde_json::json!({
                "url": url.as_str(),
                "hint": "The page encoding could not be reliably decoded.",
                "error": e.to_string()
            }),
        );
        warn!(target: "readability_extract", url = %url, "Decoding failed: {}", e);
        anyhow!(payload)
    })?;

    // Decide whether to run HTML main-content conversion based on content type and extract_main flag
    let is_html_ct = content_type_header
        .as_deref()
        .map(|ct| {
            let ct_l = ct.to_ascii_lowercase();
            let main = ct_l.split(';').next().unwrap_or("").trim();
            main == "text/html" || main == "application/xhtml+xml"
        })
        .unwrap_or(false);

    if is_html_ct {
        let started = std::time::Instant::now();
        let (source_html, main_fragment_used, extraction_kind) = if extract_main {
            info!(target: "readability_extract", url = %url, "Starting main-content extraction pipeline");
            match extract_main_content_fragment(&decoded) {
                Some(SelectedHtml::Main(fragment)) => {
                    info!(target: "readability_extract", url = %url, fragment_len = fragment.len(), "Main content fragment selected");
                    (Cow::Owned(fragment), true, ExtractionKind::HtmlMain)
                }
                Some(SelectedHtml::Body(body_html)) => {
                    warn!(target: "readability_extract", url = %url, "Main content fragment not found; using <body> as fallback");
                    (Cow::Owned(body_html), false, ExtractionKind::HtmlFull)
                }
                None => {
                    warn!(target: "readability_extract", url = %url, "Main content fragment not found; using full document");
                    (
                        Cow::Borrowed(decoded.as_str()),
                        false,
                        ExtractionKind::HtmlFull,
                    )
                }
            }
        } else {
            info!(target: "readability_extract", url = %url, "Starting full-document html2text conversion");
            (
                Cow::Borrowed(decoded.as_str()),
                false,
                ExtractionKind::HtmlFull,
            )
        };

        let text = html2text::from_read(source_html.as_bytes(), 120).map_err(|e| {
            let payload = build_error_payload(
                "ERR_FETCH_UNKNOWN",
                "Failed to convert HTML content to text",
                serde_json::json!({
                    "url": url.as_str(),
                    "hint": "The page structure could not be converted into text content.",
                    "error": e.to_string()
                }),
            );
            warn!(target: "readability_extract", url = %url, "html2text conversion failed: {}", e);
            anyhow!(payload)
        })?;

        info!(target: "readability_extract", url = %url, elapsed_ms = started.elapsed().as_millis() as u64, len = text.len(), main_fragment_used = main_fragment_used, "html2text conversion succeeded");
        Ok(ExtractedContent {
            text,
            content_type: content_type_header.clone(),
            kind: extraction_kind,
            main_fragment_used,
        })
    } else {
        info!(target: "readability_extract", url = %url, is_html = is_html_ct, "Skipping html2text; returning decoded text as-is");
        Ok(ExtractedContent {
            text: decoded,
            content_type: content_type_header.clone(),
            kind: ExtractionKind::PlainText,
            main_fragment_used: false,
        })
    }
}

/// Decode bytes into UTF-8 String using charset from Content-Type or chardetng fallback.
/// Returns error if decoding performed replacements (considered data corruption for our purposes).
fn decode_to_utf8(bytes: &[u8], content_type: Option<&str>) -> Result<String> {
    // 1) Try BOM sniff first (covers UTF-8/UTF-16 BOM)
    if let Some((enc, offset)) = encoding_rs::Encoding::for_bom(bytes) {
        let (cow, _used, had_errors) = enc.decode(&bytes[offset..]);
        if had_errors {
            anyhow::bail!("decoding had errors after BOM sniff");
        }
        return Ok(cow.into_owned());
    }

    // 2) Try charset from Content-Type header
    if let Some(label) = extract_charset_label(content_type) {
        if let Some(enc) = encoding_rs::Encoding::for_label_no_replacement(label.as_bytes()) {
            let (cow, _used, had_errors) = enc.decode(bytes);
            if !had_errors {
                return Ok(cow.into_owned());
            }
            // If header-provided label yields errors, treat as failure instead of falling back silently
            anyhow::bail!(format!(
                "decoding with declared charset '{}' produced errors",
                label
            ));
        }
    }

    // 3) Use chardetng to guess encoding, allow UTF-8
    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(bytes, true);
    let enc = detector.guess(None, true);
    let (cow, _used, had_errors) = enc.decode(bytes);
    if had_errors {
        anyhow::bail!(format!(
            "decoding with detected charset '{}' produced errors",
            enc.name()
        ));
    }
    Ok(cow.into_owned())
}

/// Extracts charset=... value from Content-Type header (case-insensitive) if present.
fn extract_charset_label(content_type: Option<&str>) -> Option<String> {
    let ct = content_type?;
    // Split on ';' and find parameter starting with charset=
    for part in ct.split(';').skip(1) {
        let kv = part.trim();
        if kv.to_ascii_lowercase().starts_with("charset=") {
            let v = kv[8..].trim();
            // Trim quotes if present
            let v = v.trim_matches('"').trim_matches('\'');
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

static POSITIVE_CLASS_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)article|body|content|entry|hentry|h-entry|main|page|pagination|post|text|blog|story|paragraph").expect("valid positive regex")
});

static NEGATIVE_CLASS_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)hidden|^hid$| hid$| hid |^hid |banner|breadcrumb|combx|comment|com-|contact|foot|footer|footnote|masthead|media|meta|outbrain|promo|related|scroll|share|shoutbox|sidebar|skyscraper|sponsor|shopping|tags|tool|widget|subscribe|nav|author|byline").expect("valid negative regex")
});

const MIN_DYNAMIC_TEXT_CHARS: usize = 180;

#[derive(Debug)]
enum SelectedHtml {
    Main(String),
    Body(String),
}

fn extract_main_content_fragment(html: &str) -> Option<SelectedHtml> {
    const MAIN_SELECTORS: &[&str] = &[
        "article",
        "article[role=\"article\"]",
        "main",
        "#main",
        "#main-content",
        "#mainContent",
        "#primary-content",
        "#article",
        "#article-body",
        "#articleBody",
        "#story-body",
        "#storyBody",
        "[role=\"main\"]",
        "[role=\"article\"]",
        ".main",
        ".main-content",
        ".main__content",
        ".main-body",
        ".primary-content",
        ".primary__content",
        ".page-content",
        ".page__content",
        ".content-body",
        ".content__body",
        ".content__article-body",
        ".contentArticle",
        ".article-content",
        ".article__content",
        ".article-body",
        ".article-body__content",
        ".article__body",
        ".articleBody",
        ".articleText",
        ".articletext",
        ".article-main",
        ".articlePage",
        ".article-page",
        ".articleDetail",
        ".article-detail",
        ".articleSection",
        ".o-article__body",
        ".c-article",
        ".c-article__content",
        ".l-article-content",
        ".story",
        ".story-body",
        ".story-body__inner",
        ".story__content",
        ".story-content",
        ".storyContent",
        ".storyText",
        ".post",
        ".post-article",
        ".post__content",
        ".post-content",
        ".post-content__body",
        ".post-body",
        ".post-body__content",
        ".post-text",
        ".entry",
        ".entry-content",
        ".entry__content",
        ".entry-content__inner",
        ".blog-post",
        ".blog__content",
        ".body-content",
        ".bodyText",
        ".body__content",
        ".rich-text",
        ".rich-text__content",
        ".prose",
        ".markdown-body",
        ".read__content",
        ".news-article",
        ".news-article__content",
        ".news-article-body",
        ".mw-parser-output",
    ];

    if html.trim().is_empty() {
        return None;
    }

    let document = Html::parse_document(html);

    for selector_str in MAIN_SELECTORS {
        if let Ok(selector) = Selector::parse(selector_str) {
            if let Some(element) = document.select(&selector).next() {
                let text = element.text().collect::<String>();
                if text.trim().is_empty() {
                    continue;
                }
                return Some(SelectedHtml::Main(element.html()));
            }
        }
    }

    if let Some(fragment) = select_by_positive_regex(&document) {
        return Some(SelectedHtml::Main(fragment));
    }

    if let Ok(body_selector) = Selector::parse("body") {
        if let Some(body) = document.select(&body_selector).next() {
            let text = body.text().collect::<String>();
            if !text.trim().is_empty() {
                return Some(SelectedHtml::Body(body.html()));
            }
        }
    }

    None
}

fn select_by_positive_regex(document: &Html) -> Option<String> {
    let selector = Selector::parse("[class],[id]").ok()?;
    let mut best: Option<(String, usize)> = None;

    for element in document.select(&selector) {
        let mut attr_tokens = String::new();
        if let Some(class_attr) = element.value().attr("class") {
            attr_tokens.push_str(class_attr);
        }
        if let Some(id_attr) = element.value().attr("id") {
            if !attr_tokens.is_empty() {
                attr_tokens.push(' ');
            }
            attr_tokens.push_str(id_attr);
        }

        if attr_tokens.is_empty() {
            continue;
        }

        if NEGATIVE_CLASS_REGEX.is_match(&attr_tokens)
            && !POSITIVE_CLASS_REGEX.is_match(&attr_tokens)
        {
            continue;
        }

        if POSITIVE_CLASS_REGEX.is_match(&attr_tokens) {
            let text = element.text().collect::<String>();
            let trimmed = text.trim();
            let text_len = trimmed.chars().count();
            if text_len < MIN_DYNAMIC_TEXT_CHARS {
                continue;
            }

            if best
                .as_ref()
                .map(|(_, current_len)| text_len > *current_len)
                .unwrap_or(true)
            {
                best = Some((element.html(), text_len));
            }
        }
    }

    best.map(|(html, _)| html)
}

#[cfg(test)]
mod tests {
    use super::{extract_main_content_fragment, SelectedHtml};

    #[test]
    fn selects_article_fragment() {
        let html = r#"<html><body><header>Header</header><article><h1>Title</h1><p>Important text.</p></article></body></html>"#;
        let fragment = extract_main_content_fragment(html).expect("fragment expected");
        match fragment {
            SelectedHtml::Main(inner) => {
                assert!(inner.contains("<h1>Title</h1>"));
                assert!(inner.contains("Important text."));
                assert!(!inner.contains("Header"));
            }
            SelectedHtml::Body(_) => panic!("expected article fragment"),
        }
    }

    #[test]
    fn falls_back_to_body_fragment() {
        let html = r#"<html><body><div class=\"content\"><p>Some text</p></div></body></html>"#;
        let fragment = extract_main_content_fragment(html).expect("fragment expected");
        match fragment {
            SelectedHtml::Main(_) => panic!("expected body fallback"),
            SelectedHtml::Body(inner) => {
                assert!(inner.contains("Some text"));
            }
        }
    }

    #[test]
    fn returns_none_for_empty_html() {
        assert!(extract_main_content_fragment("").is_none());
    }

    #[test]
    fn detects_positive_regex_fragment() {
        let repeated = "This is the main article segment. ";
        let mut body = String::new();
        for _ in 0..10 {
            body.push_str(repeated);
        }
        let html = format!(
            "<html><body><div class=\"content__article-body story-text\"><p>{}</p></div><aside class=\"sidebar\">Sidebar</aside></body></html>",
            body
        );
        let fragment = extract_main_content_fragment(&html).expect("fragment expected");
        match fragment {
            SelectedHtml::Main(inner) => {
                assert!(inner.contains("story-text"));
                assert!(inner.contains("This is the main article segment."));
            }
            SelectedHtml::Body(_) => panic!("expected main fragment from positive regex"),
        }
    }
}
