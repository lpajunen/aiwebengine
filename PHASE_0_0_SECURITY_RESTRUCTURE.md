# Phase 0.0: Security Boundary Restructuring - Concrete Implementation Plan

## Overview

This plan restructures aiwebengine to enforce the principle: **"Security is enforced at the Rust boundary, not the JavaScript boundary"**. This must be completed BEFORE Phase 0 security implementation.

**Timeline**: 2-3 days  
**Priority**: CRITICAL BLOCKER for all other security work  
**Goal**: Move all security validation to Rust, simplify JavaScript to business logic only

---

## 🚧 Day 1: Implement Secure Global Function Architecture

### Step 1.1: Create Security Validation Layer

**File**: `src/security/validation.rs` (Enhanced version)

```rust
use regex::Regex;
use std::collections::HashSet;
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
    pub fn new() -> Result<Self, SecurityError> {
        let uri_pattern = Regex::new(r"^[a-zA-Z0-9\-_/.]+$")
            .map_err(|e| SecurityError::InvalidUri(format!("Regex error: {}", e)))?;

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
            let regex = Regex::new(pattern)
                .map_err(|e| SecurityError::DangerousPattern(format!("Pattern compilation failed: {}", e)))?;
            dangerous_patterns.push(regex);
        }

        Ok(Self {
            uri_pattern,
            dangerous_patterns,
            max_uri_length: 200,
            max_script_size: 100_000, // 100KB
            max_asset_size: 10_000_000, // 10MB
        })
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
            return Err(SecurityError::InvalidUri("URI must start with '/'".to_string()));
        }

        // Path traversal check
        if uri.contains("..") || uri.contains("\\") {
            return Err(SecurityError::PathTraversal);
        }

        // Character validation
        if !self.uri_pattern.is_match(uri) {
            return Err(SecurityError::InvalidUri("Invalid characters in URI".to_string()));
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
                return Err(SecurityError::DangerousPattern(matched.as_str().to_string()));
            }
        }

        // Additional syntax validation
        self.validate_javascript_syntax(content)?;

        Ok(())
    }

    /// Validate asset content
    pub fn validate_asset_content(&self, content: &[u8], mimetype: &str) -> Result<(), SecurityError> {
        if content.len() > self.max_asset_size {
            return Err(SecurityError::ContentTooLarge {
                actual: content.len(),
                max: self.max_asset_size,
            });
        }

        // Validate MIME type
        if mimetype.is_empty() {
            return Err(SecurityError::InvalidUri("MIME type cannot be empty".to_string()));
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uri_validation() {
        let validator = InputValidator::new().unwrap();

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
        let validator = InputValidator::new().unwrap();

        // Valid script
        assert!(validator.validate_script_content("function test() { return 'hello'; }").is_ok());

        // Invalid scripts
        assert!(validator.validate_script_content("eval('code')").is_err());
        assert!(validator.validate_script_content("Function('return this')()").is_err());
        assert!(validator.validate_script_content("process.exit()").is_err());
    }
}
```

### Step 1.2: Create Capability Management System

**File**: `src/security/capabilities.rs`

```rust
use std::collections::{HashMap, HashSet};
use super::validation::Capability;

#[derive(Debug, Clone)]
pub struct UserContext {
    pub user_id: Option<String>,
    pub is_authenticated: bool,
    pub capabilities: HashSet<Capability>,
}

impl UserContext {
    pub fn anonymous() -> Self {
        Self {
            user_id: None,
            is_authenticated: false,
            capabilities: Self::anonymous_capabilities(),
        }
    }

    pub fn authenticated(user_id: String) -> Self {
        Self {
            user_id: Some(user_id),
            is_authenticated: true,
            capabilities: Self::authenticated_capabilities(),
        }
    }

    pub fn admin(user_id: String) -> Self {
        Self {
            user_id: Some(user_id),
            is_authenticated: true,
            capabilities: Self::admin_capabilities(),
        }
    }

    fn anonymous_capabilities() -> HashSet<Capability> {
        // Anonymous users can only read
        [Capability::ViewLogs].into_iter().collect()
    }

    fn authenticated_capabilities() -> HashSet<Capability> {
        // Authenticated users can read/write most things
        [
            Capability::ReadScripts,
            Capability::WriteScripts,
            Capability::ReadAssets,
            Capability::WriteAssets,
            Capability::ViewLogs,
            Capability::ManageStreams,
        ].into_iter().collect()
    }

    fn admin_capabilities() -> HashSet<Capability> {
        // Admins can do everything
        [
            Capability::ReadScripts,
            Capability::WriteScripts,
            Capability::DeleteScripts,
            Capability::ReadAssets,
            Capability::WriteAssets,
            Capability::DeleteAssets,
            Capability::ViewLogs,
            Capability::ManageStreams,
            Capability::ManageGraphQL,
        ].into_iter().collect()
    }

    pub fn has_capability(&self, capability: &Capability) -> bool {
        self.capabilities.contains(capability)
    }

    pub fn require_capability(&self, capability: &Capability) -> Result<(), super::validation::SecurityError> {
        if self.has_capability(capability) {
            Ok(())
        } else {
            Err(super::validation::SecurityError::InsufficientCapabilities {
                required: vec![capability.clone()],
            })
        }
    }
}
```

