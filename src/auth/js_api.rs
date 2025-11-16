/// JavaScript Authentication API
///
/// Exposes authentication context and functions to JavaScript runtime via rquickjs.
/// Provides secure access to user information and authentication status within JS handlers.
use rquickjs::{Ctx, Error as JsError, Function, Null, Object, Result as JsResult};
use std::sync::Arc;
use tracing::{debug, warn};

use crate::auth::AuthManager;
use crate::security::UserContext;

/// Authentication information exposed to JavaScript
#[derive(Debug, Clone)]
pub struct JsAuthContext {
    /// User ID if authenticated
    pub user_id: Option<String>,

    /// User email if available
    pub email: Option<String>,

    /// User display name if available
    pub name: Option<String>,

    /// OAuth provider used (google, microsoft, apple)
    pub provider: Option<String>,

    /// Whether user is authenticated
    pub is_authenticated: bool,

    /// Whether user has administrator privileges
    pub is_admin: bool,

    /// Whether user has editor privileges
    pub is_editor: bool,
}

impl JsAuthContext {
    /// Create anonymous (unauthenticated) context
    pub fn anonymous() -> Self {
        Self {
            user_id: None,
            email: None,
            name: None,
            provider: None,
            is_authenticated: false,
            is_admin: false,
            is_editor: false,
        }
    }

    /// Create authenticated context
    pub fn authenticated(
        user_id: String,
        email: Option<String>,
        name: Option<String>,
        provider: String,
        is_admin: bool,
        is_editor: bool,
    ) -> Self {
        Self {
            user_id: Some(user_id),
            email,
            name,
            provider: Some(provider),
            is_authenticated: true,
            is_admin,
            is_editor,
        }
    }

    /// Convert to UserContext for security checks
    pub fn to_user_context(&self) -> UserContext {
        if self.is_authenticated {
            if let Some(user_id) = &self.user_id {
                UserContext::authenticated(user_id.clone())
            } else {
                UserContext::anonymous()
            }
        } else {
            UserContext::anonymous()
        }
    }
}

/// JavaScript Authentication API
///
/// Provides functions and properties for accessing authentication in JavaScript:
/// - `auth.isAuthenticated` - Boolean indicating authentication status
/// - `auth.userId` - User ID string or null
/// - `auth.userEmail` - User email string or null
/// - `auth.userName` - User display name or null
/// - `auth.provider` - OAuth provider name or null
/// - `auth.currentUser()` - Get complete user object or null
/// - `auth.requireAuth()` - Throw error if not authenticated
pub struct AuthJsApi {
    #[allow(dead_code)]
    auth_context: JsAuthContext,
}

impl AuthJsApi {
    /// Create new JavaScript auth API with given context
    pub fn new(auth_context: JsAuthContext) -> Self {
        Self { auth_context }
    }

