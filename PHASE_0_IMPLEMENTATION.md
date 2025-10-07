# Phase 0: Critical Security Foundation - Implementation Plan

## Overview

This document provides a concrete, step-by-step implementation plan for Phase 0 security foundations that **MUST** be completed before implementing authentication. These are critical security controls that prevent fundamental vulnerabilities.

## Timeline: Week 0 (Before Authentication Development)

**Duration**: 5-7 days  
**Priority**: CRITICAL - BLOCKING for all other development  
**Effort**: ~40 hours of focused security implementation

---

## 🚨 Day 1-2: Input Validation Framework

### Step 1.1: Create Security Module Structure

**File**: `src/security/mod.rs`
```rust
//! Security module providing input validation, output encoding, and security controls
//! 
//! This module implements defense-in-depth security controls to prevent common
//! web application vulnerabilities including injection attacks, XSS, and data exposure.

pub mod validation;
pub mod encoding;
pub mod audit;
pub mod rate_limiting;
pub mod headers;

pub use validation::{InputValidator, ValidationError, ValidationResult};
pub use encoding::{OutputEncoder, EncodingType};
pub use audit::{SecurityEvent, security_audit_log};
pub use rate_limiting::{RateLimiter, RateLimitError, RateLimitType};
pub use headers::SecurityHeaders;

/// Security configuration for the application
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SecurityConfig {
    pub enable_input_validation: bool,
    pub enable_output_encoding: bool,
    pub enable_security_headers: bool,
    pub enable_rate_limiting: bool,
    pub max_script_size_bytes: usize,
    pub max_uri_length: usize,
    pub dangerous_patterns: Vec<String>,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            enable_input_validation: true,
            enable_output_encoding: true,
            enable_security_headers: true,
            enable_rate_limiting: true,
            max_script_size_bytes: 100_000, // 100KB - much smaller than current 1MB
            max_uri_length: 2048,
            dangerous_patterns: vec![
                "eval(".to_string(),
                "Function(".to_string(),
                "setTimeout(".to_string(),
                "setInterval(".to_string(),
                "import(".to_string(),
                "require(".to_string(),
                "process.".to_string(),
                "__proto__".to_string(),
                "constructor.constructor".to_string(),
            ],
        }
    }
}
```

### Step 1.2: Implement Input Validation