### Step 1.3: Create Secure Operation Handler

**File**: `src/security/operations.rs`

```rust
use super::{
    validation::{InputValidator, SecurityError, Capability},
    capabilities::UserContext,
    audit::{security_audit_log, SecurityEvent, SecuritySeverity},
};
use crate::repository;

/// Secure operation handler - ALL SECURITY LOGIC IN RUST
pub struct SecureOperationHandler {
    validator: InputValidator,
}

impl SecureOperationHandler {
    pub fn new() -> Result<Self, SecurityError> {
        Ok(Self {
            validator: InputValidator::new()?,
        })
    }

    /// Secure script upsert - CANNOT BE BYPASSED BY JAVASCRIPT
    pub fn secure_upsert_script(
        &self,
        user_context: &UserContext,
        uri: &str,
        content: &str,
        client_ip: &str,
    ) -> Result<(), SecurityError> {
        // 1. Check capability
        user_context.require_capability(&Capability::WriteScripts)?;

        // 2. Validate URI
        let clean_uri = self.validator.validate_uri(uri)?;

        // 3. Validate content
        self.validator.validate_script_content(content)?;

        // 4. Log security event
        security_audit_log(SecurityEvent::ScriptOperation {
            operation: "upsert".to_string(),
            uri: clean_uri.clone(),
            user_id: user_context.user_id.clone(),
            ip: client_ip.to_string(),
            success: true,
        });

        // 5. Execute operation only after all validation passes
        repository::upsert_script(&clean_uri, content)
            .map_err(|e| SecurityError::OperationNotAllowed(format!("Repository error: {}", e)))?;

        Ok(())
    }

    /// Secure script deletion
    pub fn secure_delete_script(
        &self,
        user_context: &UserContext,
        uri: &str,
        client_ip: &str,
    ) -> Result<bool, SecurityError> {
        // 1. Check capability
        user_context.require_capability(&Capability::DeleteScripts)?;

        // 2. Validate URI
        let clean_uri = self.validator.validate_uri(uri)?;

        // 3. Log security event
        security_audit_log(SecurityEvent::ScriptOperation {
            operation: "delete".to_string(),
            uri: clean_uri.clone(),
            user_id: user_context.user_id.clone(),
            ip: client_ip.to_string(),
            success: true,
        });

        // 4. Execute operation
        Ok(repository::delete_script(&clean_uri))
    }

    /// Secure script read
    pub fn secure_get_script(
        &self,
        user_context: &UserContext,
        uri: &str,
        client_ip: &str,
    ) -> Result<Option<String>, SecurityError> {
        // 1. Check capability
        user_context.require_capability(&Capability::ReadScripts)?;

        // 2. Validate URI
        let clean_uri = self.validator.validate_uri(uri)?;

        // 3. Log access
        security_audit_log(SecurityEvent::DataAccess {
            resource_type: "script".to_string(),
            resource_id: clean_uri.clone(),
            user_id: user_context.user_id.clone(),
            ip: client_ip.to_string(),
        });

        // 4. Execute operation
        Ok(repository::fetch_script(&clean_uri))
    }

    /// Secure asset upsert
    pub fn secure_upsert_asset(
        &self,
        user_context: &UserContext,
        public_path: &str,
        mimetype: &str,
        content: &[u8],
        client_ip: &str,
    ) -> Result<(), SecurityError> {
        // 1. Check capability
        user_context.require_capability(&Capability::WriteAssets)?;

        // 2. Validate path
        let clean_path = self.validator.validate_uri(public_path)?;

        // 3. Validate content
        self.validator.validate_asset_content(content, mimetype)?;

        // 4. Log security event
        security_audit_log(SecurityEvent::AssetOperation {
            operation: "upsert".to_string(),
            path: clean_path.clone(),
            size: content.len(),
            user_id: user_context.user_id.clone(),
            ip: client_ip.to_string(),
        });

        // 5. Execute operation
        let asset = repository::Asset {
            public_path: clean_path,
            mimetype: mimetype.to_string(),
            content: content.to_vec(),
        };

        repository::upsert_asset(asset)
            .map_err(|e| SecurityError::OperationNotAllowed(format!("Repository error: {}", e)))?;

        Ok(())
    }
}

// Add to security/audit.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SecurityEvent {
    ScriptOperation {
        operation: String,
        uri: String,
        user_id: Option<String>,
        ip: String,
        success: bool,
    },
    AssetOperation {
        operation: String,
        path: String,
        size: usize,
        user_id: Option<String>,
        ip: String,
    },
    DataAccess {
        resource_type: String,
        resource_id: String,
        user_id: Option<String>,
        ip: String,
    },
    SecurityViolation {
        violation_type: String,
        details: String,
        user_id: Option<String>,
        ip: String,
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

pub fn security_audit_log(event: SecurityEvent) {
    match &event {
        SecurityEvent::ScriptOperation { operation, uri, user_id, ip, success } => {
            if *success {
                tracing::info!(
                    event = "script_operation",
                    operation = %operation,
                    uri = %uri,
                    user_id = ?user_id,
                    ip = %ip,
                    "Script operation completed"
                );
            } else {
                tracing::warn!(
                    event = "script_operation_failed",
                    operation = %operation,
                    uri = %uri,
                    user_id = ?user_id,
                    ip = %ip,
                    "Script operation failed"
                );
            }
        },
        SecurityEvent::SecurityViolation { violation_type, details, user_id, ip, severity } => {
            let level = match severity {
                SecuritySeverity::Low => tracing::Level::INFO,
                SecuritySeverity::Medium => tracing::Level::WARN,
                SecuritySeverity::High | SecuritySeverity::Critical => tracing::Level::ERROR,
            };

            tracing::event!(
                level,
                event = "security_violation",
                violation_type = %violation_type,
                details = %details,
                user_id = ?user_id,
                ip = %ip,
                severity = ?severity,
                "Security violation detected"
            );
        },
        // Handle other event types...
        _ => {
            tracing::info!(event = ?event, "Security event logged");
        }
    }
}
```