    /// Create authentication object for JavaScript context
    ///
    /// Creates an `auth` object with properties and methods for accessing
    /// authentication information within JavaScript handlers.
    /// This object should be attached to the request object as `req.auth`.
    pub fn create_auth_object<'js>(
        ctx: &Ctx<'js>,
        auth_context: JsAuthContext,
    ) -> JsResult<Object<'js>> {
        debug!(
            "Creating auth object, authenticated: {}",
            auth_context.is_authenticated
        );

        // Create auth object
        let auth_obj = Object::new(ctx.clone())?;

        // Set authentication status properties
        auth_obj.set("isAuthenticated", auth_context.is_authenticated)?;
        auth_obj.set("isAdmin", auth_context.is_admin)?;
        auth_obj.set("isEditor", auth_context.is_editor)?;

        // Set user information properties (null if not available)
        if let Some(user_id) = &auth_context.user_id {
            auth_obj.set("userId", user_id.clone())?;
        } else {
            auth_obj.set("userId", Null)?;
        }

        if let Some(email) = &auth_context.email {
            auth_obj.set("userEmail", email.clone())?;
        } else {
            auth_obj.set("userEmail", Null)?;
        }

        if let Some(name) = &auth_context.name {
            auth_obj.set("userName", name.clone())?;
        } else {
            auth_obj.set("userName", Null)?;
        }

        if let Some(provider) = &auth_context.provider {
            auth_obj.set("provider", provider.clone())?;
        } else {
            auth_obj.set("provider", Null)?;
        }

        // Add currentUser() method - returns object with user info or null
        let current_user_ctx = auth_context.clone();
        let current_user_fn =
            Function::new(ctx.clone(), move |_ctx: Ctx<'_>| -> JsResult<String> {
                if current_user_ctx.is_authenticated {
                    // Return JSON string representing user object
                    let user_json = serde_json::json!({
                        "id": current_user_ctx.user_id,
                        "email": current_user_ctx.email,
                        "name": current_user_ctx.name,
                        "provider": current_user_ctx.provider,
                        "isAuthenticated": true,
                    });
                    Ok(user_json.to_string())
                } else {
                    Ok("null".to_string())
                }
            })?;

        // Set the implementation function first
        auth_obj.set("__currentUserImpl", current_user_fn)?;

        // Add requireAuth() method - throws if not authenticated
        let require_auth_ctx = auth_context.clone();
        let require_auth_fn =
            Function::new(ctx.clone(), move |_ctx: Ctx<'_>| -> JsResult<String> {
                if require_auth_ctx.is_authenticated {
                    // Return JSON string representing user object
                    let user_json = serde_json::json!({
                        "id": require_auth_ctx.user_id,
                        "email": require_auth_ctx.email,
                        "name": require_auth_ctx.name,
                        "provider": require_auth_ctx.provider,
                        "isAuthenticated": true,
                    });
                    Ok(user_json.to_string())
                } else {
                    Err(JsError::Unknown)
                }
            })?;

        // Set the implementation function
        auth_obj.set("__requireAuthImpl", require_auth_fn)?;

        // Now wrap in functions that parse JSON
        // We need to create a temporary global to run the eval, then remove it
        ctx.globals().set("__tempAuth", auth_obj.clone())?;
        ctx.eval::<(), _>(
            r#"
            __tempAuth.currentUser = function() {
                const json = this.__currentUserImpl();
                return json === "null" ? null : JSON.parse(json);
            };
            __tempAuth.requireAuth = function() {
                try {
                    const json = this.__requireAuthImpl();
                    return JSON.parse(json);
                } catch (e) {
                    throw new Error('Authentication required. Please login to access this resource.');
                }
            };
            "#
        )?;

        // Get the modified object back and remove the temp global
        let auth_obj_with_methods: Object = ctx.globals().get("__tempAuth")?;
        ctx.globals().remove("__tempAuth")?;

        debug!("Auth object created successfully");
        Ok(auth_obj_with_methods)
    }
}

