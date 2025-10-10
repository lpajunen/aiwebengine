use regex::Regex;
use thiserror::Error;
use std::collections::HashSet;
use html_escape::encode_text;
use sha2::{Sha256, Digest};

#[derive(Error, Debug)]
pub enum SecurityError {
    #[error("Invalid URI: {0}")]
    InvalidUri(String),

    #[error("URI too long: {actual} > {max}")]
    UriTooLong { actual: usize, max: usize },

    #[error("Content too large: {actual} > {max}")]
    ContentTooLarge { actual: usize, max: usize },

    #[error("Dangerous pattern detected: {0}")]
    DangerousPattern(String),

    #[error("Path traversal attempt")]
    PathTraversal,

    #[error("Insufficient capabilities: required {required:?}")]
    InsufficientCapabilities { required: Vec<Capability> },

    #[error("Operation not allowed: {0}")]
    OperationNotAllowed(String),

    #[error("XSS attempt detected: {0}")]
    XssAttempt(String),

    #[error("CSRF token validation failed")]
    CsrfValidationFailed,

    #[error("MIME type mismatch: expected {expected}, got {actual}")]
    MimeTypeMismatch { expected: String, actual: String },

    #[error("Suspicious JavaScript pattern detected: {0}")]
    SuspiciousJsPattern(String),

    #[error("Infinite loop detected in script")]
    InfiniteLoopDetected,

    #[error("Prototype pollution attempt")]
    PrototypePollutionAttempt,
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum Capability {
    ReadScripts,
    WriteScripts,
    DeleteScripts,
    ReadAssets,
    WriteAssets,
    DeleteAssets,
    ViewLogs,
    ManageStreams,
    ManageGraphQL,
}

/// Comprehensive input validator - ALL VALIDATION IN RUST
#[derive(Clone)]
pub struct InputValidator {
    uri_pattern: Regex,
    dangerous_patterns: Vec<Regex>,
    max_uri_length: usize,
    max_script_size: usize,
    max_asset_size: usize,
    // Enhanced patterns for JavaScript analysis
    prototype_pollution_patterns: Vec<Regex>,
    infinite_loop_patterns: Vec<Regex>,
    xss_patterns: Vec<Regex>,
    // Allowed MIME types for assets
    allowed_mime_types: HashSet<String>,
}

impl Default for InputValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl InputValidator {
    pub fn new() -> Self {
        let uri_pattern =
            Regex::new(r"^[a-zA-Z0-9\-_/.]+$").expect("Valid regex pattern for URI validation");

        // Compile dangerous patterns
        let dangerous_str_patterns = vec![
            r"eval\s*\(",
            r"Function\s*\(",
            r"setTimeout\s*\(",
            r"setInterval\s*\(",
            r"import\s*\(",
            r"require\s*\(",
            r"process\.",
            r"__proto__",
            r"constructor\.constructor",
            r"globalThis",
            r"window",
            r"document",
        ];

        let mut dangerous_patterns = Vec::new();
        for pattern in dangerous_str_patterns {
            let regex =
                Regex::new(pattern).expect("Valid regex pattern for dangerous pattern detection");
            dangerous_patterns.push(regex);
        }

        // Prototype pollution patterns
        let prototype_pollution_str_patterns = vec![
            r"__proto__\s*=",
            r"constructor\s*=",
            r"prototype\s*=",
            r"constructor\.prototype",
            r"Object\.prototype",
            r"Array\.prototype",
            r"Function\.prototype",
        ];

        let mut prototype_pollution_patterns = Vec::new();
        for pattern in prototype_pollution_str_patterns {
            let regex = Regex::new(pattern).expect("Valid regex pattern for prototype pollution detection");
            prototype_pollution_patterns.push(regex);
        }

        // Infinite loop patterns
        let infinite_loop_str_patterns = vec![
            r"while\s*\(\s*true\s*\)",
            r"while\s*\(\s*1\s*\)",
            r"for\s*\(\s*;\s*;\s*\)",
            r"for\s*\(\s*;.*true.*;\s*\)",
            r"do\s*\{.*\}\s*while\s*\(\s*true\s*\)",
        ];

        let mut infinite_loop_patterns = Vec::new();
        for pattern in infinite_loop_str_patterns {
            let regex = Regex::new(pattern).expect("Valid regex pattern for infinite loop detection");
            infinite_loop_patterns.push(regex);
        }

        // XSS patterns
        let xss_str_patterns = vec![
            r"<script[^>]*>",
            r"javascript\s*:",
            r"on\w+\s*=",
            r"<iframe[^>]*>",
            r"<object[^>]*>",
            r"<embed[^>]*>",
            r"<applet[^>]*>",
            r"<meta[^>]*>",
            r"<link[^>]*>",
        ];

        let mut xss_patterns = Vec::new();
        for pattern in xss_str_patterns {
            let regex = Regex::new(pattern).expect("Valid regex pattern for XSS detection");
            xss_patterns.push(regex);
        }

        // Allowed MIME types for assets
        let mut allowed_mime_types = HashSet::new();
        allowed_mime_types.extend([
            "text/plain".to_string(),
            "text/css".to_string(),
            "text/html".to_string(),
            "text/javascript".to_string(),
            "application/javascript".to_string(),
            "application/json".to_string(),
            "application/xml".to_string(),
            "image/png".to_string(),
            "image/jpeg".to_string(),
            "image/gif".to_string(),
            "image/svg+xml".to_string(),
            "image/webp".to_string(),
            "font/woff".to_string(),
            "font/woff2".to_string(),
            "font/ttf".to_string(),
            "font/otf".to_string(),
        ]);

        Self {
            uri_pattern,
            dangerous_patterns,
            max_uri_length: 200,
            max_script_size: 100_000,   // 100KB
            max_asset_size: 10_000_000, // 10MB
            prototype_pollution_patterns,
            infinite_loop_patterns,
            xss_patterns,
            allowed_mime_types,
        }
    }