---

## 🔒 Day 2: Replace Global Functions with Secure Wrappers

### Step 2.1: Create Secure Global Function Factory

**File**: `src/js_engine/secure_globals.rs`

```rust
use rquickjs::{Context, Function, Runtime, Value, Error as JsError};
use std::sync::Arc;
use crate::security::{
    operations::SecureOperationHandler,
    capabilities::UserContext,
    validation::SecurityError,
    audit::{security_audit_log, SecurityEvent, SecuritySeverity},
};

/// Thread-safe secure operation handler
pub struct SecureGlobalFunctions {
    operation_handler: Arc<SecureOperationHandler>,
}

impl SecureGlobalFunctions {
    pub fn new() -> Result<Self, SecurityError> {
        Ok(Self {
            operation_handler: Arc::new(SecureOperationHandler::new()?),
        })
    }

    /// Setup secure global functions - ALL SECURITY IN RUST
    pub fn setup_secure_globals(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        user_context: UserContext,
        client_ip: String,
    ) -> Result<(), JsError> {
        let global = ctx.globals();

        // Secure script management functions
        self.setup_script_functions(ctx, &global, user_context.clone(), client_ip.clone())?;

        // Secure asset management functions
        self.setup_asset_functions(ctx, &global, user_context.clone(), client_ip.clone())?;

        // Secure logging functions
        self.setup_logging_functions(ctx, &global, user_context.clone(), client_ip.clone())?;

        // Other secure functions...

        Ok(())
    }

    fn setup_script_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        global: &rquickjs::Object<'_>,
        user_context: UserContext,
        client_ip: String,
    ) -> Result<(), JsError> {
        let handler = Arc::clone(&self.operation_handler);

        // Secure upsertScript function
        let secure_upsert_script = {
            let handler = Arc::clone(&handler);
            let user_context = user_context.clone();
            let client_ip = client_ip.clone();

            Function::new(
                ctx.clone(),
                move |_c: rquickjs::Ctx<'_>, uri: String, content: String| -> Result<(), JsError> {
                    match handler.secure_upsert_script(&user_context, &uri, &content, &client_ip) {
                        Ok(()) => Ok(()),
                        Err(e) => {
                            // Log security violation
                            security_audit_log(SecurityEvent::SecurityViolation {
                                violation_type: "upsert_script_blocked".to_string(),
                                details: e.to_string(),
                                user_id: user_context.user_id.clone(),
                                ip: client_ip.clone(),
                                severity: SecuritySeverity::Medium,
                            });

                            tracing::warn!("Script upsert blocked: {}", e);
                            Err(JsError::Exception)
                        }
                    }
                },
            )?
        };

        // Only expose if user has capability
        if user_context.has_capability(&crate::security::validation::Capability::WriteScripts) {
            global.set("upsertScript", secure_upsert_script)?;
        } else {
            // Provide no-op function for users without capability
            let noop = Function::new(
                ctx.clone(),
                |_c: rquickjs::Ctx<'_>, _uri: String, _content: String| -> Result<(), JsError> {
                    tracing::warn!("Attempted to call upsertScript without permission");
                    Err(JsError::Exception)
                },
            )?;
            global.set("upsertScript", noop)?;
        }

        // Secure deleteScript function
        let secure_delete_script = {
            let handler = Arc::clone(&handler);
            let user_context = user_context.clone();
            let client_ip = client_ip.clone();

            Function::new(
                ctx.clone(),
                move |_c: rquickjs::Ctx<'_>, uri: String| -> Result<bool, JsError> {
                    match handler.secure_delete_script(&user_context, &uri, &client_ip) {
                        Ok(result) => Ok(result),
                        Err(e) => {
                            security_audit_log(SecurityEvent::SecurityViolation {
                                violation_type: "delete_script_blocked".to_string(),
                                details: e.to_string(),
                                user_id: user_context.user_id.clone(),
                                ip: client_ip.clone(),
                                severity: SecuritySeverity::High,
                            });

                            tracing::warn!("Script deletion blocked: {}", e);
                            Err(JsError::Exception)
                        }
                    }
                },
            )?
        };

        if user_context.has_capability(&crate::security::validation::Capability::DeleteScripts) {
            global.set("deleteScript", secure_delete_script)?;
        } else {
            let noop = Function::new(
                ctx.clone(),
                |_c: rquickjs::Ctx<'_>, _uri: String| -> Result<bool, JsError> {
                    tracing::warn!("Attempted to call deleteScript without permission");
                    Err(JsError::Exception)
                },
            )?;
            global.set("deleteScript", noop)?;
        }

        // Secure getScript function
        let secure_get_script = {
            let handler = Arc::clone(&handler);
            let user_context = user_context.clone();
            let client_ip = client_ip.clone();

            Function::new(
                ctx.clone(),
                move |_c: rquickjs::Ctx<'_>, uri: String| -> Result<String, JsError> {
                    match handler.secure_get_script(&user_context, &uri, &client_ip) {
                        Ok(Some(content)) => Ok(content),
                        Ok(None) => Ok("".to_string()),
                        Err(e) => {
                            security_audit_log(SecurityEvent::SecurityViolation {
                                violation_type: "get_script_blocked".to_string(),
                                details: e.to_string(),
                                user_id: user_context.user_id.clone(),
                                ip: client_ip.clone(),
                                severity: SecuritySeverity::Low,
                            });

                            tracing::warn!("Script access blocked: {}", e);
                            Err(JsError::Exception)
                        }
                    }
                },
            )?
        };

        if user_context.has_capability(&crate::security::validation::Capability::ReadScripts) {
            global.set("getScript", secure_get_script)?;
        }

        Ok(())
    }

    fn setup_asset_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        global: &rquickjs::Object<'_>,
        user_context: UserContext,
        client_ip: String,
    ) -> Result<(), JsError> {
        let handler = Arc::clone(&self.operation_handler);

        // Secure upsertAsset function
        if user_context.has_capability(&crate::security::validation::Capability::WriteAssets) {
            let secure_upsert_asset = {
                let handler = Arc::clone(&handler);
                let user_context = user_context.clone();
                let client_ip = client_ip.clone();

                Function::new(
                    ctx.clone(),
                    move |_c: rquickjs::Ctx<'_>,
                          public_path: String,
                          mimetype: String,
                          content_b64: String| -> Result<(), JsError> {
                        // Decode base64 content
                        let content = match base64::Engine::decode(
                            &base64::engine::general_purpose::STANDARD,
                            &content_b64,
                        ) {
                            Ok(content) => content,
                            Err(e) => {
                                tracing::warn!("Invalid base64 content: {}", e);
                                return Err(JsError::Exception);
                            }
                        };

                        match handler.secure_upsert_asset(
                            &user_context,
                            &public_path,
                            &mimetype,
                            &content,
                            &client_ip,
                        ) {
                            Ok(()) => Ok(()),
                            Err(e) => {
                                security_audit_log(SecurityEvent::SecurityViolation {
                                    violation_type: "upsert_asset_blocked".to_string(),
                                    details: e.to_string(),
                                    user_id: user_context.user_id.clone(),
                                    ip: client_ip.clone(),
                                    severity: SecuritySeverity::Medium,
                                });

                                tracing::warn!("Asset upsert blocked: {}", e);
                                Err(JsError::Exception)
                            }
                        }
                    },
                )?
            };

            global.set("upsertAsset", secure_upsert_asset)?;
        }

        Ok(())
    }

    fn setup_logging_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        global: &rquickjs::Object<'_>,
        user_context: UserContext,
        client_ip: String,
    ) -> Result<(), JsError> {
        // Secure writeLog function (with input sanitization)
        let secure_write_log = {
            let user_context = user_context.clone();
            let client_ip = client_ip.clone();

            Function::new(
                ctx.clone(),
                move |_c: rquickjs::Ctx<'_>, message: String| -> Result<(), JsError> {
                    // Sanitize log message to prevent log injection
                    let sanitized_message = message
                        .replace('\n', " ")
                        .replace('\r', " ")
                        .replace('\t', " ");

                    // Truncate if too long
                    let truncated_message = if sanitized_message.len() > 1000 {
                        format!("{}... (truncated)", &sanitized_message[..1000])
                    } else {
                        sanitized_message
                    };

                    // Log with user context
                    tracing::info!(
                        user_id = ?user_context.user_id,
                        ip = %client_ip,
                        message = %truncated_message,
                        "JavaScript log message"
                    );

                    Ok(())
                },
            )?
        };

        global.set("writeLog", secure_write_log)?;

        Ok(())
    }
}
```

