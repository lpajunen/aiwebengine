pub mod audit;
pub mod capabilities;
pub mod operations;
pub mod secure_globals;
pub mod validation;

pub use audit::{SecurityAuditor, SecurityEvent, SecurityEventType, SecuritySeverity};
pub use capabilities::UserContext;
pub use operations::{OperationResult, SecureOperations, UpsertScriptRequest};
pub use secure_globals::{GlobalSecurityConfig, SecureGlobalContext};
pub use validation::{Capability, InputValidator, SecurityError};

// Re-export convenience macros
pub use crate::{log_auth_failure, log_authz_failure, log_security_event};
