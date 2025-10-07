use regex::Regex;
use thiserror::Error;

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
pub struct InputValidator {
    uri_pattern: Regex,
    dangerous_patterns: Vec<Regex>,
    max_uri_length: usize,
    max_script_size: usize,
    max_asset_size: usize,
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

        Self {
            uri_pattern,
            dangerous_patterns,
            max_uri_length: 200,
            max_script_size: 100_000,   // 100KB
            max_asset_size: 10_000_000, // 10MB
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

    /// Validate asset content
    pub fn validate_asset_content(
        &self,
        content: &[u8],
        mimetype: &str,
    ) -> Result<(), SecurityError> {
        if content.len() > self.max_asset_size {
            return Err(SecurityError::ContentTooLarge {
                actual: content.len(),
                max: self.max_asset_size,
            });
        }

        // Validate MIME type
        if mimetype.is_empty() {
            return Err(SecurityError::InvalidUri(
                "MIME type cannot be empty".to_string(),
            ));
        }

        Ok(())
    }

    fn validate_javascript_syntax(&self, content: &str) -> Result<(), SecurityError> {
        // Check for infinite loops
        let infinite_patterns = ["while(true)", "while (true)", "for(;;)", "for (;;)"];
        for pattern in &infinite_patterns {
            if content.contains(pattern) {
                tracing::warn!("Potentially infinite loop detected: {}", pattern);
                // Note: We warn but don't block - could be legitimate
            }
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
}
