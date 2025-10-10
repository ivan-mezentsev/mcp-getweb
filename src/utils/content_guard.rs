use serde_json::{json, Value};

/// Result of binary detection based on Content-Type and magic bytes
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinaryDetection {
    /// Content is considered binary; optional MIME is provided when known
    Binary { content_type: Option<String> },
    /// Content is considered textual (safe to decode as UTF-8)
    Text,
}

/// Detects whether the content should be treated as binary using MIME and/or magic signatures in the head bytes.
///
/// content_type: Optional Content-Type value from response headers
/// head: First bytes of the body (recommended ~512 bytes)
pub fn detect_binary(content_type: Option<&str>, head: &[u8]) -> BinaryDetection {
    // 1) Check MIME if available
    if let Some(ct_raw) = content_type {
        let ct = ct_raw.trim().to_ascii_lowercase();

        // Extract type/subtype without parameters (e.g., `text/html; charset=utf-8` -> `text/html`)
        let mime_main = ct.split(';').next().unwrap_or("").trim();

        // Whitelist textual types explicitly considered safe
        // Note: `text/*`, `application/json`, `application/xml`, `application/javascript`, `application/xhtml+xml`
        let is_textual = mime_main.starts_with("text/")
            || matches!(
                mime_main,
                "application/json"
                    | "application/xml"
                    | "application/javascript"
                    | "application/xhtml+xml"
                    | "application/x-www-form-urlencoded"
            );

        if !is_textual {
            // Explicit binary types and families
            let is_explicit_binary = mime_main.starts_with("image/")
                || mime_main.starts_with("audio/")
                || mime_main.starts_with("video/")
                || mime_main.starts_with("font/")
                || mime_main == "application/pdf"
                || mime_main == "application/zip"
                || mime_main == "application/gzip"
                || mime_main == "application/octet-stream"
                || mime_main.starts_with("application/x-")
                || mime_main.starts_with("application/vnd.");

            if is_explicit_binary {
                return BinaryDetection::Binary {
                    content_type: Some(mime_main.to_string()),
                };
            }
        }

        // If MIME confidently says it's textual, we can immediately return Text without peeking bytes
        if is_textual {
            return BinaryDetection::Text;
        }
        // Otherwise fall through to signature-based detection for extra safety
    }

    // 2) Signature-based detection on the first bytes
    let h = head;

    // Helper closures
    let starts_with = |pat: &[u8]| h.len() >= pat.len() && &h[..pat.len()] == pat;
    let contains_within = |pat: &[u8], limit: usize| {
        let lim = limit.min(h.len());
        h[..lim].windows(pat.len()).any(|w| w == pat)
    };

    // Common binary magic signatures
    const PDF: &[u8] = b"%PDF-"; // PDF
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]; // PNG
    const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF]; // JPEG
    const GIF: &[u8] = b"GIF8"; // GIF87a/GIF89a
    const RIFF: &[u8] = b"RIFF"; // RIFF container (WebP, WAV, AVI)
    const WEBP: &[u8] = b"WEBP"; // WebP signature after RIFF
    const ZIP: &[u8] = &[0x50, 0x4B, 0x03, 0x04]; // ZIP
    const GZIP: &[u8] = &[0x1F, 0x8B]; // GZIP
    const RAR: &[u8] = b"Rar!"; // RAR
    const SEVEN_Z: &[u8] = &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]; // 7z
    const MP4_FTYP: &[u8] = b"ftyp"; // MP4 brands indicator

    let is_binary_by_magic = starts_with(PDF)
        || starts_with(PNG)
        || starts_with(JPEG)
        || starts_with(GIF)
        || starts_with(ZIP)
        || starts_with(GZIP)
        || starts_with(RAR)
        || starts_with(SEVEN_Z)
        // RIFF...WEBP
        || (starts_with(RIFF) && h.len() >= 12 && &h[8..12] == WEBP)
        // MP4: `ftyp` often appears within first ~64 bytes
        || contains_within(MP4_FTYP, 64);

    if is_binary_by_magic {
        return BinaryDetection::Binary { content_type: None };
    }

    BinaryDetection::Text
}

/// Safely truncates a UTF-8 string without breaking character boundaries.
/// If `s` length exceeds `max`, returns a string cut at a valid char boundary and appends `suffix`.
/// The resulting string length will be <= max whenever possible (suffix included). If `max` < suffix length,
/// the function returns a safely cut string without suffix, not exceeding `max` bytes.
pub fn safe_truncate_utf8(s: &str, max: usize, suffix: &str) -> String {
    if s.len() <= max {
        return s.to_string();
    }

    if max == 0 {
        return String::new();
    }

    let suffix_len = suffix.len();
    if max <= suffix_len {
        // Not enough room for the suffix; just cut on a char boundary up to max
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        return s[..end].to_string();
    }

    let mut end = max - suffix_len;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut result = String::with_capacity(end + suffix_len);
    result.push_str(&s[..end]);
    result.push_str(suffix);
    result
}

/// Builds a standardized error payload string for tool errors.
/// The resulting text is intended to be returned as a textual tool error body.
/// First line: short human-readable message.
/// Then a JSON object with fields: code, message, details.
pub fn build_error_payload(code: &str, message: &str, details: Value) -> String {
    let obj = json!({
        "code": code,
        "message": message,
        "details": details,
    });
    let mut out = String::new();
    out.push_str(message);
    out.push('\n');
    out.push_str(&obj.to_string());
    out
}