    /// Validate and sanitize URI - COMPREHENSIVE RUST VALIDATION
    pub fn validate_uri(&self, uri: &str) -> Result<String, SecurityError> {
        // Length check
        if uri.len() > self.max_uri_length {
            return Err(SecurityError::UriTooLong {
                actual: uri.len(),
                max: self.max_uri_length,
            });
        }

        // Must start with /
        if !uri.starts_with('/') {
            return Err(SecurityError::InvalidUri(
                "URI must start with '/'".to_string(),
            ));
        }

        // Path traversal check
        if uri.contains("..") || uri.contains("\\") {
            return Err(SecurityError::PathTraversal);
        }

        // Character validation
        if !self.uri_pattern.is_match(uri) {
            return Err(SecurityError::InvalidUri(
                "Invalid characters in URI".to_string(),
            ));
        }

        // Normalize path
        let segments: Vec<&str> = uri.split('/').filter(|s| !s.is_empty()).collect();
        Ok(format!("/{}", segments.join("/")))
    }

    /// Validate script content - COMPREHENSIVE RUST VALIDATION
    pub fn validate_script_content(&self, content: &str) -> Result<(), SecurityError> {
        // Size check
        if content.len() > self.max_script_size {
            return Err(SecurityError::ContentTooLarge {
                actual: content.len(),
                max: self.max_script_size,
            });
        }

        // Check for dangerous patterns
        for pattern in &self.dangerous_patterns {
            if let Some(matched) = pattern.find(content) {
                return Err(SecurityError::DangerousPattern(
                    matched.as_str().to_string(),
                ));
            }
        }

        // Additional syntax validation
        self.validate_javascript_syntax(content)?;

        Ok(())
    }

