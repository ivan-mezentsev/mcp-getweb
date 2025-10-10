use html5ever::driver::ParseOpts;
use html5ever::parse_document;
use html5ever::tendril::TendrilSink;
use html5ever::tree_builder::TreeBuilderOpts;
use html5ever::Attribute;
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;
use serde_json::json;
use std::cell::RefCell;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::OnceLock;
use tracing::{error, info};

use crate::mcp::types::{CallToolResult, ToolAnnotations, ToolDefinition};

pub static URL_FETCH_TOOL_DEFINITION: Lazy<ToolDefinition> = Lazy::new(|| ToolDefinition {
    name: "url-fetch".to_string(),
    description: "Fetch web pages and convert them to markdown format".to_string(),
    input_schema: json!({
        "type": "object",
        "properties": {
            "url": {
                "type": "string",
                "description": "The URL to fetch and convert to markdown"
            }
        },
        "required": ["url"]
    }),
    annotations: Some(ToolAnnotations {
        title: Some("URL Fetch Tool".to_string()),
        read_only_hint: Some(true),
        open_world_hint: Some(true),
    }),
});

#[derive(Debug, Deserialize)]
struct UrlFetchParams {
    url: String,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
enum ContentType {
    Html,
    Plaintext,
    Json,
}

// HTML Element structure
#[derive(Debug, Clone)]
pub struct HtmlElement {
    tag: String,
    pub attrs: RefCell<Vec<Attribute>>,
}

impl HtmlElement {
    pub fn new(tag: String, attrs: RefCell<Vec<Attribute>>) -> Self {
        Self { tag, attrs }
    }

    pub fn tag(&self) -> &str {
        &self.tag
    }

    pub fn is_inline(&self) -> bool {
        inline_elements().contains(self.tag.as_str())
    }

    pub fn attr(&self, name: &str) -> Option<String> {
        self.attrs
            .borrow()
            .iter()
            .find(|attr| attr.name.local.to_string() == name)
            .map(|attr| attr.value.to_string())
    }

    pub fn classes(&self) -> Vec<String> {
        self.attrs
            .borrow()
            .iter()
            .find(|attr| attr.name.local.to_string() == "class")
            .map(|attr| {
                attr.value
                    .split(' ')
                    .map(|class| class.trim().to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }

    pub fn has_class(&self, class: &str) -> bool {
        self.has_any_classes(&[class])
    }

    pub fn has_any_classes(&self, classes: &[&str]) -> bool {
        self.attrs.borrow().iter().any(|attr| {
            attr.name.local.to_string() == "class"
                && attr
                    .value
                    .split(' ')
                    .any(|class| classes.contains(&class.trim()))
        })
    }
}

/// Returns a [`HashSet`] containing the HTML elements that are inline by default.
fn inline_elements() -> &'static HashSet<&'static str> {
    static INLINE_ELEMENTS: OnceLock<HashSet<&str>> = OnceLock::new();
    INLINE_ELEMENTS.get_or_init(|| {
        HashSet::from_iter([
            "a", "abbr", "acronym", "audio", "b", "bdi", "bdo", "big", "br", "button", "canvas",
            "cite", "code", "data", "datalist", "del", "dfn", "em", "embed", "i", "iframe", "img",
            "input", "ins", "kbd", "label", "map", "mark", "meter", "noscript", "object", "output",
            "picture", "progress", "q", "ruby", "s", "samp", "script", "select", "slot", "small",
            "span", "strong", "sub", "sup", "svg", "template", "textarea", "time", "tt", "u",
            "var", "video", "wbr",
        ])
    })
}

// Markdown Writer
fn empty_line_regex() -> &'static Regex {
    static EMPTY_LINE_REGEX: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"^\s*$").expect("Failed to create empty_line_regex"));
    &EMPTY_LINE_REGEX
}

fn more_than_three_newlines_regex() -> &'static Regex {
    static REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"\n{3}").unwrap());
    &REGEX
}

pub enum StartTagOutcome {
    Continue,
    Skip,
}

pub type TagHandler = Rc<RefCell<dyn HandleTag>>;

pub struct MarkdownWriter {
    current_element_stack: VecDeque<HtmlElement>,
    pub markdown: String,
}

impl Default for MarkdownWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl MarkdownWriter {
    pub fn new() -> Self {
        Self {
            current_element_stack: VecDeque::new(),
            markdown: String::new(),
        }
    }