**File**: `src/security/validation.rs`
```rust
use regex::Regex;
use std::collections::HashSet;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ValidationError {
    #[error("Invalid URI format: {0}")]
    InvalidUri(String),
    
    #[error("URI too long: {actual} bytes (max: {max})")]
    UriTooLong { actual: usize, max: usize },
    
    #[error("Script content too large: {actual} bytes (max: {max})")]
    ScriptTooLarge { actual: usize, max: usize },
    
    #[error("Dangerous pattern detected: {pattern}")]
    DangerousPattern { pattern: String },
    
    #[error("Invalid characters in input: {0}")]
    InvalidCharacters(String),
    
    #[error("Path traversal attempt detected")]
    PathTraversalAttempt,
    
    #[error("Potential injection detected in: {field}")]
    PotentialInjection { field: String },
}

pub type ValidationResult<T> = Result<T, ValidationError>;

pub struct InputValidator {
    safe_uri_pattern: Regex,
    dangerous_patterns: Vec<Regex>,
    max_script_size: usize,
    max_uri_length: usize,
}

impl InputValidator {
    pub fn new(config: &crate::security::SecurityConfig) -> Result<Self, ValidationError> {
        // Safe URI pattern: alphanumeric, hyphens, underscores, slashes, dots
        let safe_uri_pattern = Regex::new(r"^[a-zA-Z0-9\-_/.]+$")
            .map_err(|e| ValidationError::InvalidUri(format!("Regex error: {}", e)))?;
        
        // Compile dangerous patterns into regex
        let mut dangerous_patterns = Vec::new();
        for pattern in &config.dangerous_patterns {
            let regex = Regex::new(&regex::escape(pattern))
                .map_err(|e| ValidationError::DangerousPattern { 
                    pattern: format!("Regex compilation failed for '{}': {}", pattern, e) 
                })?;
            dangerous_patterns.push(regex);
        }
        
        Ok(Self {
            safe_uri_pattern,
            dangerous_patterns,
            max_script_size: config.max_script_size_bytes,
            max_uri_length: config.max_uri_length,
        })
    }
    
    /// Validate and sanitize a URI path
    pub fn validate_uri(&self, uri: &str) -> ValidationResult<String> {
        // Length check
        if uri.len() > self.max_uri_length {
            return Err(ValidationError::UriTooLong {
                actual: uri.len(),
                max: self.max_uri_length,
            });
        }
        
        // Must start with /
        if !uri.starts_with('/') {
            return Err(ValidationError::InvalidUri("URI must start with '/'".to_string()));
        }
        
        // Path traversal check
        if uri.contains("..") || uri.contains("./") || uri.contains("\\") {
            return Err(ValidationError::PathTraversalAttempt);
        }
        
        // Character validation
        if !self.safe_uri_pattern.is_match(uri) {
            return Err(ValidationError::InvalidCharacters(
                "URI contains invalid characters".to_string()
            ));
        }
        
        // Normalize path (remove double slashes, etc.)
        let normalized = uri.split('/')
            .filter(|segment| !segment.is_empty())
            .collect::<Vec<_>>()
            .join("/");
        
        Ok(format!("/{}", normalized))
    }
    
    /// Validate JavaScript code content for dangerous patterns
    pub fn validate_script_content(&self, content: &str) -> ValidationResult<()> {
        // Size check
        if content.len() > self.max_script_size {
            return Err(ValidationError::ScriptTooLarge {
                actual: content.len(),
                max: self.max_script_size,
            });
        }
        
        // Check for dangerous patterns
        for pattern_regex in &self.dangerous_patterns {
            if pattern_regex.is_match(content) {
                if let Some(matched) = pattern_regex.find(content) {
                    return Err(ValidationError::DangerousPattern {
                        pattern: matched.as_str().to_string(),
                    });
                }
            }
        }
        
        // Additional JavaScript-specific validation
        self.validate_javascript_ast(content)?;
        
        Ok(())
    }
    
    /// Basic AST-level validation of JavaScript (simplified)
    fn validate_javascript_ast(&self, content: &str) -> ValidationResult<()> {
        // Check for obviously dangerous constructs
        let dangerous_constructs = [
            "while(true)",
            "while (true)",
            "for(;;)",
            "for (;;)",
            "while(1)",
            "while (1)",
        ];
        
        let content_lower = content.to_lowercase();
        for construct in &dangerous_constructs {
            if content_lower.contains(construct) {
                tracing::warn!("Potentially infinite loop detected: {}", construct);
                // Note: We warn but don't block - this could be legitimate
            }
        }
        
        Ok(())
    }
    
    /// Validate form data fields
    pub fn validate_form_field(&self, field_name: &str, value: &str) -> ValidationResult<String> {
        // Length limits based on field type
        let max_length = match field_name {
            "name" | "email" => 100,
            "message" | "content" => 10_000,
            "uri" => self.max_uri_length,
            _ => 1_000, // Default limit
        };
        
        if value.len() > max_length {
            return Err(ValidationError::InvalidCharacters(
                format!("Field '{}' too long: {} > {}", field_name, value.len(), max_length)
            ));
        }
        
        // Basic injection pattern detection
        let injection_patterns = [
            "<script", "</script", "javascript:", "vbscript:", "onload=", "onerror=",
            "eval(", "alert(", "confirm(", "prompt(",
        ];
        
        let value_lower = value.to_lowercase();
        for pattern in &injection_patterns {
            if value_lower.contains(pattern) {
                return Err(ValidationError::PotentialInjection {
                    field: field_name.to_string(),
                });
            }
        }
        
        Ok(value.to_string())
    }
    
    /// Validate HTTP method
    pub fn validate_http_method(&self, method: &str) -> ValidationResult<String> {
        const ALLOWED_METHODS: &[&str] = &["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"];
        
        let method_upper = method.to_uppercase();
        if ALLOWED_METHODS.contains(&method_upper.as_str()) {
            Ok(method_upper)
        } else {
            Err(ValidationError::InvalidCharacters(
                format!("Invalid HTTP method: {}", method)
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::SecurityConfig;
    
    fn create_test_validator() -> InputValidator {
        InputValidator::new(&SecurityConfig::default()).unwrap()
    }
    
    #[test]
    fn test_uri_validation() {
        let validator = create_test_validator();
        
        // Valid URIs
        assert!(validator.validate_uri("/api/test").is_ok());
        assert!(validator.validate_uri("/users/123").is_ok());
        assert!(validator.validate_uri("/").is_ok());
        
        // Invalid URIs
        assert!(validator.validate_uri("api/test").is_err()); // No leading slash
        assert!(validator.validate_uri("/api/../etc/passwd").is_err()); // Path traversal
        assert!(validator.validate_uri("/api/test<script>").is_err()); // Invalid chars
    }
    
    #[test]
    fn test_script_validation() {
        let validator = create_test_validator();
        
        // Valid script
        assert!(validator.validate_script_content("function test() { return 'hello'; }").is_ok());
        
        // Invalid scripts
        assert!(validator.validate_script_content("eval('malicious code')").is_err());
        assert!(validator.validate_script_content("Function('return process')()").is_err());
    }
    
    #[test]
    fn test_form_field_validation() {
        let validator = create_test_validator();
        
        // Valid input
        assert!(validator.validate_form_field("name", "John Doe").is_ok());
        
        // XSS attempt
        assert!(validator.validate_form_field("name", "<script>alert('xss')</script>").is_err());
    }
}
```

