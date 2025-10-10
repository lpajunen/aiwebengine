// Content Security Policy (CSP) Module
// Provides dynamic CSP generation, nonce-based script execution, and runtime policy updates

use base64::{Engine as _, engine::general_purpose};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// CSP directive types
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CspDirective {
    DefaultSrc,
    ScriptSrc,
    StyleSrc,
    ImgSrc,
    ConnectSrc,
    FontSrc,
    ObjectSrc,
    MediaSrc,
    FrameSrc,
    ChildSrc,
    FormAction,
    FrameAncestors,
    BaseUri,
    ReportUri,
    ReportTo,
}

impl CspDirective {
    fn as_str(&self) -> &'static str {
        match self {
            CspDirective::DefaultSrc => "default-src",
            CspDirective::ScriptSrc => "script-src",
            CspDirective::StyleSrc => "style-src",
            CspDirective::ImgSrc => "img-src",
            CspDirective::ConnectSrc => "connect-src",
            CspDirective::FontSrc => "font-src",
            CspDirective::ObjectSrc => "object-src",
            CspDirective::MediaSrc => "media-src",
            CspDirective::FrameSrc => "frame-src",
            CspDirective::ChildSrc => "child-src",
            CspDirective::FormAction => "form-action",
            CspDirective::FrameAncestors => "frame-ancestors",
            CspDirective::BaseUri => "base-uri",
            CspDirective::ReportUri => "report-uri",
            CspDirective::ReportTo => "report-to",
        }
    }
}

/// CSP source types
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CspSource {
    None,
    Self_,
    UnsafeInline,
    UnsafeEval,
    StrictDynamic,
    UnsafeHashes,
    Nonce(String),
    Hash(String, String), // algorithm, hash
    Uri(String),
    Scheme(String),
    Host(String),
}

impl CspSource {
    fn as_str(&self) -> String {
        match self {
            CspSource::None => "'none'".to_string(),
            CspSource::Self_ => "'self'".to_string(),
            CspSource::UnsafeInline => "'unsafe-inline'".to_string(),
            CspSource::UnsafeEval => "'unsafe-eval'".to_string(),
            CspSource::StrictDynamic => "'strict-dynamic'".to_string(),
            CspSource::UnsafeHashes => "'unsafe-hashes'".to_string(),
            CspSource::Nonce(nonce) => format!("'nonce-{}'", nonce),
            CspSource::Hash(algo, hash) => format!("'{}-{}'", algo, hash),
            CspSource::Uri(uri) => uri.clone(),
            CspSource::Scheme(scheme) => format!("{}:", scheme),
            CspSource::Host(host) => host.clone(),
        }
    }
}

/// CSP policy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CspPolicy {
    pub directives: HashMap<CspDirective, Vec<CspSource>>,
    pub report_only: bool,
    pub nonce: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl CspPolicy {
    pub fn new() -> Self {
        Self {
            directives: HashMap::new(),
            report_only: false,
            nonce: None,
            created_at: Utc::now(),
            expires_at: None,
        }
    }

    /// Generate a secure nonce for this policy
    pub fn generate_nonce(&mut self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(format!(
            "{}{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0),
            rand::random::<u64>()
        ));
        let nonce = general_purpose::STANDARD.encode(hasher.finalize());
        self.nonce = Some(nonce.clone());
        nonce
    }

    /// Add a directive with sources
    pub fn add_directive(&mut self, directive: CspDirective, sources: Vec<CspSource>) {
        self.directives.insert(directive, sources);
    }

    /// Add a source to an existing directive
    pub fn add_source(&mut self, directive: CspDirective, source: CspSource) {
        self.directives.entry(directive).or_default().push(source);
    }

    /// Generate the CSP header value
    pub fn to_header_value(&self) -> String {
        let mut parts = Vec::new();

        for (directive, sources) in &self.directives {
            let source_strs: Vec<String> = sources.iter().map(|s| s.as_str()).collect();
            if !source_strs.is_empty() {
                parts.push(format!("{} {}", directive.as_str(), source_strs.join(" ")));
            }
        }

        parts.join("; ")
    }

    /// Check if the policy is expired
    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            Utc::now() > expires_at
        } else {
            false
        }
    }
}