    pub fn current_element_stack(&self) -> &VecDeque<HtmlElement> {
        &self.current_element_stack
    }

    pub fn is_inside(&self, tag: &str) -> bool {
        self.current_element_stack
            .iter()
            .any(|parent_element| parent_element.tag() == tag)
    }

    pub fn push_str(&mut self, str: &str) {
        self.markdown.push_str(str);
    }

    pub fn push_newline(&mut self) {
        self.push_str("\n");
    }

    pub fn push_blank_line(&mut self) {
        self.push_str("\n\n");
    }

    pub fn run(
        mut self,
        root_node: &Handle,
        handlers: &mut [TagHandler],
    ) -> anyhow::Result<String> {
        self.visit_node(root_node, handlers)?;
        Ok(Self::prettify_markdown(self.markdown))
    }

    fn prettify_markdown(markdown: String) -> String {
        let markdown = empty_line_regex().replace_all(&markdown, "");
        let markdown = more_than_three_newlines_regex().replace_all(&markdown, "\n\n");

        markdown.trim().to_string()
    }

    fn visit_node(&mut self, node: &Handle, handlers: &mut [TagHandler]) -> anyhow::Result<()> {
        let mut current_element = None;

        match node.data {
            NodeData::Document
            | NodeData::Doctype { .. }
            | NodeData::ProcessingInstruction { .. }
            | NodeData::Comment { .. } => {
                // Currently left unimplemented, as we're not interested in this data
                // at this time.
            }
            NodeData::Element {
                ref name,
                ref attrs,
                ..
            } => {
                let tag_name = name.local.to_string();
                if !tag_name.is_empty() {
                    current_element = Some(HtmlElement::new(tag_name, attrs.clone()));
                }
            }
            NodeData::Text { ref contents } => {
                let text = contents.borrow().to_string();
                self.visit_text(text, handlers)?;
            }
        }

        if let Some(current_element) = current_element.as_ref() {
            match self.start_tag(current_element, handlers) {
                StartTagOutcome::Continue => {}
                StartTagOutcome::Skip => return Ok(()),
            }

            self.current_element_stack
                .push_back(current_element.clone());
        }

        for child in node.children.borrow().iter() {
            self.visit_node(child, handlers)?;
        }

        if let Some(current_element) = current_element {
            self.current_element_stack.pop_back();
            self.end_tag(&current_element, handlers);
        }

        Ok(())
    }

    fn start_tag(&mut self, tag: &HtmlElement, handlers: &mut [TagHandler]) -> StartTagOutcome {
        for handler in handlers {
            if handler.borrow().should_handle(tag.tag()) {
                match handler.borrow_mut().handle_tag_start(tag, self) {
                    StartTagOutcome::Continue => {}
                    StartTagOutcome::Skip => return StartTagOutcome::Skip,
                }
            }
        }

        StartTagOutcome::Continue
    }

    fn end_tag(&mut self, tag: &HtmlElement, handlers: &mut [TagHandler]) {
        for handler in handlers {
            if handler.borrow().should_handle(tag.tag()) {
                handler.borrow_mut().handle_tag_end(tag, self);
            }
        }
    }

    fn visit_text(&mut self, text: String, handlers: &mut [TagHandler]) -> anyhow::Result<()> {
        for handler in handlers {
            match handler.borrow_mut().handle_text(&text, self) {
                HandlerOutcome::Handled => return Ok(()),
                HandlerOutcome::NoOp => {}
            }
        }

        let text = text
            .trim_matches(|char| char == '\n' || char == '\r' || char == '\t')
            .replace('\n', " ");

        self.push_str(&text);

        Ok(())
    }
}

pub enum HandlerOutcome {
    Handled,
    NoOp,
}

pub trait HandleTag {
    fn should_handle(&self, tag: &str) -> bool;

    fn handle_tag_start(
        &mut self,
        _tag: &HtmlElement,
        _writer: &mut MarkdownWriter,
    ) -> StartTagOutcome {
        StartTagOutcome::Continue
    }

    fn handle_tag_end(&mut self, _tag: &HtmlElement, _writer: &mut MarkdownWriter) {}

    fn handle_text(&mut self, _text: &str, _writer: &mut MarkdownWriter) -> HandlerOutcome {
        HandlerOutcome::NoOp
    }
}

// Tag Handlers
pub struct WebpageChromeRemover;

impl HandleTag for WebpageChromeRemover {
    fn should_handle(&self, _tag: &str) -> bool {
        true // Check all elements for unwanted content
    }