    /// Validate asset content with enhanced MIME type verification
    pub fn validate_asset_content(
        &self,
        content: &[u8],
        declared_mimetype: &str,
    ) -> Result<(), SecurityError> {
        if content.len() > self.max_asset_size {
            return Err(SecurityError::ContentTooLarge {
                actual: content.len(),
                max: self.max_asset_size,
            });
        }

        // Validate declared MIME type is allowed
        if declared_mimetype.is_empty() {
            return Err(SecurityError::InvalidUri(
                "MIME type cannot be empty".to_string(),
            ));
        }

        if !self.allowed_mime_types.contains(declared_mimetype) {
            return Err(SecurityError::InvalidUri(
                format!("MIME type not allowed: {}", declared_mimetype)
            ));
        }

        // Detect actual MIME type from content and verify it matches declared type
        self.verify_mime_type_matches_content(content, declared_mimetype)?;

        // Additional security checks based on MIME type
        match declared_mimetype {
            "text/html" => self.validate_html_content(content)?,
            "text/javascript" | "application/javascript" => {
                let content_str = String::from_utf8_lossy(content);
                self.validate_script_content(&content_str)?;
            }
            "image/svg+xml" => self.validate_svg_content(content)?,
            _ => {}
        }

        Ok(())
    }

    /// Verify that the actual content matches the declared MIME type
    fn verify_mime_type_matches_content(
        &self,
        content: &[u8],
        declared_mimetype: &str,
    ) -> Result<(), SecurityError> {
        // Use file magic bytes to detect actual type
        let detected_type = self.detect_mime_type_from_content(content);
        
        // Allow some flexibility for text types
        let is_compatible = match (declared_mimetype, detected_type.as_str()) {
            // Exact matches
            (declared, detected) if declared == detected => true,
            // Text type compatibility
            ("text/plain", detected) if detected.starts_with("text/") => true,
            ("application/javascript", "text/javascript") => true,
            ("text/javascript", "application/javascript") => true,
            // Image format compatibility
            ("image/jpeg", "image/jpg") => true,
            ("image/jpg", "image/jpeg") => true,
            _ => false,
        };

        if !is_compatible {
            return Err(SecurityError::MimeTypeMismatch {
                expected: declared_mimetype.to_string(),
                actual: detected_type,
            });
        }

        Ok(())
    }

    /// Detect MIME type from file content using magic bytes
    fn detect_mime_type_from_content(&self, content: &[u8]) -> String {
        if content.is_empty() {
            return "application/octet-stream".to_string();
        }

        // Check for common file signatures
        match content {
            // PNG
            content if content.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) => {
                "image/png".to_string()
            }
            // JPEG
            content if content.starts_with(&[0xFF, 0xD8, 0xFF]) => "image/jpeg".to_string(),
            // GIF
            content if content.starts_with(b"GIF87a") || content.starts_with(b"GIF89a") => {
                "image/gif".to_string()
            }
            // WebP
            content if content.len() >= 12 && 
                      content[0..4] == [0x52, 0x49, 0x46, 0x46] && 
                      content[8..12] == [0x57, 0x45, 0x42, 0x50] => {
                "image/webp".to_string()
            }
            // SVG (starts with < or <?xml)
            content if content.starts_with(b"<") || content.starts_with(b"<?xml") => {
                let content_str = String::from_utf8_lossy(content).to_lowercase();
                if content_str.contains("<svg") {
                    "image/svg+xml".to_string()
                } else if content_str.contains("<!doctype html") || content_str.contains("<html") {
                    "text/html".to_string()
                } else {
                    "text/xml".to_string()
                }
            }
            // Try to detect text content
            content => {
                // Check if content is valid UTF-8 text
                if let Ok(text) = std::str::from_utf8(content) {
                    // Detect JavaScript
                    if text.contains("function") || text.contains("var ") || text.contains("const ") {
                        "text/javascript".to_string()
                    } else if text.contains("{") && text.contains("}") {
                        "application/json".to_string()
                    } else {
                        "text/plain".to_string()
                    }
                } else {
                    "application/octet-stream".to_string()
                }
            }
        }
    }