### Step 1.3: Update Cargo.toml Dependencies

**File**: `Cargo.toml` (add to dependencies section)
```toml
# Security dependencies
regex = "1.10"
html-escape = "0.2"
sha2 = "0.10"
constant_time_eq = "0.3"

# Enhanced error handling
thiserror = "1.0"
```

---

## 🛡️ Day 2-3: XSS Prevention & Output Encoding

### Step 2.1: Implement Output Encoding

**File**: `src/security/encoding.rs`
```rust
use html_escape::{encode_text, encode_double_quoted_attribute};

#[derive(Debug, Clone)]
pub enum EncodingType {
    Html,
    HtmlAttribute,
    JavaScript,
    Url,
    None,
}

pub struct OutputEncoder;

impl OutputEncoder {
    /// Encode text for safe inclusion in HTML content
    pub fn encode_html(input: &str) -> String {
        encode_text(input).to_string()
    }
    
    /// Encode text for safe inclusion in HTML attributes
    pub fn encode_html_attribute(input: &str) -> String {
        encode_double_quoted_attribute(input).to_string()
    }
    
    /// Encode text for safe inclusion in JavaScript strings
    pub fn encode_javascript(input: &str) -> String {
        input
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\'', "\\'")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
            .replace('\u{2028}', "\\u2028") // Line separator
            .replace('\u{2029}', "\\u2029") // Paragraph separator
    }
    
    /// Encode text for safe inclusion in URLs
    pub fn encode_url(input: &str) -> String {
        url::form_urlencoded::byte_serialize(input.as_bytes()).collect()
    }
    
    /// Smart encoding based on context
    pub fn encode_for_context(input: &str, context: EncodingType) -> String {
        match context {
            EncodingType::Html => Self::encode_html(input),
            EncodingType::HtmlAttribute => Self::encode_html_attribute(input),
            EncodingType::JavaScript => Self::encode_javascript(input),
            EncodingType::Url => Self::encode_url(input),
            EncodingType::None => input.to_string(),
        }
    }
}

/// Safe HTML template builder with automatic encoding
pub struct SafeHtmlBuilder {
    content: String,
}

impl SafeHtmlBuilder {
    pub fn new() -> Self {
        Self {
            content: String::new(),
        }
    }
    
    /// Add safe HTML content (already trusted/encoded)
    pub fn add_html(&mut self, html: &str) -> &mut Self {
        self.content.push_str(html);
        self
    }
    
    /// Add text content with automatic HTML encoding
    pub fn add_text(&mut self, text: &str) -> &mut Self {
        self.content.push_str(&OutputEncoder::encode_html(text));
        self
    }
    
    /// Add an HTML element with encoded attributes and content
    pub fn add_element(&mut self, tag: &str, attributes: &[(&str, &str)], content: &str) -> &mut Self {
        self.content.push('<');
        self.content.push_str(tag);
        
        for (attr_name, attr_value) in attributes {
            self.content.push(' ');
            self.content.push_str(attr_name);
            self.content.push_str("=\"");
            self.content.push_str(&OutputEncoder::encode_html_attribute(attr_value));
            self.content.push('"');
        }
        
        self.content.push('>');
        self.content.push_str(&OutputEncoder::encode_html(content));
        self.content.push_str("</");
        self.content.push_str(tag);
        self.content.push('>');
        
        self
    }
    
    /// Build the final HTML string
    pub fn build(self) -> String {
        self.content
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_html_encoding() {
        assert_eq!(
            OutputEncoder::encode_html("<script>alert('xss')</script>"),
            "&lt;script&gt;alert(&#x27;xss&#x27;)&lt;/script&gt;"
        );
    }
    
    #[test]
    fn test_safe_html_builder() {
        let html = SafeHtmlBuilder::new()
            .add_element("h1", &[], "Welcome")
            .add_element("p", &[("class", "user-name")], "<script>alert('xss')</script>")
            .build();
        
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }
}
```

### Step 2.2: Create Security Headers Middleware

