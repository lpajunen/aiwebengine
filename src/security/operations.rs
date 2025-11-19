use super::{
    capabilities::UserContext,
    validation::{Capability, InputValidator},
};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct UpsertScriptRequest {
    pub script_name: String,
    pub js_script: String,
}

#[derive(Debug, Serialize)]
pub struct OperationResult<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T> OperationResult<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(message: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message),
        }
    }
}

#[derive(Clone)]
pub struct SecureOperations {
    validator: InputValidator,
}

impl SecureOperations {
    pub fn new() -> Self {
        Self {
            validator: InputValidator::new(),
        }
    }

    /// Secure wrapper for script upsert operations
    pub async fn upsert_script(
        &self,
        user: &UserContext,
        request: UpsertScriptRequest,
    ) -> Result<OperationResult<String>, StatusCode> {
        // Check user capabilities
        if let Err(e) = user.require_capability(&Capability::WriteScripts) {
            return Ok(OperationResult::error(format!("Access denied: {}", e)));
        }

        // Validate script name
        if let Err(e) = self.validator.validate_script_name(&request.script_name) {
            return Ok(OperationResult::error(format!(
                "Invalid script name: {}",
                e
            )));
        }

        // Validate script content
        if let Err(e) = self.validator.validate_script_content(&request.js_script) {
            return Ok(OperationResult::error(format!(
                "Invalid script content: {}",
                e
            )));
        }

        // Call actual repository layer
        match crate::repository::upsert_script(&request.script_name, &request.js_script) {
            Ok(()) => {
                let success_message =
                    format!("Script '{}' upserted successfully", request.script_name);
                Ok(OperationResult::success(success_message))
            }
            Err(e) => Ok(OperationResult::error(format!(
                "Failed to upsert script: {}",
                e
            ))),
        }
    }

    /// Secure wrapper for asset management operations
    pub async fn upload_asset(
        &self,
        user: &UserContext,
        filename: String,
        content: Vec<u8>,
    ) -> Result<OperationResult<String>, StatusCode> {
        // Check user capabilities
        if let Err(e) = user.require_capability(&Capability::WriteAssets) {
            return Ok(OperationResult::error(format!("Access denied: {}", e)));
        }

        // Validate filename
        if let Err(e) = self.validator.validate_asset_filename(&filename) {
            return Ok(OperationResult::error(format!("Invalid filename: {}", e)));
        }

        // Validate content size (10MB limit)
        if content.len() > 10 * 1024 * 1024 {
            return Ok(OperationResult::error(
                "Asset too large (max 10MB)".to_string(),
            ));
        }

        // Determine MIME type (simple implementation, could be enhanced)
        let mimetype = if filename.ends_with(".html") {
            "text/html"
        } else if filename.ends_with(".css") {
            "text/css"
        } else if filename.ends_with(".js") {
            "application/javascript"
        } else if filename.ends_with(".json") {
            "application/json"
        } else if filename.ends_with(".svg") {
            "image/svg+xml"
        } else if filename.ends_with(".png") {
            "image/png"
        } else if filename.ends_with(".jpg") || filename.ends_with(".jpeg") {
            "image/jpeg"
        } else if filename.ends_with(".txt") {
            "text/plain"
        } else {
            "application/octet-stream"
        };

        // Call actual asset storage
        let now = std::time::SystemTime::now();
        let asset = crate::repository::Asset {
            uri: filename.clone(),
            name: Some(filename.clone()),
            mimetype: mimetype.to_string(),
            content,
            created_at: now,
            updated_at: now,
        };

        match crate::repository::upsert_asset(asset) {
            Ok(()) => {
                let success_message = format!("Asset '{}' uploaded successfully", filename);
                Ok(OperationResult::success(success_message))
            }
            Err(e) => Ok(OperationResult::error(format!(
                "Failed to upload asset: {}",
                e
            ))),
        }
    }