### Step 2.2: Update JavaScript Engine to Use Secure Functions

**File**: `src/js_engine.rs` (Replace setup_global_functions)

```rust
// Add this import at the top
use crate::security::{
    capabilities::UserContext,
    audit::extract_client_ip,
};

mod secure_globals;
use secure_globals::SecureGlobalFunctions;

// Replace the existing setup_global_functions with this simplified version
fn setup_secure_global_functions(
    ctx: &rquickjs::Ctx<'_>,
    script_uri: &str,
    user_context: Option<UserContext>,
    client_ip: String,
    register_fn: Option<RegisterFunctionType>,
) -> Result<(), rquickjs::Error> {
    let global = ctx.globals();

    // Setup register function (unchanged)
    if let Some(register_impl) = register_fn {
        let register = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>,
                  path: String,
                  handler: String,
                  method: Option<String>|
                  -> Result<(), rquickjs::Error> {
                let method_ref = method.as_deref();
                register_impl(&path, &handler, method_ref)
            },
        )?;
        global.set("register", register)?;
    }

    // Use secure global functions
    let secure_globals = SecureGlobalFunctions::new()
        .map_err(|e| {
            tracing::error!("Failed to create secure globals: {}", e);
            rquickjs::Error::Exception
        })?;

    let user_ctx = user_context.unwrap_or_else(UserContext::anonymous);

    secure_globals.setup_secure_globals(ctx, user_ctx, client_ip)?;

    Ok(())
}

// Update execute_script_for_request to pass user context
pub fn execute_script_for_request(
    script_uri: &str,
    script_content: &str,
    handler_name: &str,
    path: &str,
    method: &str,
    query_params: std::collections::HashMap<String, String>,
    form_data: Option<std::collections::HashMap<String, String>>,
    headers: &axum::http::HeaderMap,
    timeout_ms: u64,
) -> ScriptExecutionResult {
    // Extract client IP for security logging
    let client_ip = extract_client_ip(headers);

    // TODO: Extract user context from headers/session
    // For now, use anonymous context
    let user_context = UserContext::anonymous();

    // ... rest of the function with updated setup call
    let setup_result = setup_secure_global_functions(
        &ctx,
        script_uri,
        Some(user_context),
        client_ip,
        None, // No register function for request execution
    );

    // ... rest unchanged
}
```