**File**: `src/security/headers.rs`
```rust
use axum::{
    extract::Request,
    http::{HeaderName, HeaderValue},
    middleware::Next,
    response::Response,
};

pub struct SecurityHeaders;

impl SecurityHeaders {
    /// Apply comprehensive security headers to response
    pub async fn middleware(request: Request, next: Next) -> Response {
        let mut response = next.run(request).await;
        
        let headers = response.headers_mut();
        
        // Prevent clickjacking
        if !headers.contains_key("x-frame-options") {
            headers.insert(
                HeaderName::from_static("x-frame-options"),
                HeaderValue::from_static("DENY"),
            );
        }
        
        // Prevent MIME type sniffing
        headers.insert(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        );
        
        // XSS Protection (for older browsers)
        headers.insert(
            HeaderName::from_static("x-xss-protection"),
            HeaderValue::from_static("1; mode=block"),
        );
        
        // Content Security Policy
        if !headers.contains_key("content-security-policy") {
            let csp = concat!(
                "default-src 'self'; ",
                "script-src 'self' 'unsafe-inline'; ",
                "style-src 'self' 'unsafe-inline'; ",
                "img-src 'self' data: https:; ",
                "font-src 'self'; ",
                "connect-src 'self'; ",
                "object-src 'none'; ",
                "base-uri 'self'; ",
                "form-action 'self'"
            );
            
            headers.insert(
                HeaderName::from_static("content-security-policy"),
                HeaderValue::from_static(csp),
            );
        }
        
        // Referrer Policy
        headers.insert(
            HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        );
        
        // Permissions Policy (Feature Policy replacement)
        headers.insert(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static("camera=(), microphone=(), geolocation=(), payment=()"),
        );
        
        // HSTS (only for HTTPS)
        if request.uri().scheme_str() == Some("https") {
            headers.insert(
                HeaderName::from_static("strict-transport-security"),
                HeaderValue::from_static("max-age=31536000; includeSubDomains; preload"),
            );
        }
        
        response
    }
}
```

---

## 📊 Day 3-4: Security Monitoring & Audit Logging

### Step 3.1: Implement Security Event Logging