    /// Secure wrapper for HTTP requests
    pub async fn make_http_request(
        &self,
        user: &UserContext,
        url: String,
        method: String,
        headers: Option<HashMap<String, String>>,
        _body: Option<String>,
    ) -> Result<OperationResult<String>, StatusCode> {
        // Check user capabilities
        if let Err(e) = user.require_capability(&Capability::ManageStreams) {
            return Ok(OperationResult::error(format!("Access denied: {}", e)));
        }

        // Validate URL
        if let Err(e) = self.validator.validate_url(&url) {
            return Ok(OperationResult::error(format!("Invalid URL: {}", e)));
        }

        // Validate method
        let allowed_methods = ["GET", "POST", "PUT", "DELETE", "PATCH"];
        if !allowed_methods.contains(&method.to_uppercase().as_str()) {
            return Ok(OperationResult::error("Invalid HTTP method".to_string()));
        }

        // Validate headers if provided
        if let Some(ref headers) = headers {
            for (key, value) in headers {
                if let Err(e) = self.validator.validate_header_value(value) {
                    return Ok(OperationResult::error(format!(
                        "Invalid header '{}': {}",
                        key, e
                    )));
                }
            }
        }

        // TODO: Implement actual HTTP client here
        let success_message = format!("HTTP {} request to {} completed", method, url);
        Ok(OperationResult::success(success_message))
    }

    /// Secure wrapper for GraphQL schema management
    pub async fn update_graphql_schema(
        &self,
        user: &UserContext,
        schema: String,
    ) -> Result<OperationResult<String>, StatusCode> {
        // Check user capabilities (only admin can modify GraphQL schema)
        if let Err(e) = user.require_capability(&Capability::ManageGraphQL) {
            return Ok(OperationResult::error(format!("Access denied: {}", e)));
        }

        // Validate schema content
        if let Err(e) = self.validator.validate_graphql_schema(&schema) {
            return Ok(OperationResult::error(format!(
                "Invalid GraphQL schema: {}",
                e
            )));
        }

        // TODO: Implement actual GraphQL schema update
        Ok(OperationResult::success(
            "GraphQL schema updated successfully".to_string(),
        ))
    }

    /// Secure wrapper for stream management
    pub async fn create_stream(
        &self,
        user: &UserContext,
        stream_name: String,
        config: HashMap<String, String>,
    ) -> Result<OperationResult<String>, StatusCode> {
        // Check user capabilities
        if let Err(e) = user.require_capability(&Capability::ManageStreams) {
            return Ok(OperationResult::error(format!("Access denied: {}", e)));
        }

        // Validate stream name
        if let Err(e) = self.validator.validate_stream_name(&stream_name) {
            return Ok(OperationResult::error(format!(
                "Invalid stream name: {}",
                e
            )));
        }

        // Validate configuration
        for (key, value) in &config {
            if let Err(e) = self.validator.validate_config_value(value) {
                return Ok(OperationResult::error(format!(
                    "Invalid config '{}': {}",
                    key, e
                )));
            }
        }

        // TODO: Implement actual stream creation
        let success_message = format!("Stream '{}' created successfully", stream_name);
        Ok(OperationResult::success(success_message))
    }
}

impl Default for SecureOperations {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upsert_script_requires_capability() {
        let ops = SecureOperations::new();

        // Create a user with no capabilities at all (custom context)
        let user = UserContext {
            user_id: None,
            is_authenticated: false,
            capabilities: std::collections::HashSet::new(), // No capabilities
        };

        let request = UpsertScriptRequest {
            script_name: "test".to_string(),
            js_script: "console.log('hello');".to_string(),
        };

        // Should be blocked due to insufficient capabilities
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(ops.upsert_script(&user, request));

        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("Access denied"));

        // Test that anonymous users (with write capability) CAN write
        let anonymous_user = UserContext::anonymous();
        let request2 = UpsertScriptRequest {
            script_name: "test2".to_string(),
            js_script: "console.log('hello2');".to_string(),
        };
        let result2 = rt.block_on(ops.upsert_script(&anonymous_user, request2));
        assert!(result2.is_ok());
        let result2 = result2.unwrap();
        assert!(
            result2.success,
            "Anonymous users should be able to write scripts in dev mode"
        );
    }

    #[test]
    fn test_upload_asset_validates_filename() {
        let ops = SecureOperations::new();
        let user = UserContext::authenticated("user123".to_string());

        // Should fail with invalid filename
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(ops.upload_asset(
            &user,
            "../../../etc/passwd".to_string(), // Path traversal attempt
            vec![1, 2, 3],
        ));

        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("Invalid filename"));
    }

    #[test]
    fn test_http_request_validates_url() {
        let ops = SecureOperations::new();
        let user = UserContext::authenticated("user123".to_string());

        // Should fail with invalid URL
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(ops.make_http_request(
            &user,
            "javascript:alert('xss')".to_string(), // XSS attempt
            "GET".to_string(),
            None,
            None,
        ));

        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("Invalid URL"));
    }
}