/// Extract authentication context from request extensions
///
/// This is called from the JS engine when executing a handler to get the
/// authentication context from the request middleware.
pub fn extract_auth_from_request(
    session_token: Option<&str>,
    auth_manager: Option<&Arc<AuthManager>>,
) -> JsAuthContext {
    // If no auth manager or session token, return anonymous
    let _auth_manager = match auth_manager {
        Some(mgr) => mgr,
        None => {
            debug!("No auth manager available, using anonymous context");
            return JsAuthContext::anonymous();
        }
    };

    let _session_token = match session_token {
        Some(token) => token,
        None => {
            debug!("No session token, using anonymous context");
            return JsAuthContext::anonymous();
        }
    };

    // Validate session and extract user info
    // Note: This is a synchronous wrapper - actual implementation would need async
    // For now, we'll just return anonymous as a placeholder
    warn!("Session validation not yet implemented in JS API, using anonymous context");
    JsAuthContext::anonymous()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rquickjs::{Context, Runtime};

    #[test]
    fn test_js_auth_context_creation() {
        let anon = JsAuthContext::anonymous();
        assert!(!anon.is_authenticated);
        assert!(!anon.is_admin);
        assert!(anon.user_id.is_none());

        let auth = JsAuthContext::authenticated(
            "user123".to_string(),
            Some("user@example.com".to_string()),
            Some("Test User".to_string()),
            "google".to_string(),
            false,
            false,
        );
        assert!(auth.is_authenticated);
        assert!(!auth.is_admin);
        assert!(!auth.is_editor);
        assert_eq!(auth.user_id, Some("user123".to_string()));
        assert_eq!(auth.email, Some("user@example.com".to_string()));
    }

    #[test]
    fn test_to_user_context() {
        let auth = JsAuthContext::authenticated(
            "user123".to_string(),
            None,
            None,
            "google".to_string(),
            false,
            false,
        );

        let user_ctx = auth.to_user_context();
        assert!(user_ctx.is_authenticated);
        assert_eq!(user_ctx.user_id, Some("user123".to_string()));
    }

    #[test]
    fn test_create_auth_object_anonymous() {
        let rt = Runtime::new().unwrap();
        let ctx = Context::full(&rt).unwrap();

        ctx.with(|ctx| {
            let auth_context = JsAuthContext::anonymous();
            let auth_obj = AuthJsApi::create_auth_object(&ctx, auth_context).unwrap();

            // Create a req object and attach auth
            let req = Object::new(ctx.clone()).unwrap();
            req.set("auth", auth_obj).unwrap();
            ctx.globals().set("req", req).unwrap();

            // Test that req.auth exists
            let result: bool = ctx.eval("typeof req.auth !== 'undefined'").unwrap();
            assert!(result);

            // Test isAuthenticated is false
            let is_authed: bool = ctx.eval("req.auth.isAuthenticated").unwrap();
            assert!(!is_authed);

            // Test isAdmin is false
            let is_admin: bool = ctx.eval("req.auth.isAdmin").unwrap();
            assert!(!is_admin);

            // Test isEditor is false
            let is_editor: bool = ctx.eval("req.auth.isEditor").unwrap();
            assert!(!is_editor);

            // Test userId is null
            let user_id_is_null: bool = ctx.eval("req.auth.userId === null").unwrap();
            assert!(user_id_is_null);
        });
    }

    #[test]
    fn test_create_auth_object_authenticated() {
        let rt = Runtime::new().unwrap();
        let ctx = Context::full(&rt).unwrap();

        ctx.with(|ctx| {
            let auth_context = JsAuthContext::authenticated(
                "user123".to_string(),
                Some("test@example.com".to_string()),
                Some("Test User".to_string()),
                "google".to_string(),
                true,  // is_admin
                false, // is_editor
            );
            let auth_obj = AuthJsApi::create_auth_object(&ctx, auth_context).unwrap();

            // Create a req object and attach auth
            let req = Object::new(ctx.clone()).unwrap();
            req.set("auth", auth_obj).unwrap();
            ctx.globals().set("req", req).unwrap();

            // Test isAuthenticated is true
            let is_authed: bool = ctx.eval("req.auth.isAuthenticated").unwrap();
            assert!(is_authed);

            // Test isAdmin is true
            let is_admin: bool = ctx.eval("req.auth.isAdmin").unwrap();
            assert!(is_admin);

            // Test isEditor is false
            let is_editor: bool = ctx.eval("req.auth.isEditor").unwrap();
            assert!(!is_editor);

            // Test userId
            let user_id: String = ctx.eval("req.auth.userId").unwrap();
            assert_eq!(user_id, "user123");

            // Test userEmail
            let email: String = ctx.eval("req.auth.userEmail").unwrap();
            assert_eq!(email, "test@example.com");

            // Test provider
            let provider: String = ctx.eval("req.auth.provider").unwrap();
            assert_eq!(provider, "google");
        });
    }

    #[test]
    fn test_current_user_function() {
        let rt = Runtime::new().unwrap();
        let ctx = Context::full(&rt).unwrap();

        ctx.with(|ctx| {
            let auth_context = JsAuthContext::authenticated(
                "user123".to_string(),
                Some("test@example.com".to_string()),
                Some("Test User".to_string()),
                "google".to_string(),
                false,
                false,
            );
            let auth_obj = AuthJsApi::create_auth_object(&ctx, auth_context).unwrap();

            // Create a req object and attach auth
            let req = Object::new(ctx.clone()).unwrap();
            req.set("auth", auth_obj).unwrap();
            ctx.globals().set("req", req).unwrap();

            // Test currentUser() returns object
            let has_user: bool = ctx.eval("req.auth.currentUser() !== null").unwrap();
            assert!(has_user);

            // Test user properties
            let user_id: String = ctx.eval("req.auth.currentUser().id").unwrap();
            assert_eq!(user_id, "user123");
        });
    }

    #[test]
    fn test_current_user_anonymous() {
        let rt = Runtime::new().unwrap();
        let ctx = Context::full(&rt).unwrap();

        ctx.with(|ctx| {
            let auth_context = JsAuthContext::anonymous();
            let auth_obj = AuthJsApi::create_auth_object(&ctx, auth_context).unwrap();

            // Create a req object and attach auth
            let req = Object::new(ctx.clone()).unwrap();
            req.set("auth", auth_obj).unwrap();
            ctx.globals().set("req", req).unwrap();

            // Test currentUser() returns null for anonymous
            let is_null: bool = ctx.eval("req.auth.currentUser() === null").unwrap();
            assert!(is_null);
        });
    }

    #[test]
    fn test_require_auth_throws_when_anonymous() {
        let rt = Runtime::new().unwrap();
        let ctx = Context::full(&rt).unwrap();

        ctx.with(|ctx| {
            let auth_context = JsAuthContext::anonymous();
            let auth_obj = AuthJsApi::create_auth_object(&ctx, auth_context).unwrap();

            // Create a req object and attach auth
            let req = Object::new(ctx.clone()).unwrap();
            req.set("auth", auth_obj).unwrap();
            ctx.globals().set("req", req).unwrap();

            // Test requireAuth() throws error
            let result: Result<bool, JsError> = ctx.eval(
                r#"
                try {
                    req.auth.requireAuth();
                    false; // Should not reach here
                } catch (e) {
                    if (e.message.includes("Authentication required")) {
                        true;
                    } else {
                        false;
                    }
                }
                "#,
            );

            match result {
                Ok(threw_correct_error) => assert!(threw_correct_error),
                Err(_) => panic!("Expected script to handle error"),
            }
        });
    }

    #[test]
    fn test_require_auth_succeeds_when_authenticated() {
        let rt = Runtime::new().unwrap();
        let ctx = Context::full(&rt).unwrap();

        ctx.with(|ctx| {
            let auth_context = JsAuthContext::authenticated(
                "user123".to_string(),
                Some("test@example.com".to_string()),
                None,
                "google".to_string(),
                false,
                false,
            );
            let auth_obj = AuthJsApi::create_auth_object(&ctx, auth_context).unwrap();

            // Create a req object and attach auth
            let req = Object::new(ctx.clone()).unwrap();
            req.set("auth", auth_obj).unwrap();
            ctx.globals().set("req", req).unwrap();

            // Test requireAuth() returns user
            let user_id: String = ctx.eval("req.auth.requireAuth().id").unwrap();
            assert_eq!(user_id, "user123");
        });
    }

    #[test]
    fn test_is_editor_property() {
        let rt = Runtime::new().unwrap();
        let ctx = Context::full(&rt).unwrap();

        ctx.with(|ctx| {
            // Test editor user
            let auth_context = JsAuthContext::authenticated(
                "editor123".to_string(),
                Some("editor@example.com".to_string()),
                Some("Editor User".to_string()),
                "google".to_string(),
                false, // not admin
                true,  // is editor
            );
            let auth_obj = AuthJsApi::create_auth_object(&ctx, auth_context).unwrap();

            // Create a req object and attach auth
            let req = Object::new(ctx.clone()).unwrap();
            req.set("auth", auth_obj).unwrap();
            ctx.globals().set("req", req).unwrap();

            // Test isEditor is true
            let is_editor: bool = ctx.eval("req.auth.isEditor").unwrap();
            assert!(is_editor);

            // Test isAdmin is false
            let is_admin: bool = ctx.eval("req.auth.isAdmin").unwrap();
            assert!(!is_admin);

            // Test isAuthenticated is true
            let is_authed: bool = ctx.eval("req.auth.isAuthenticated").unwrap();
            assert!(is_authed);
        });
    }

    #[test]
    fn test_admin_and_editor_combined() {
        let rt = Runtime::new().unwrap();
        let ctx = Context::full(&rt).unwrap();

        ctx.with(|ctx| {
            // Test user who is both admin and editor
            let auth_context = JsAuthContext::authenticated(
                "superuser".to_string(),
                Some("super@example.com".to_string()),
                Some("Super User".to_string()),
                "google".to_string(),
                true, // is admin
                true, // is editor
            );
            let auth_obj = AuthJsApi::create_auth_object(&ctx, auth_context).unwrap();

            // Create a req object and attach auth
            let req = Object::new(ctx.clone()).unwrap();
            req.set("auth", auth_obj).unwrap();
            ctx.globals().set("req", req).unwrap();

            // Test both flags are true
            let is_admin: bool = ctx.eval("req.auth.isAdmin").unwrap();
            assert!(is_admin);

            let is_editor: bool = ctx.eval("req.auth.isEditor").unwrap();
            assert!(is_editor);
        });
    }
}