**File**: `src/security/audit.rs`
```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use tracing::{error, info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SecurityEvent {
    InputValidationFailure {
        ip: String,
        user_agent: Option<String>,
        field: String,
        error: String,
        attempted_value: String, // Truncated for safety
    },
    
    XssAttempt {
        ip: String,
        user_agent: Option<String>,
        field: String,
        pattern_detected: String,
    },
    
    PathTraversalAttempt {
        ip: String,
        user_agent: Option<String>,
        attempted_path: String,
    },
    
    DangerousScriptSubmission {
        ip: String,
        user_agent: Option<String>,
        pattern_detected: String,
        script_uri: String,
    },
    
    RateLimitExceeded {
        ip: String,
        endpoint: String,
        limit_type: String,
        attempts: u32,
    },
    
    SuspiciousActivity {
        ip: String,
        user_agent: Option<String>,
        description: String,
        severity: SecuritySeverity,
    },
    
    SystemSecurityEvent {
        event_type: String,
        description: String,
        severity: SecuritySeverity,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SecuritySeverity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityEventRecord {
    pub timestamp: DateTime<Utc>,
    pub event: SecurityEvent,
    pub request_id: Option<String>,
    pub correlation_id: String,
}

impl SecurityEventRecord {
    pub fn new(event: SecurityEvent, request_id: Option<String>) -> Self {
        Self {
            timestamp: Utc::now(),
            event,
            request_id,
            correlation_id: uuid::Uuid::new_v4().to_string(),
        }
    }
}

/// Log security events with appropriate severity
pub fn security_audit_log(event: SecurityEvent) {
    let record = SecurityEventRecord::new(event.clone(), None);
    
    match &event {
        SecurityEvent::InputValidationFailure { ip, field, error, attempted_value, .. } => {
            warn!(
                event = "input_validation_failure",
                ip = %ip,
                field = %field,
                error = %error,
                attempted_value = %truncate_for_log(attempted_value),
                correlation_id = %record.correlation_id,
                "Input validation failure detected"
            );
        },
        
        SecurityEvent::XssAttempt { ip, field, pattern_detected, .. } => {
            error!(
                event = "xss_attempt",
                ip = %ip,
                field = %field,
                pattern = %pattern_detected,
                correlation_id = %record.correlation_id,
                "XSS attempt detected"
            );
        },
        
        SecurityEvent::PathTraversalAttempt { ip, attempted_path, .. } => {
            error!(
                event = "path_traversal_attempt",
                ip = %ip,
                path = %attempted_path,
                correlation_id = %record.correlation_id,
                "Path traversal attempt detected"
            );
        },
        
        SecurityEvent::DangerousScriptSubmission { ip, pattern_detected, script_uri, .. } => {
            error!(
                event = "dangerous_script_submission",
                ip = %ip,
                pattern = %pattern_detected,
                script_uri = %script_uri,
                correlation_id = %record.correlation_id,
                "Dangerous script submission detected"
            );
        },
        
        SecurityEvent::RateLimitExceeded { ip, endpoint, limit_type, attempts } => {
            warn!(
                event = "rate_limit_exceeded",
                ip = %ip,
                endpoint = %endpoint,
                limit_type = %limit_type,
                attempts = %attempts,
                correlation_id = %record.correlation_id,
                "Rate limit exceeded"
            );
        },
        
        SecurityEvent::SuspiciousActivity { ip, description, severity, .. } => {
            let level = match severity {
                SecuritySeverity::Low => tracing::Level::INFO,
                SecuritySeverity::Medium => tracing::Level::WARN,
                SecuritySeverity::High | SecuritySeverity::Critical => tracing::Level::ERROR,
            };
            
            tracing::event!(
                level,
                event = "suspicious_activity",
                ip = %ip,
                description = %description,
                severity = ?severity,
                correlation_id = %record.correlation_id,
                "Suspicious activity detected"
            );
        },
        
        SecurityEvent::SystemSecurityEvent { event_type, description, severity } => {
            let level = match severity {
                SecuritySeverity::Low => tracing::Level::INFO,
                SecuritySeverity::Medium => tracing::Level::WARN,
                SecuritySeverity::High | SecuritySeverity::Critical => tracing::Level::ERROR,
            };
            
            tracing::event!(
                level,
                event = "system_security_event",
                event_type = %event_type,
                description = %description,
                severity = ?severity,
                correlation_id = %record.correlation_id,
                "System security event"
            );
        },
    }
}

/// Truncate sensitive data for logging while preserving useful information
fn truncate_for_log(value: &str) -> String {
    if value.len() <= 100 {
        value.to_string()
    } else {
        format!("{}... (truncated from {} chars)", &value[..100], value.len())
    }
}

/// Extract client IP from request headers with X-Forwarded-For support
pub fn extract_client_ip(headers: &axum::http::HeaderMap) -> String {
    // Check X-Forwarded-For header first (for reverse proxies)
    if let Some(forwarded) = headers.get("x-forwarded-for") {
        if let Ok(forwarded_str) = forwarded.to_str() {
            // Take the first IP (original client)
            if let Some(first_ip) = forwarded_str.split(',').next() {
                return first_ip.trim().to_string();
            }
        }
    }
    
    // Check X-Real-IP header
    if let Some(real_ip) = headers.get("x-real-ip") {
        if let Ok(ip_str) = real_ip.to_str() {
            return ip_str.to_string();
        }
    }
    
    // Fallback to "unknown" - actual IP extraction requires connection info
    "unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_truncate_for_log() {
        let short = "short string";
        assert_eq!(truncate_for_log(short), short);
        
        let long = "a".repeat(200);
        let truncated = truncate_for_log(&long);
        assert!(truncated.len() < long.len());
        assert!(truncated.contains("truncated"));
    }
}
```

---

## ⚡ Day 4-5: Rate Limiting & Integration

### Step 4.1: Implement Rate Limiting