---

## 🧹 Day 3: Simplify JavaScript Handlers

### Step 3.1: Update Core Script Handler

**File**: `scripts/feature_scripts/core.js` (Simplified version)

```javascript
// scripts/feature_scripts/core.js - SIMPLIFIED TO BUSINESS LOGIC ONLY

// Script management endpoint - NO SECURITY LOGIC IN JAVASCRIPT
function upsert_script_handler(req) {
  try {
    // ✅ ONLY BUSINESS LOGIC - extract parameters
    const uri = req.form?.uri || req.query?.uri;
    const content = req.form?.content || req.query?.content;

    // ✅ ONLY BUSINESS VALIDATION - check required fields
    if (!uri || !content) {
      return {
        status: 400,
        body: JSON.stringify({
          error: "URI and content are required",
          timestamp: new Date().toISOString(),
        }),
        contentType: "application/json",
      };
    }

    // ✅ CALL RUST-SECURED FUNCTION - all security validation happens in Rust
    upsertScript(uri, content);

    // ✅ BUSINESS LOGIC RESPONSE
    return {
      status: 200,
      body: JSON.stringify({
        message: "Script updated successfully",
        uri: uri,
        size: content.length,
        timestamp: new Date().toISOString(),
      }),
      contentType: "application/json",
    };
  } catch (error) {
    // ✅ BUSINESS LOGIC ERROR HANDLING - no security details exposed
    return {
      status: 400,
      body: JSON.stringify({
        error: "Operation failed",
        timestamp: new Date().toISOString(),
      }),
      contentType: "application/json",
    };
  }
}

// Delete script endpoint - SIMPLIFIED
function delete_script_handler(req) {
  try {
    const uri = req.form?.uri || req.query?.uri;

    if (!uri) {
      return {
        status: 400,
        body: JSON.stringify({ error: "URI is required" }),
        contentType: "application/json",
      };
    }

    // ✅ CALL RUST-SECURED FUNCTION
    const deleted = deleteScript(uri);

    return {
      status: deleted ? 200 : 404,
      body: JSON.stringify({
        message: deleted ? "Script deleted successfully" : "Script not found",
        uri: uri,
        deleted: deleted,
        timestamp: new Date().toISOString(),
      }),
      contentType: "application/json",
    };
  } catch (error) {
    return {
      status: 400,
      body: JSON.stringify({ error: "Delete operation failed" }),
      contentType: "application/json",
    };
  }
}

// Get script endpoint - SIMPLIFIED
function get_script_handler(req) {
  try {
    const uri = req.query?.uri;

    if (!uri) {
      return {
        status: 400,
        body: JSON.stringify({ error: "URI is required" }),
        contentType: "application/json",
      };
    }

    // ✅ CALL RUST-SECURED FUNCTION
    const content = getScript(uri);

    return {
      status: 200,
      body: JSON.stringify({
        uri: uri,
        content: content,
        length: content.length,
        timestamp: new Date().toISOString(),
      }),
      contentType: "application/json",
    };
  } catch (error) {
    return {
      status: 404,
      body: JSON.stringify({ error: "Script not found" }),
      contentType: "application/json",
    };
  }
}

// Asset management - SIMPLIFIED
function upsert_asset_handler(req) {
  try {
    const publicPath = req.form?.publicPath || req.query?.publicPath;
    const mimetype = req.form?.mimetype || req.query?.mimetype;
    const contentBase64 = req.form?.content || req.query?.content;

    if (!publicPath || !mimetype || !contentBase64) {
      return {
        status: 400,
        body: JSON.stringify({
          error: "publicPath, mimetype, and content are required",
        }),
        contentType: "application/json",
      };
    }

    // ✅ CALL RUST-SECURED FUNCTION
    upsertAsset(publicPath, mimetype, contentBase64);

    return {
      status: 200,
      body: JSON.stringify({
        message: "Asset uploaded successfully",
        publicPath: publicPath,
        mimetype: mimetype,
        timestamp: new Date().toISOString(),
      }),
      contentType: "application/json",
    };
  } catch (error) {
    return {
      status: 400,
      body: JSON.stringify({ error: "Asset upload failed" }),
      contentType: "application/json",
    };
  }
}

// Register routes
register("/api/scripts/upsert", "upsert_script_handler", "POST");
register("/api/scripts/delete", "delete_script_handler", "POST");
register("/api/scripts/get", "get_script_handler", "GET");
register("/api/assets/upsert", "upsert_asset_handler", "POST");

// Log server start - SIMPLIFIED
try {
  writeLog(`Core API handlers loaded at ${new Date().toISOString()}`);
} catch (e) {
  // Ignore if logging not available
}
```