    fn handle_tag_start(
        &mut self,
        tag: &HtmlElement,
        _writer: &mut MarkdownWriter,
    ) -> StartTagOutcome {
        // Skip common unwanted tags
        match tag.tag() {
            "head" | "script" | "style" | "nav" | "footer" | "aside" => {
                return StartTagOutcome::Skip;
            }
            _ => {}
        }

        // Skip elements with ad-related classes
        if tag.has_any_classes(&[
            "ad",
            "ads",
            "advertisement",
            "banner",
            "popup",
            "modal",
            "cookie",
            "newsletter",
            "sidebar",
            "widget",
            "promo",
            "sponsored",
            "affiliate",
            "tracking",
        ]) {
            return StartTagOutcome::Skip;
        }

        // Also check individual classes for more specific filtering
        let element_classes = tag.classes();
        for class in &element_classes {
            if class.contains("ad")
                || class.contains("banner")
                || class.contains("popup")
                || class.contains("promo")
            {
                return StartTagOutcome::Skip;
            }
        }

        // Check for specific unwanted classes
        if tag.has_class("advertisement") || tag.has_class("sponsored-content") {
            return StartTagOutcome::Skip;
        }

        // Skip elements with ad-related IDs
        if let Some(id) = tag.attr("id") {
            let id_lower = id.to_lowercase();
            if id_lower.contains("ad")
                || id_lower.contains("banner")
                || id_lower.contains("popup")
                || id_lower.contains("cookie")
                || id_lower.contains("newsletter")
                || id_lower.contains("sidebar")
            {
                return StartTagOutcome::Skip;
            }
        }

        StartTagOutcome::Continue
    }
}

pub struct ParagraphHandler;

impl HandleTag for ParagraphHandler {
    fn should_handle(&self, _tag: &str) -> bool {
        true
    }

    fn handle_tag_start(
        &mut self,
        tag: &HtmlElement,
        writer: &mut MarkdownWriter,
    ) -> StartTagOutcome {
        if tag.is_inline() && writer.is_inside("p") {
            if let Some(parent) = writer.current_element_stack().iter().last() {
                if !(parent.is_inline()
                    || writer.markdown.ends_with(' ')
                    || writer.markdown.ends_with('\n'))
                {
                    writer.push_str(" ");
                }
            }
        }

        if tag.tag() == "p" {
            writer.push_blank_line()
        }
        StartTagOutcome::Continue
    }
}

pub struct HeadingHandler;

impl HandleTag for HeadingHandler {
    fn should_handle(&self, tag: &str) -> bool {
        matches!(tag, "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
    }

    fn handle_tag_start(
        &mut self,
        tag: &HtmlElement,
        writer: &mut MarkdownWriter,
    ) -> StartTagOutcome {
        match tag.tag() {
            "h1" => writer.push_str("\n\n# "),
            "h2" => writer.push_str("\n\n## "),
            "h3" => writer.push_str("\n\n### "),
            "h4" => writer.push_str("\n\n#### "),
            "h5" => writer.push_str("\n\n##### "),
            "h6" => writer.push_str("\n\n###### "),
            _ => {}
        }

        StartTagOutcome::Continue
    }

    fn handle_tag_end(&mut self, tag: &HtmlElement, writer: &mut MarkdownWriter) {
        match tag.tag() {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => writer.push_blank_line(),
            _ => {}
        }
    }
}

pub struct ListHandler;

impl HandleTag for ListHandler {
    fn should_handle(&self, tag: &str) -> bool {
        matches!(tag, "ul" | "ol" | "li")
    }

    fn handle_tag_start(
        &mut self,
        tag: &HtmlElement,
        writer: &mut MarkdownWriter,
    ) -> StartTagOutcome {
        match tag.tag() {
            "ul" | "ol" => writer.push_newline(),
            "li" => writer.push_str("- "),
            _ => {}
        }

        StartTagOutcome::Continue
    }

    fn handle_tag_end(&mut self, tag: &HtmlElement, writer: &mut MarkdownWriter) {
        match tag.tag() {
            "ul" | "ol" => writer.push_newline(),
            "li" => writer.push_newline(),
            _ => {}
        }
    }
}

pub struct TableHandler {
    current_table_columns: usize,
    is_first_th: bool,
    is_first_td: bool,
}

impl TableHandler {
    pub fn new() -> Self {
        Self {
            current_table_columns: 0,
            is_first_th: true,
            is_first_td: true,
        }
    }
}

impl Default for TableHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl HandleTag for TableHandler {
    fn should_handle(&self, tag: &str) -> bool {
        matches!(tag, "table" | "thead" | "tbody" | "tr" | "th" | "td")
    }