impl Default for CspPolicy {
    fn default() -> Self {
        let mut policy = Self::new();

        // Secure default policy
        policy.add_directive(CspDirective::DefaultSrc, vec![CspSource::Self_]);
        policy.add_directive(
            CspDirective::ScriptSrc,
            vec![CspSource::Self_, CspSource::StrictDynamic],
        );
        policy.add_directive(
            CspDirective::StyleSrc,
            vec![CspSource::Self_, CspSource::UnsafeInline],
        );
        policy.add_directive(CspDirective::ImgSrc, vec![CspSource::Self_]);
        policy.add_directive(CspDirective::ConnectSrc, vec![CspSource::Self_]);
        policy.add_directive(CspDirective::FontSrc, vec![CspSource::Self_]);
        policy.add_directive(CspDirective::ObjectSrc, vec![CspSource::None]);
        policy.add_directive(CspDirective::BaseUri, vec![CspSource::Self_]);
        policy.add_directive(CspDirective::FrameAncestors, vec![CspSource::None]);

        policy
    }
}

/// CSP violation report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CspViolationReport {
    pub document_uri: String,
    pub referrer: String,
    pub blocked_uri: String,
    pub violated_directive: String,
    pub effective_directive: String,
    pub original_policy: String,
    pub disposition: String,
    pub script_sample: Option<String>,
    pub status_code: u16,
    pub timestamp: DateTime<Utc>,
}

/// CSP Manager for dynamic policy generation and management
pub struct CspManager {
    /// Active policies by session/request ID
    active_policies: Arc<RwLock<HashMap<String, CspPolicy>>>,
    /// Default policy template
    default_policy: CspPolicy,
    /// Policy cache TTL
    policy_ttl: Duration,
    /// Violation reports
    violation_reports: Arc<RwLock<Vec<CspViolationReport>>>,
}