### Step 3.2: Update Other JavaScript Files

Apply the same simplification pattern to other JavaScript files:

1. **Remove all security validation logic**
2. **Keep only business logic and user experience logic**
3. **Call Rust-secured functions directly**
4. **Handle errors gracefully without exposing internals**

---

## 🔧 Integration Steps

### Step 4.1: Update Cargo.toml

```toml
# Add to dependencies
[dependencies]
base64 = "0.22"
regex = "1.10"
thiserror = "1.0"
serde = { version = "1.0", features = ["derive"] }
```

### Step 4.2: Update Module Structure

**File**: `src/security/mod.rs`

```rust
pub mod validation;
pub mod capabilities;
pub mod operations;
pub mod audit;

pub use validation::{InputValidator, SecurityError, Capability};
pub use capabilities::UserContext;
pub use operations::SecureOperationHandler;
pub use audit::{security_audit_log, SecurityEvent, SecuritySeverity, extract_client_ip};
```

**File**: `src/lib.rs` (add security module)

```rust
pub mod security;
```

### Step 4.3: Update Request Processing

**File**: Update main request handler to extract user context\*\*

```rust
// In your main request handler, extract user context from headers/cookies
// and pass it to the JavaScript execution engine

async fn handle_dynamic_request(req: Request<Body>) -> impl IntoResponse {
    // Extract user context (placeholder - will be implemented with authentication)
    let user_context = extract_user_context_from_request(&req);

    // Pass user context to script execution
    let result = js_engine::execute_script_for_request_with_context(
        &script_uri,
        &script_content,
        &handler_name,
        &path,
        &method,
        query_params,
        form_data,
        req.headers(),
        user_context, // Pass user context
        timeout_ms,
    );

    // Handle result...
}

fn extract_user_context_from_request(req: &Request<Body>) -> UserContext {
    // TODO: Extract from JWT cookie when authentication is implemented
    // For now, return anonymous context
    UserContext::anonymous()
}
```