    fn handle_tag_start(
        &mut self,
        tag: &HtmlElement,
        writer: &mut MarkdownWriter,
    ) -> StartTagOutcome {
        match tag.tag() {
            "thead" => writer.push_blank_line(),
            "tr" => writer.push_newline(),
            "th" => {
                self.current_table_columns += 1;
                if self.is_first_th {
                    self.is_first_th = false;
                } else {
                    writer.push_str(" ");
                }
                writer.push_str("| ");
            }
            "td" => {
                if self.is_first_td {
                    self.is_first_td = false;
                } else {
                    writer.push_str(" ");
                }
                writer.push_str("| ");
            }
            _ => {}
        }

        StartTagOutcome::Continue
    }

    fn handle_tag_end(&mut self, tag: &HtmlElement, writer: &mut MarkdownWriter) {
        match tag.tag() {
            "thead" => {
                writer.push_newline();
                for ix in 0..self.current_table_columns {
                    if ix > 0 {
                        writer.push_str(" ");
                    }
                    writer.push_str("| ---");
                }
                writer.push_str(" |");
                self.is_first_th = true;
            }
            "tr" => {
                writer.push_str(" |");
                self.is_first_td = true;
            }
            "table" => {
                self.current_table_columns = 0;
            }
            _ => {}
        }
    }
}

pub struct StyledTextHandler;

impl HandleTag for StyledTextHandler {
    fn should_handle(&self, tag: &str) -> bool {
        matches!(tag, "strong" | "em")
    }

    fn handle_tag_start(
        &mut self,
        tag: &HtmlElement,
        writer: &mut MarkdownWriter,
    ) -> StartTagOutcome {
        match tag.tag() {
            "strong" => writer.push_str("**"),
            "em" => writer.push_str("_"),
            _ => {}
        }

        StartTagOutcome::Continue
    }

    fn handle_tag_end(&mut self, tag: &HtmlElement, writer: &mut MarkdownWriter) {
        match tag.tag() {
            "strong" => writer.push_str("**"),
            "em" => writer.push_str("_"),
            _ => {}
        }
    }
}

pub struct LinkHandler;

impl HandleTag for LinkHandler {
    fn should_handle(&self, tag: &str) -> bool {
        tag == "a"
    }

    fn handle_tag_start(
        &mut self,
        tag: &HtmlElement,
        writer: &mut MarkdownWriter,
    ) -> StartTagOutcome {
        if tag.attr("href").is_some() {
            writer.push_str("[");
        }
        StartTagOutcome::Continue
    }

    fn handle_tag_end(&mut self, tag: &HtmlElement, writer: &mut MarkdownWriter) {
        if let Some(href) = tag.attr("href") {
            writer.push_str(&format!("]({})", href));
        }
    }
}

pub struct ImageHandler;

impl HandleTag for ImageHandler {
    fn should_handle(&self, tag: &str) -> bool {
        tag == "img"
    }

    fn handle_tag_start(
        &mut self,
        tag: &HtmlElement,
        writer: &mut MarkdownWriter,
    ) -> StartTagOutcome {
        if let Some(src) = tag.attr("src") {
            let alt = tag.attr("alt").unwrap_or("image".to_string());
            writer.push_str(&format!("![{}]({})", alt, src));
        }
        StartTagOutcome::Skip // Images are self-closing
    }
}

pub struct CodeHandler;

impl HandleTag for CodeHandler {
    fn should_handle(&self, tag: &str) -> bool {
        matches!(tag, "pre" | "code")
    }

    fn handle_tag_start(
        &mut self,
        tag: &HtmlElement,
        writer: &mut MarkdownWriter,
    ) -> StartTagOutcome {
        match tag.tag() {
            "code" => {
                if !writer.is_inside("pre") {
                    writer.push_str("`");
                }
            }
            "pre" => writer.push_str("\n\n```\n"),
            _ => {}
        }

        StartTagOutcome::Continue
    }

    fn handle_tag_end(&mut self, tag: &HtmlElement, writer: &mut MarkdownWriter) {
        match tag.tag() {
            "code" => {
                if !writer.is_inside("pre") {
                    writer.push_str("`");
                }
            }
            "pre" => writer.push_str("\n```\n"),
            _ => {}
        }
    }