impl CspManager {
    pub fn new() -> Self {
        Self {
            active_policies: Arc::new(RwLock::new(HashMap::new())),
            default_policy: CspPolicy::default(),
            policy_ttl: Duration::hours(1),
            violation_reports: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Generate a new CSP policy for a request
    pub async fn generate_policy(&self, request_id: &str) -> CspPolicy {
        let mut policy = self.default_policy.clone();
        let nonce = policy.generate_nonce();

        // Add nonce to script-src if not already present
        if let Some(script_sources) = policy.directives.get_mut(&CspDirective::ScriptSrc) {
            script_sources.push(CspSource::Nonce(nonce));
        }

        // Set expiration
        policy.expires_at = Some(Utc::now() + self.policy_ttl);

        // Store the policy
        self.active_policies
            .write()
            .await
            .insert(request_id.to_string(), policy.clone());

        info!("Generated CSP policy for request: {}", request_id);
        debug!("CSP Policy: {}", policy.to_header_value());

        policy
    }

    /// Get an existing policy by request ID
    pub async fn get_policy(&self, request_id: &str) -> Option<CspPolicy> {
        let policies = self.active_policies.read().await;
        policies.get(request_id).cloned()
    }

    /// Update an existing policy
    pub async fn update_policy(
        &self,
        request_id: &str,
        mut policy: CspPolicy,
    ) -> Result<(), String> {
        if policy.is_expired() {
            return Err("Cannot update expired policy".to_string());
        }

        policy.created_at = Utc::now();
        self.active_policies
            .write()
            .await
            .insert(request_id.to_string(), policy);

        info!("Updated CSP policy for request: {}", request_id);
        Ok(())
    }

    /// Clean up expired policies
    pub async fn cleanup_expired_policies(&self) {
        let mut policies = self.active_policies.write().await;
        let initial_count = policies.len();

        policies.retain(|_, policy| !policy.is_expired());

        let removed_count = initial_count - policies.len();
        if removed_count > 0 {
            info!("Cleaned up {} expired CSP policies", removed_count);
        }
    }

    /// Record a CSP violation
    pub async fn record_violation(&self, report: CspViolationReport) {
        warn!(
            "CSP Violation: {} blocked {} in directive {}",
            report.document_uri, report.blocked_uri, report.violated_directive
        );

        self.violation_reports.write().await.push(report);
    }

    /// Get violation reports
    pub async fn get_violation_reports(&self) -> Vec<CspViolationReport> {
        self.violation_reports.read().await.clone()
    }

    /// Generate script hash for inline scripts
    pub fn generate_script_hash(
        &self,
        script_content: &str,
        algorithm: &str,
    ) -> Result<String, String> {
        match algorithm {
            "sha256" => {
                let mut hasher = Sha256::new();
                hasher.update(script_content.as_bytes());
                Ok(general_purpose::STANDARD.encode(hasher.finalize()))
            }
            _ => Err(format!("Unsupported hash algorithm: {}", algorithm)),
        }
    }

    /// Create a policy for JavaScript execution
    pub async fn create_js_execution_policy(
        &self,
        request_id: &str,
        script_content: &str,
    ) -> CspPolicy {
        let mut policy = self.default_policy.clone();
        let nonce = policy.generate_nonce();

        // Generate hash for the script
        if let Ok(hash) = self.generate_script_hash(script_content, "sha256") {
            policy.add_source(
                CspDirective::ScriptSrc,
                CspSource::Hash("sha256".to_string(), hash),
            );
        }

        // Add nonce for dynamic scripts
        policy.add_source(CspDirective::ScriptSrc, CspSource::Nonce(nonce));

        // Allow strict-dynamic for modern browsers
        policy.add_source(CspDirective::ScriptSrc, CspSource::StrictDynamic);

        policy.expires_at = Some(Utc::now() + self.policy_ttl);

        self.active_policies
            .write()
            .await
            .insert(request_id.to_string(), policy.clone());

        info!(
            "Created JavaScript execution CSP policy for request: {}",
            request_id
        );

        policy
    }
}

impl Default for CspManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_csp_policy_creation() {
        let mut policy = CspPolicy::new();
        policy.add_directive(CspDirective::ScriptSrc, vec![CspSource::Self_]);

        assert!(policy.directives.contains_key(&CspDirective::ScriptSrc));
        assert_eq!(
            policy.directives[&CspDirective::ScriptSrc],
            vec![CspSource::Self_]
        );
    }

    #[test]
    fn test_csp_policy_header_generation() {
        let mut policy = CspPolicy::new();
        policy.add_directive(
            CspDirective::ScriptSrc,
            vec![CspSource::Self_, CspSource::UnsafeInline],
        );
        policy.add_directive(CspDirective::StyleSrc, vec![CspSource::Self_]);

        let header = policy.to_header_value();
        assert!(header.contains("script-src 'self' 'unsafe-inline'"));
        assert!(header.contains("style-src 'self'"));
    }

    #[test]
    fn test_nonce_generation() {
        let mut policy = CspPolicy::new();
        let nonce = policy.generate_nonce();

        assert!(policy.nonce.is_some());
        assert_eq!(policy.nonce.unwrap(), nonce);
        assert!(!nonce.is_empty());
    }

    #[test]
    fn test_script_hash_generation() {
        let manager = CspManager::new();
        let script = "console.log('Hello, World!');";

        let hash = manager.generate_script_hash(script, "sha256").unwrap();
        assert!(!hash.is_empty());

        // Same script should generate same hash
        let hash2 = manager.generate_script_hash(script, "sha256").unwrap();
        assert_eq!(hash, hash2);
    }

    #[tokio::test]
    async fn test_csp_manager_policy_generation() {
        let manager = CspManager::new();
        let policy = manager.generate_policy("test-request-123").await;

        assert!(policy.nonce.is_some());
        assert!(policy.directives.contains_key(&CspDirective::ScriptSrc));

        // Should be able to retrieve the policy
        let retrieved = manager.get_policy("test-request-123").await;
        assert!(retrieved.is_some());
    }

    #[tokio::test]
    async fn test_csp_violation_recording() {
        let manager = CspManager::new();

        let violation = CspViolationReport {
            document_uri: "https://example.com/page".to_string(),
            referrer: "https://example.com/".to_string(),
            blocked_uri: "https://evil.com/script.js".to_string(),
            violated_directive: "script-src 'self'".to_string(),
            effective_directive: "script-src".to_string(),
            original_policy: "script-src 'self'".to_string(),
            disposition: "enforce".to_string(),
            script_sample: None,
            status_code: 200,
            timestamp: Utc::now(),
        };

        manager.record_violation(violation.clone()).await;

        let reports = manager.get_violation_reports().await;
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].blocked_uri, violation.blocked_uri);
    }

    #[tokio::test]
    async fn test_js_execution_policy() {
        let manager = CspManager::new();
        let script = "console.log('test');";

        let policy = manager
            .create_js_execution_policy("js-test-123", script)
            .await;

        assert!(policy.nonce.is_some());
        assert!(policy.directives.contains_key(&CspDirective::ScriptSrc));

        let script_sources = &policy.directives[&CspDirective::ScriptSrc];
        assert!(
            script_sources
                .iter()
                .any(|s| matches!(s, CspSource::Hash(_, _)))
        );
        assert!(
            script_sources
                .iter()
                .any(|s| matches!(s, CspSource::Nonce(_)))
        );
        assert!(
            script_sources
                .iter()
                .any(|s| matches!(s, CspSource::StrictDynamic))
        );
    }
}