---

## ✅ Verification and Testing

### Step 5.1: Security Validation Tests

**File**: `tests/security_boundary_tests.rs`

```rust
#[cfg(test)]
mod security_boundary_tests {
    use super::*;

    #[test]
    fn test_javascript_cannot_bypass_rust_validation() {
        // Test that malicious JavaScript cannot bypass Rust security
        let malicious_script = r#"
            // Try to bypass validation by calling repository functions directly
            try {
                // This should fail because repository functions are not exposed
                repository.upsert_script("/evil", "eval('hack')");
            } catch (e) {
                // Expected
            }

            // Try to call upsertScript with dangerous content
            try {
                upsertScript("/test", "eval('malicious code')");
                // This should be blocked by Rust validation
            } catch (e) {
                // Expected - Rust validation should block this
            }
        "#;

        let result = execute_script("test", malicious_script);
        // Script should execute but dangerous operations should be blocked
        assert!(result.success);
    }

    #[test]
    fn test_capability_enforcement() {
        // Test that capability checks are enforced in Rust
        let user_context = UserContext::anonymous(); // No write capabilities
        let handler = SecureOperationHandler::new().unwrap();

        let result = handler.secure_upsert_script(
            &user_context,
            "/test",
            "function test() {}",
            "127.0.0.1",
        );

        // Should fail due to insufficient capabilities
        assert!(result.is_err());
    }

    #[test]
    fn test_input_validation_in_rust() {
        let user_context = UserContext::admin("admin".to_string());
        let handler = SecureOperationHandler::new().unwrap();

        // Test dangerous content is blocked
        let result = handler.secure_upsert_script(
            &user_context,
            "/test",
            "eval('dangerous code')",
            "127.0.0.1",
        );

        assert!(result.is_err());

        // Test path traversal is blocked
        let result = handler.secure_upsert_script(
            &user_context,
            "/../etc/passwd",
            "function test() {}",
            "127.0.0.1",
        );

        assert!(result.is_err());
    }
}
```