    fn handle_text(&mut self, text: &str, writer: &mut MarkdownWriter) -> HandlerOutcome {
        if writer.is_inside("pre") {
            writer.push_str(text);
            return HandlerOutcome::Handled;
        }

        HandlerOutcome::NoOp
    }
}

// HTML to Markdown conversion
pub fn convert_html_to_markdown(
    html: &[u8],
    handlers: &mut [TagHandler],
) -> anyhow::Result<String> {
    let dom = parse_html(html)?;

    let markdown_writer = MarkdownWriter::new();
    let markdown = markdown_writer
        .run(&dom.document, handlers)
        .map_err(|e| anyhow::anyhow!("Failed to convert HTML to Markdown: {}", e))?;

    Ok(markdown)
}

fn parse_html(html: &[u8]) -> anyhow::Result<RcDom> {
    let parse_options = ParseOpts {
        tree_builder: TreeBuilderOpts {
            drop_doctype: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let dom = parse_document(RcDom::default(), parse_options)
        .from_utf8()
        .read_from(&mut std::io::Cursor::new(html))
        .map_err(|e| anyhow::anyhow!("Failed to parse HTML document: {}", e))?;

    Ok(dom)
}

pub struct UrlFetchTool;

impl UrlFetchTool {
    pub fn new() -> Self {
        Self
    }

    async fn build_message(url: &str) -> anyhow::Result<String> {
        let url = if !url.starts_with("https://") && !url.starts_with("http://") {
            format!("https://{}", url)
        } else {
            url.to_string()
        };

        let client = reqwest::Client::new();
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch URL: {}", e))?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "HTTP error {}: {}",
                response.status(),
                response
                    .status()
                    .canonical_reason()
                    .unwrap_or("Unknown error")
            ));
        }

        let content_type_header = response
            .headers()
            .get("content-type")
            .and_then(|ct| ct.to_str().ok())
            .unwrap_or("text/html")
            .to_string();

        let body = response
            .bytes()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read response body: {}", e))?;

        let content_type = match content_type_header.as_str() {
            ct if ct.contains("text/html") => ContentType::Html,
            ct if ct.contains("text/plain") => ContentType::Plaintext,
            ct if ct.contains("application/json") => ContentType::Json,
            _ => ContentType::Html,
        };

        match content_type {
            ContentType::Html => {
                let mut handlers: Vec<TagHandler> = vec![
                    Rc::new(RefCell::new(WebpageChromeRemover)),
                    Rc::new(RefCell::new(ParagraphHandler)),
                    Rc::new(RefCell::new(HeadingHandler)),
                    Rc::new(RefCell::new(ListHandler)),
                    Rc::new(RefCell::new(TableHandler::new())),
                    Rc::new(RefCell::new(StyledTextHandler)),
                    Rc::new(RefCell::new(LinkHandler)),
                    Rc::new(RefCell::new(ImageHandler)),
                    Rc::new(RefCell::new(CodeHandler)),
                ];

                convert_html_to_markdown(&body, &mut handlers)
            }
            ContentType::Plaintext => Ok(std::str::from_utf8(&body)
                .map_err(|e| anyhow::anyhow!("Invalid UTF-8: {}", e))?
                .to_owned()),
            ContentType::Json => {
                let json: serde_json::Value = serde_json::from_slice(&body)
                    .map_err(|e| anyhow::anyhow!("Invalid JSON: {}", e))?;

                Ok(format!(
                    "```json\n{}\n```",
                    serde_json::to_string_pretty(&json)
                        .map_err(|e| anyhow::anyhow!("Failed to format JSON: {}", e))?
                ))
            }
        }
    }

    pub async fn execute(&self, arguments: Option<serde_json::Value>) -> CallToolResult {
        let params = match arguments {
            Some(args) => match serde_json::from_value::<UrlFetchParams>(args) {
                Ok(params) => params,
                Err(e) => {
                    error!("Invalid URL fetch parameters: {}", e);
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

        info!("Fetching and converting URL to markdown: {}", params.url);

        match Self::build_message(&params.url).await {
            Ok(content) => {
                if content.trim().is_empty() {
                    CallToolResult::error("No textual content found")
                } else {
                    CallToolResult::success(content)
                }
            }
            Err(e) => {
                error!("Error fetching URL {}: {}", params.url, e);
                CallToolResult::error(format!("Error fetching URL: {}", e))
            }
        }
    }
}