**File**: `src/security/rate_limiting.rs`
```rust
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RateLimitError {
    #[error("Rate limit exceeded for {limit_type}: {current}/{max} requests")]
    LimitExceeded {
        limit_type: String,
        current: u32,
        max: u32,
    },
    
    #[error("Rate limiter configuration error: {0}")]
    ConfigError(String),
}

#[derive(Debug, Clone)]
pub enum RateLimitType {
    General,
    ScriptUpload,
    Authentication,
    AssetUpload,
}

impl RateLimitType {
    pub fn as_str(&self) -> &'static str {
        match self {
            RateLimitType::General => "general",
            RateLimitType::ScriptUpload => "script_upload",
            RateLimitType::Authentication => "authentication",
            RateLimitType::AssetUpload => "asset_upload",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    pub general_requests_per_minute: u32,
    pub script_upload_requests_per_minute: u32,
    pub auth_requests_per_minute: u32,
    pub asset_upload_requests_per_minute: u32,
    pub window_duration: Duration,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            general_requests_per_minute: 100,
            script_upload_requests_per_minute: 10,
            auth_requests_per_minute: 20,
            asset_upload_requests_per_minute: 20,
            window_duration: Duration::from_secs(60),
        }
    }
}

#[derive(Debug)]
struct AttemptTracker {
    attempts: Vec<Instant>,
    first_attempt: Instant,
}

impl AttemptTracker {
    fn new() -> Self {
        Self {
            attempts: Vec::new(),
            first_attempt: Instant::now(),
        }
    }
    
    fn add_attempt(&mut self, now: Instant) {
        self.attempts.push(now);
    }
    
    fn cleanup_old_attempts(&mut self, window: Duration) {
        let cutoff = Instant::now() - window;
        self.attempts.retain(|&attempt| attempt > cutoff);
    }
    
    fn attempt_count(&self) -> u32 {
        self.attempts.len() as u32
    }
}

pub struct RateLimiter {
    trackers: Arc<RwLock<HashMap<String, AttemptTracker>>>,
    config: RateLimitConfig,
}

impl RateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            trackers: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }
    
    /// Check if request is within rate limits
    pub async fn check_rate_limit(&self, key: &str, limit_type: RateLimitType) -> Result<(), RateLimitError> {
        let now = Instant::now();
        let max_requests = match limit_type {
            RateLimitType::General => self.config.general_requests_per_minute,
            RateLimitType::ScriptUpload => self.config.script_upload_requests_per_minute,
            RateLimitType::Authentication => self.config.auth_requests_per_minute,
            RateLimitType::AssetUpload => self.config.asset_upload_requests_per_minute,
        };
        
        let mut trackers = self.trackers.write().await;
        let tracker = trackers
            .entry(key.to_string())
            .or_insert_with(AttemptTracker::new);
        
        // Clean up old attempts
        tracker.cleanup_old_attempts(self.config.window_duration);
        
        // Check if we're over the limit
        if tracker.attempt_count() >= max_requests {
            // Log the rate limit event
            crate::security::audit::security_audit_log(
                crate::security::audit::SecurityEvent::RateLimitExceeded {
                    ip: key.to_string(),
                    endpoint: limit_type.as_str().to_string(),
                    limit_type: limit_type.as_str().to_string(),
                    attempts: tracker.attempt_count(),
                }
            );
            
            return Err(RateLimitError::LimitExceeded {
                limit_type: limit_type.as_str().to_string(),
                current: tracker.attempt_count(),
                max: max_requests,
            });
        }
        
        // Add this attempt
        tracker.add_attempt(now);
        
        Ok(())
    }
    
    /// Clean up old tracking data periodically
    pub async fn cleanup_old_trackers(&self) {
        let cutoff = Instant::now() - Duration::from_secs(3600); // 1 hour
        let mut trackers = self.trackers.write().await;
        
        trackers.retain(|_, tracker| tracker.first_attempt > cutoff);
    }
}

/// Rate limiting middleware for Axum
pub async fn rate_limiting_middleware(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    use axum::http::StatusCode;
    
    // Extract IP address for rate limiting key
    let ip = crate::security::audit::extract_client_ip(request.headers());
    
    // Determine rate limit type based on path
    let limit_type = determine_rate_limit_type(request.uri().path());
    
    // TODO: Get rate limiter from app state
    // For now, create a default one (in real implementation, this should be in app state)
    let rate_limiter = RateLimiter::new(RateLimitConfig::default());
    
    // Check rate limit
    match rate_limiter.check_rate_limit(&ip, limit_type).await {
        Ok(()) => {
            // Rate limit OK, continue to next middleware
            next.run(request).await
        },
        Err(RateLimitError::LimitExceeded { .. }) => {
            // Rate limit exceeded, return 429 Too Many Requests
            (StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded").into_response()
        },
        Err(_) => {
            // Other rate limiting error, return 500
            (StatusCode::INTERNAL_SERVER_ERROR, "Rate limiting error").into_response()
        }
    }
}

fn determine_rate_limit_type(path: &str) -> RateLimitType {
    if path.contains("/api/script") || path.contains("/upsert_script") {
        RateLimitType::ScriptUpload
    } else if path.contains("/auth/") || path.contains("/login") {
        RateLimitType::Authentication
    } else if path.contains("/api/asset") {
        RateLimitType::AssetUpload
    } else {
        RateLimitType::General
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio;
    
    #[tokio::test]
    async fn test_rate_limiting() {
        let config = RateLimitConfig {
            general_requests_per_minute: 2,
            ..Default::default()
        };
        
        let limiter = RateLimiter::new(config);
        
        // First request should pass
        assert!(limiter.check_rate_limit("test_ip", RateLimitType::General).await.is_ok());
        
        // Second request should pass
        assert!(limiter.check_rate_limit("test_ip", RateLimitType::General).await.is_ok());
        
        // Third request should fail
        assert!(limiter.check_rate_limit("test_ip", RateLimitType::General).await.is_err());
    }
}
```

### Step 4.2: Update Main Application Integration

