// Minimal PDF utilities used across fetch-url and url-fetch flows.
// Always keep this module small and dependency-light.

use anyhow::Context;

/// Extracts text from a PDF stored fully in memory.
/// This is a thin wrapper over the `pdf-extract` crate API.
pub fn extract_text_from_pdf_mem(bytes: &[u8]) -> anyhow::Result<String> {
    let text = pdf_extract::extract_text_from_mem(bytes)
        .context("failed to extract text from PDF bytes using pdf-extract")?;
    Ok(text)
}

/// Returns true if given content-type or head indicates a PDF file.
/// - Content-Type: application/pdf (case-insensitive, substring match)
/// - Magic bytes: %PDF-
pub fn is_pdf(content_type: Option<&str>, head: &[u8]) -> bool {
    let ct = content_type.unwrap_or("").to_ascii_lowercase();
    ct.contains("application/pdf") || head.starts_with(b"%PDF-")
}