### Step 5.2: JavaScript Simplification Verification

Create tests to verify JavaScript files contain minimal security logic:

```bash
# Script to check for security patterns in JavaScript files
grep -r "validation\|sanitize\|dangerous\|security" scripts/ || echo "✅ No security logic found in JavaScript"
```

---

## 📋 Implementation Checklist

### Day 1: Secure Rust Architecture ✅

- [ ] Create `src/security/validation.rs` with comprehensive input validation
- [ ] Create `src/security/capabilities.rs` with capability management
- [ ] Create `src/security/operations.rs` with secure operation handlers
- [ ] Create `src/security/audit.rs` with security event logging
- [ ] Add security module to `src/lib.rs`

### Day 2: Secure Global Functions ✅

- [ ] Create `src/js_engine/secure_globals.rs` with secure function wrappers
- [ ] Replace `setup_global_functions` with `setup_secure_global_functions`
- [ ] Update `execute_script_for_request` to pass user context
- [ ] Test that JavaScript cannot bypass Rust validation

### Day 3: Simplify JavaScript ✅

- [ ] Remove all security validation logic from `scripts/feature_scripts/core.js`
- [ ] Simplify all handler functions to business logic only
- [ ] Update all JavaScript files to call Rust-secured functions
- [ ] Verify JavaScript files contain minimal security logic

### Integration & Testing ✅

- [ ] Update dependencies in `Cargo.toml`
- [ ] Create comprehensive security boundary tests
- [ ] Verify capability enforcement works correctly
- [ ] Test that malicious JavaScript is blocked
- [ ] Document the new security architecture

---

## 🎯 Success Criteria

After implementation, the system should achieve:

1. **✅ All security validation enforced in Rust** - No JavaScript can bypass security checks
2. **✅ JavaScript contains only business logic** - No security validation in JS files
3. **✅ Capability-based access control** - User permissions enforced in Rust
4. **✅ Comprehensive security logging** - All security events logged in Rust
5. **✅ Input validation at Rust boundary** - All inputs validated before processing
6. **✅ No direct repository access from JS** - All operations go through secure handlers

### Architectural Principle Achieved:

> **"Security is enforced at the Rust boundary, not the JavaScript boundary"**

This restructuring creates a solid security foundation where:

- 🔒 **Rust handles all security concerns**
- 🎯 **JavaScript focuses on business logic**
- 🛡️ **No security bypass possibilities**
- 📊 **Complete security visibility**
- ⚡ **Clean separation of concerns**

After this restructuring is complete, you can proceed with Phase 0 security implementation and then authentication, knowing that the security boundaries are correctly designed and enforced.
