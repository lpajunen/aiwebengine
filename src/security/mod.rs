pub mod audit;
pub mod capabilities;
pub mod operations;
pub mod validation;

pub use audit::{SecurityAuditor, SecurityEvent, SecurityEventType, SecuritySeverity};
pub use capabilities::UserContext;
pub use operations::{OperationResult, SecureOperations, UpsertScriptRequest};
pub use validation::{Capability, InputValidator, SecurityError};

// Re-export convenience macros
pub use crate::{log_auth_failure, log_authz_failure, log_security_event};