    /// Validate HTML content for XSS prevention
    fn validate_html_content(&self, content: &[u8]) -> Result<(), SecurityError> {
        let content_str = String::from_utf8_lossy(content);
        
        // Check for XSS patterns
        for pattern in &self.xss_patterns {
            if let Some(matched) = pattern.find(&content_str) {
                return Err(SecurityError::XssAttempt(
                    matched.as_str().to_string()
                ));
            }
        }

        // Check for dangerous JavaScript in HTML attributes
        let js_in_attr_pattern = Regex::new(r#"on\w+\s*=\s*["'][^"']*javascript"#).unwrap();
        if js_in_attr_pattern.is_match(&content_str) {
            return Err(SecurityError::XssAttempt(
                "JavaScript in HTML attributes detected".to_string()
            ));
        }

        Ok(())
    }

    /// Validate SVG content for security issues
    fn validate_svg_content(&self, content: &[u8]) -> Result<(), SecurityError> {
        let content_str = String::from_utf8_lossy(content);
        
        // SVGs can contain JavaScript, check for script elements
        if content_str.contains("<script") {
            return Err(SecurityError::XssAttempt(
                "Script element in SVG detected".to_string()
            ));
        }

        // Check for foreign object elements that could contain HTML
        if content_str.contains("<foreignObject") {
            return Err(SecurityError::XssAttempt(
                "Foreign object in SVG detected".to_string()
            ));
        }

        // Check for dangerous event handlers
        let event_handlers = [
            "onload", "onerror", "onclick", "onmouseover", "onmouseout",
            "onfocus", "onblur", "onchange", "onsubmit"
        ];
        
        for handler in &event_handlers {
            if content_str.to_lowercase().contains(handler) {
                return Err(SecurityError::XssAttempt(
                    format!("Event handler {} in SVG detected", handler)
                ));
            }
        }

        Ok(())
    }

    fn validate_javascript_syntax(&self, content: &str) -> Result<(), SecurityError> {
        // Enhanced JavaScript analysis - simulating basic AST analysis

        // Check for infinite loops
        for pattern in &self.infinite_loop_patterns {
            if pattern.is_match(content) {
                return Err(SecurityError::InfiniteLoopDetected);
            }
        }

        // Check for prototype pollution attempts
        for pattern in &self.prototype_pollution_patterns {
            if let Some(_matched) = pattern.find(content) {
                return Err(SecurityError::PrototypePollutionAttempt);
            }
        }

        // Check for suspicious JavaScript patterns
        self.analyze_suspicious_patterns(content)?;

        // Check for nested function calls that could be dangerous
        self.analyze_function_nesting(content)?;

        // Check for string concatenation that could lead to injection
        self.analyze_string_operations(content)?;

        Ok(())
    }

    /// Analyze suspicious JavaScript patterns
    fn analyze_suspicious_patterns(&self, content: &str) -> Result<(), SecurityError> {
        // Check for dynamic property access that could be dangerous
        let suspicious_patterns = [
            r"this\[.*\]",
            r"global\[.*\]",
            r"window\[.*\]",
            r"document\[.*\]",
            r"process\[.*\]",
            r"require\[.*\]",
            r"\.call\s*\(",
            r"\.apply\s*\(",
            r"\.bind\s*\(",
            r"new\s+Function\s*\(",
            r"new\s+Array\s*\(",
        ];

        for pattern_str in &suspicious_patterns {
            let pattern = Regex::new(pattern_str).unwrap();
            if let Some(matched) = pattern.find(content) {
                return Err(SecurityError::SuspiciousJsPattern(
                    matched.as_str().to_string()
                ));
            }
        }

        Ok(())
    }

    /// Analyze function nesting depth and complexity
    fn analyze_function_nesting(&self, content: &str) -> Result<(), SecurityError> {
        let mut brace_depth: i32 = 0;
        let mut max_depth: i32 = 0;
        
        for char in content.chars() {
            match char {
                '{' => {
                    brace_depth += 1;
                    max_depth = max_depth.max(brace_depth);
                }
                '}' => {
                    brace_depth = brace_depth.saturating_sub(1);
                }
                _ => {}
            }
        }

        // Prevent deeply nested code that could indicate obfuscation
        if max_depth > 20 {
            return Err(SecurityError::SuspiciousJsPattern(
                "Excessive nesting depth detected".to_string()
            ));
        }

        Ok(())
    }

    /// Analyze string operations for potential injection
    fn analyze_string_operations(&self, content: &str) -> Result<(), SecurityError> {
        // Check for template literal injections
        if content.contains("${") && content.contains("}") {
            let template_pattern = Regex::new(r"\$\{[^}]*\}").unwrap();
            for matched in template_pattern.find_iter(content) {
                let template_content = matched.as_str();
                // Check if template contains dangerous operations
                if template_content.contains("eval") || 
                   template_content.contains("Function") ||
                   template_content.contains("require") {
                    return Err(SecurityError::SuspiciousJsPattern(
                        format!("Dangerous template literal: {}", template_content)
                    ));
                }
            }
        }

        // Check for string concatenation with eval-like operations
        let concat_eval_pattern = Regex::new(r#"["'`][^"'`]*["'`]\s*\+.*eval"#).unwrap();
        if concat_eval_pattern.is_match(content) {
            return Err(SecurityError::SuspiciousJsPattern(
                "String concatenation with eval detected".to_string()
            ));
        }

        Ok(())
    }

    /// Validate script names (filenames for scripts)
    pub fn validate_script_name(&self, name: &str) -> Result<(), SecurityError> {
        if name.is_empty() {
            return Err(SecurityError::InvalidUri(
                "Script name cannot be empty".to_string(),
            ));
        }

        // Check for path traversal
        if name.contains("..") || name.contains('/') || name.contains('\\') {
            return Err(SecurityError::InvalidUri(
                "Script name contains invalid path characters".to_string(),
            ));
        }

        // Check for valid filename characters
        let valid_name = Regex::new(r"^[a-zA-Z0-9_\-\.]+$").unwrap();
        if !valid_name.is_match(name) {
            return Err(SecurityError::InvalidUri(
                "Script name contains invalid characters".to_string(),
            ));
        }

        Ok(())
    }

    /// Validate asset filenames
    pub fn validate_asset_filename(&self, filename: &str) -> Result<(), SecurityError> {
        if filename.is_empty() {
            return Err(SecurityError::InvalidUri(
                "Asset filename cannot be empty".to_string(),
            ));
        }

        // Check for path traversal
        if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
            return Err(SecurityError::InvalidUri(
                "Asset filename contains invalid path characters".to_string(),
            ));
        }

        // Check for valid filename characters
        let valid_filename = Regex::new(r"^[a-zA-Z0-9_\-\.]+$").unwrap();
        if !valid_filename.is_match(filename) {
            return Err(SecurityError::InvalidUri(
                "Asset filename contains invalid characters".to_string(),
            ));
        }

        Ok(())
    }

    /// Validate URLs (alias for validate_uri but more specific for HTTP URLs)
    pub fn validate_url(&self, url: &str) -> Result<(), SecurityError> {
        // Must be HTTP or HTTPS
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(SecurityError::InvalidUri(
                "URL must use HTTP or HTTPS protocol".to_string(),
            ));
        }

        // Check for dangerous schemes that could be XSS vectors
        let dangerous_schemes = ["javascript:", "data:", "file:", "ftp:"];
        for scheme in &dangerous_schemes {
            if url.to_lowercase().starts_with(scheme) {
                return Err(SecurityError::InvalidUri(format!(
                    "Dangerous URL scheme detected: {}",
                    scheme
                )));
            }
        }

        // Basic URL format validation
        url::Url::parse(url)
            .map_err(|_| SecurityError::InvalidUri("Invalid URL format".to_string()))?;

        Ok(())
    }