**File**: `src/lib.rs` (add security module and update imports)
```rust
// Add to existing imports at top of file
pub mod security;

// Add to existing modules
use security::{
    InputValidator, OutputEncoder, SecurityHeaders,
    security_audit_log, SecurityEvent, SecuritySeverity
};
```

### Step 4.3: Integrate Security into JavaScript Engine

**File**: `src/js_engine.rs` (update the existing script validation function)
```rust
// Replace the existing validate_script function with this enhanced version
fn validate_script(content: &str, limits: &ExecutionLimits) -> Result<(), String> {
    // Create input validator with security config
    let security_config = crate::security::SecurityConfig::default();
    let validator = crate::security::InputValidator::new(&security_config)
        .map_err(|e| format!("Security validator initialization failed: {}", e))?;
    
    // Use the comprehensive validation
    validator.validate_script_content(content)
        .map_err(|e| format!("Script validation failed: {}", e))?;
    
    // Original size check (now redundant but keeping for compatibility)
    if content.len() > limits.max_script_size_bytes {
        return Err(format!(
            "Script too large: {} bytes (max: {})",
            content.len(),
            limits.max_script_size_bytes
        ));
    }
    
    Ok(())
}
```

### Step 4.4: Secure the Script Management Handlers

**File**: `scripts/feature_scripts/core.js` (update the upsert_script_handler function)
```javascript
// Replace the existing upsert_script_handler function
function upsert_script_handler(req) {
    try {
        // Extract and validate parameters
        let uri = null;
        let content = null;
        
        if (req.form) {
            uri = req.form.uri;
            content = req.form.content;
        }
        
        // Fallback to query parameters if form data is not available
        if (!uri && req.query) {
            uri = req.query.uri;
        }
        if (!content && req.query) {
            content = req.query.content;
        }
        
        // Enhanced validation
        if (!uri || typeof uri !== 'string') {
            return {
                status: 400,
                body: JSON.stringify({
                    error: 'Missing or invalid required parameter: uri',
                    timestamp: new Date().toISOString()
                }),
                contentType: 'application/json'
            };
        }
        
        if (!content || typeof content !== 'string') {
            return {
                status: 400,
                body: JSON.stringify({
                    error: 'Missing or invalid required parameter: content',
                    timestamp: new Date().toISOString()
                }),
                contentType: 'application/json'
            };
        }
        
        // Basic security validation (additional validation happens in Rust layer)
        if (uri.length > 200) {
            return {
                status: 400,
                body: JSON.stringify({
                    error: 'URI too long (max 200 characters)',
                    timestamp: new Date().toISOString()
                }),
                contentType: 'application/json'
            };
        }
        
        if (content.length > 100000) { // 100KB limit
            return {
                status: 400,
                body: JSON.stringify({
                    error: 'Script content too large (max 100KB)',
                    timestamp: new Date().toISOString()
                }),
                contentType: 'application/json'
            };
        }
        
        // Check for basic dangerous patterns
        const dangerousPatterns = [
            'eval(', 'Function(', 'setTimeout(', 'setInterval(',
            'import(', 'require(', 'process.', '__proto__'
        ];
        
        for (const pattern of dangerousPatterns) {
            if (content.includes(pattern)) {
                return {
                    status: 400,
                    body: JSON.stringify({
                        error: `Dangerous pattern detected: ${pattern}`,
                        timestamp: new Date().toISOString()
                    }),
                    contentType: 'application/json'
                };
            }
        }
        
        // Log the script update attempt (sanitized)
        writeLog(`Script upsert attempt: URI=${uri}, content_length=${content.length}`);
        
        // Proceed with upsert
        upsertScript(uri, content);
        
        const response = {
            message: 'Script upserted successfully',
            uri: uri,
            chars: content.length,
            success: true,
            timestamp: new Date().toISOString()
        };
        
        writeLog(`Script upserted successfully: ${uri}`);
        
        return {
            status: 200,
            body: JSON.stringify(response),
            contentType: 'application/json'
        };
        
    } catch (error) {
        writeLog(`Script upsert error: ${error.message}`);
        
        return {
            status: 500,
            body: JSON.stringify({
                error: 'Internal server error during script upsert',
                timestamp: new Date().toISOString()
            }),
            contentType: 'application/json'
        };
    }
}
```

---

## 🔗 Day 5: Integration & Testing

### Step 5.1: Update Server Configuration

**File**: `src/bin/server.rs` (add security middleware)
```rust
// Add to imports
use aiwebengine::security::SecurityHeaders;

// In the main function, add security middleware to the router
// (This will be integrated into the main server setup)
```

### Step 5.2: Add Security Configuration

