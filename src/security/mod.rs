pub mod audit;
pub mod capabilities;
pub mod csp;
pub mod csrf;
pub mod encryption;
pub mod operations;
pub mod rate_limiting;
pub mod secure_globals;
pub mod session;
pub mod threat_detection;
pub mod validation;

pub use audit::{SecurityAuditor, SecurityEvent, SecurityEventType, SecuritySeverity};
pub use capabilities::UserContext;
pub use csp::{CspDirective, CspManager, CspPolicy, CspSource, CspViolationReport};
pub use csrf::{CsrfProtection, CsrfToken, OAuthStateManager};
pub use encryption::{
    DataEncryption, EncryptedData, EncryptionError, FieldEncryptor, SecureString,
};
pub use operations::{OperationResult, SecureOperations, UpsertScriptRequest};
pub use rate_limiting::{RateLimitConfig, RateLimitKey, RateLimitResult, RateLimiter, TokenBucket};
pub use secure_globals::{GlobalSecurityConfig, SecureGlobalContext};
pub use session::{
    CreateSessionParams, SecureSessionManager, SessionData, SessionError, SessionFingerprint,
    SessionToken,
};
pub use threat_detection::{ThreatAssessment, ThreatDetectionConfig, ThreatDetector, ThreatLevel};
pub use validation::{Capability, InputValidator, SecurityError};

// Re-export convenience macros
pub use crate::{log_auth_failure, log_authz_failure, log_security_event};