    /// Validate HTTP header values
    pub fn validate_header_value(&self, value: &str) -> Result<(), SecurityError> {
        // Check for CRLF injection
        if value.contains('\r') || value.contains('\n') {
            return Err(SecurityError::InvalidUri(
                "Header value contains CRLF characters".to_string(),
            ));
        }

        // Check for null bytes
        if value.contains('\0') {
            return Err(SecurityError::InvalidUri(
                "Header value contains null bytes".to_string(),
            ));
        }

        // Basic length check
        if value.len() > 8192 {
            return Err(SecurityError::InvalidUri(
                "Header value too long".to_string(),
            ));
        }

        Ok(())
    }

    /// Validate GraphQL schema
    pub fn validate_graphql_schema(&self, schema: &str) -> Result<(), SecurityError> {
        if schema.is_empty() {
            return Err(SecurityError::InvalidUri(
                "GraphQL schema cannot be empty".to_string(),
            ));
        }

        // Check for dangerous patterns
        let dangerous_patterns = ["__schema", "__type", "introspection"];
        for pattern in &dangerous_patterns {
            if schema.to_lowercase().contains(pattern) {
                tracing::warn!(
                    "GraphQL schema contains potentially sensitive introspection: {}",
                    pattern
                );
            }
        }

        // Basic length check
        if schema.len() > 1_000_000 {
            return Err(SecurityError::InvalidUri(
                "GraphQL schema too large".to_string(),
            ));
        }

        Ok(())
    }