**File**: `config.example.yaml` (add security section)
```yaml
# Add to existing configuration
security:
  enable_input_validation: true
  enable_output_encoding: true
  enable_security_headers: true
  enable_rate_limiting: true
  max_script_size_bytes: 100000  # 100KB
  max_uri_length: 2048
  rate_limiting:
    general_requests_per_minute: 100
    script_upload_requests_per_minute: 10
    auth_requests_per_minute: 20
    asset_upload_requests_per_minute: 20
```

### Step 5.3: Create Security Tests

**File**: `tests/security_integration.rs`
```rust
//! Security integration tests for Phase 0 implementation

use aiwebengine::security::*;

#[tokio::test]
async fn test_input_validation_integration() {
    let config = SecurityConfig::default();
    let validator = InputValidator::new(&config).unwrap();
    
    // Test XSS prevention
    let xss_attempt = "<script>alert('xss')</script>";
    assert!(validator.validate_form_field("name", xss_attempt).is_err());
    
    // Test path traversal prevention
    assert!(validator.validate_uri("/api/../etc/passwd").is_err());
    
    // Test dangerous script patterns
    assert!(validator.validate_script_content("eval('malicious')").is_err());
}

#[tokio::test]
async fn test_rate_limiting_integration() {
    let config = RateLimitConfig {
        general_requests_per_minute: 2,
        ..Default::default()
    };
    
    let limiter = RateLimiter::new(config);
    
    // Should allow first two requests
    assert!(limiter.check_rate_limit("test", RateLimitType::General).await.is_ok());
    assert!(limiter.check_rate_limit("test", RateLimitType::General).await.is_ok());
    
    // Should block third request
    assert!(limiter.check_rate_limit("test", RateLimitType::General).await.is_err());
}

#[test]
fn test_output_encoding() {
    let xss = "<script>alert('xss')</script>";
    let encoded = OutputEncoder::encode_html(xss);
    
    assert!(!encoded.contains("<script>"));
    assert!(encoded.contains("&lt;script&gt;"));
}

#[test]
fn test_safe_html_builder() {
    let mut builder = SafeHtmlBuilder::new();
    
    let html = builder
        .add_element("div", &[("class", "user-content")], "<script>alert('xss')</script>")
        .build();
    
    // Should not contain executable script
    assert!(!html.contains("<script>alert('xss')</script>"));
    // Should contain encoded version
    assert!(html.contains("&lt;script&gt;"));
}
```

---

## 📋 Implementation Checklist

### Day 1-2: Input Validation ✅
- [ ] Create `src/security/mod.rs` with module structure
- [ ] Implement comprehensive `InputValidator` in `src/security/validation.rs`
- [ ] Add security dependencies to `Cargo.toml`
- [ ] Test URI validation, script content validation, form field validation
- [ ] Update `js_engine.rs` to use new validation

### Day 2-3: XSS Prevention ✅
- [ ] Implement `OutputEncoder` in `src/security/encoding.rs`
- [ ] Create `SafeHtmlBuilder` for secure HTML generation
- [ ] Implement security headers middleware in `src/security/headers.rs`
- [ ] Update JavaScript handlers to use output encoding
- [ ] Test XSS prevention mechanisms

### Day 3-4: Security Monitoring ✅
- [ ] Implement security event logging in `src/security/audit.rs`
- [ ] Create comprehensive security event types
- [ ] Add IP extraction and event correlation
- [ ] Test security event logging and structured logging
- [ ] Integrate with existing tracing infrastructure

### Day 4-5: Rate Limiting & Integration ✅
- [ ] Implement rate limiting in `src/security/rate_limiting.rs`
- [ ] Create rate limiting middleware
- [ ] Update script management handlers with security
- [ ] Add security configuration to config files
- [ ] Create comprehensive security integration tests

### Final Integration ✅
- [ ] Update main server to use security middleware
- [ ] Verify all security controls are active
- [ ] Run complete security test suite
- [ ] Document security implementation
- [ ] Create security monitoring dashboard (optional)

---

## ⚠️ Critical Success Criteria

### Before Proceeding to Authentication Implementation:
1. **All input validation tests pass** - No injection vectors remain
2. **XSS protection verified** - Output encoding working correctly
3. **Rate limiting functional** - Brute force protection in place
4. **Security logging operational** - Visibility into security events
5. **Security headers applied** - Basic hardening measures active

### Security Validation Tests:
1. **Injection Testing**: Attempt SQL, script, and command injection
2. **XSS Testing**: Try reflected and stored XSS attacks
3. **Path Traversal Testing**: Attempt directory traversal attacks
4. **Rate Limit Testing**: Verify rate limiting blocks excessive requests
5. **Header Testing**: Confirm all security headers are present

Only after these tests pass should authentication development begin. This Phase 0 creates the security foundation that makes the authentication system secure from day one.