    /// Validate stream names
    pub fn validate_stream_name(&self, name: &str) -> Result<(), SecurityError> {
        if name.is_empty() {
            return Err(SecurityError::InvalidUri(
                "Stream name cannot be empty".to_string(),
            ));
        }

        // Check for valid stream name format
        let valid_name = Regex::new(r"^[a-zA-Z0-9_\-]+$").unwrap();
        if !valid_name.is_match(name) {
            return Err(SecurityError::InvalidUri(
                "Stream name contains invalid characters".to_string(),
            ));
        }

        // Length check
        if name.len() > 64 {
            return Err(SecurityError::InvalidUri(
                "Stream name too long".to_string(),
            ));
        }

        Ok(())
    }

    /// Validate configuration values
    pub fn validate_config_value(&self, value: &str) -> Result<(), SecurityError> {
        // Check for dangerous patterns in configuration
        let dangerous_patterns = ["eval(", "function(", "require(", "import("];
        for pattern in &dangerous_patterns {
            if value.to_lowercase().contains(pattern) {
                return Err(SecurityError::InvalidUri(format!(
                    "Dangerous pattern in config value: {}",
                    pattern
                )));
            }
        }

        // Check for path traversal
        if value.contains("../") || value.contains("..\\") {
            return Err(SecurityError::InvalidUri(
                "Config value contains path traversal".to_string(),
            ));
        }

        // Basic length check
        if value.len() > 4096 {
            return Err(SecurityError::InvalidUri(
                "Config value too long".to_string(),
            ));
        }

        Ok(())
    }

    /// Sanitize and encode text for safe HTML output
    pub fn encode_html_text(&self, text: &str) -> String {
        encode_text(text).to_string()
    }

    /// Sanitize and encode text for safe HTML attribute output
    pub fn encode_html_attribute(&self, text: &str) -> String {
        // html-escape doesn't have encode_attribute, so we'll use encode_text
        // and add quotes for attribute safety
        format!("\"{}\"", encode_text(text))
    }

    /// Generate CSRF token
    pub fn generate_csrf_token(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(uuid::Uuid::new_v4().as_bytes());
        hasher.update(chrono::Utc::now().timestamp().to_string().as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Validate CSRF token
    pub fn validate_csrf_token(&self, token: &str, session_token: &str) -> Result<(), SecurityError> {
        if token.is_empty() || session_token.is_empty() {
            return Err(SecurityError::CsrfValidationFailed);
        }

        // Basic token format validation
        if token.len() != 64 || !token.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(SecurityError::CsrfValidationFailed);
        }

        // In a real implementation, you would verify the token against
        // the session or use a more sophisticated CSRF protection mechanism
        // For now, we just validate the format
        Ok(())
    }

    /// Validate input for potential XSS attacks
    pub fn validate_user_input(&self, input: &str) -> Result<String, SecurityError> {
        // Check for XSS patterns
        for pattern in &self.xss_patterns {
            if let Some(matched) = pattern.find(input) {
                return Err(SecurityError::XssAttempt(
                    matched.as_str().to_string()
                ));
            }
        }

        // Return encoded safe version
        Ok(self.encode_html_text(input))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uri_validation() {
        let validator = InputValidator::new();

        // Valid URIs
        assert!(validator.validate_uri("/api/test").is_ok());
        assert!(validator.validate_uri("/users/123").is_ok());

        // Invalid URIs
        assert!(validator.validate_uri("api/test").is_err()); // No leading slash
        assert!(validator.validate_uri("/api/../etc").is_err()); // Path traversal
        assert!(validator.validate_uri("/api/<script>").is_err()); // Invalid chars
    }

    #[test]
    fn test_script_validation() {
        let validator = InputValidator::new();

        // Valid script
        assert!(
            validator
                .validate_script_content("function test() { return 'hello'; }")
                .is_ok()
        );

        // Invalid scripts
        assert!(validator.validate_script_content("eval('code')").is_err());
        assert!(
            validator
                .validate_script_content("Function('return this')()")
                .is_err()
        );
        assert!(validator.validate_script_content("process.exit()").is_err());
    }

    #[test]
    fn test_enhanced_javascript_validation() {
        let validator = InputValidator::new();

        // Test infinite loop detection
        assert!(validator.validate_script_content("while (true) { console.log('test'); }").is_err());
        assert!(validator.validate_script_content("for (;;) { break; }").is_err());

        // Test prototype pollution detection
        assert!(validator.validate_script_content("obj.__proto__ = malicious").is_err());
        assert!(validator.validate_script_content("constructor.prototype = bad").is_err());

        // Test suspicious patterns
        assert!(validator.validate_script_content("this['eval']('code')").is_err());
        assert!(validator.validate_script_content("new Function('return this')()").is_err());
    }

    #[test]
    fn test_mime_type_validation() {
        let validator = InputValidator::new();

        // Test PNG validation
        let png_content = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00];
        assert!(validator.validate_asset_content(&png_content, "image/png").is_ok());

        // Test MIME type mismatch
        let text_content = b"Hello, world!";
        assert!(validator.validate_asset_content(text_content, "image/png").is_err());

        // Test disallowed MIME type
        assert!(validator.validate_asset_content(text_content, "application/x-executable").is_err());
    }

    #[test]
    fn test_xss_prevention() {
        let validator = InputValidator::new();

        // Test XSS pattern detection
        assert!(validator.validate_user_input("<script>alert('xss')</script>").is_err());
        assert!(validator.validate_user_input("javascript:alert('xss')").is_err());
        assert!(validator.validate_user_input("<img onerror='alert(1)' src='x'>").is_err());

        // Test safe input encoding
        let safe_input = validator.validate_user_input("Hello & goodbye").unwrap();
        assert!(safe_input.contains("&amp;"));
    }

    #[test]
    fn test_csrf_token_validation() {
        let validator = InputValidator::new();

        // Test token generation
        let token = validator.generate_csrf_token();
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));

        // Test token validation
        assert!(validator.validate_csrf_token(&token, "session123").is_ok());
        assert!(validator.validate_csrf_token("", "session123").is_err());
        assert!(validator.validate_csrf_token("invalid", "session123").is_err());
    }
}
