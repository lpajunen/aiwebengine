use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{error, info, warn};

use super::threat_detection::{ThreatAssessment, ThreatDetector, ThreatLevel};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SecurityEventType {
    AuthenticationAttempt,
    AuthenticationSuccess,
    AuthenticationFailure,
    AuthorizationFailure,
    InputValidationFailure,
    SuspiciousActivity,
    CapabilityViolation,
    SystemSecurityEvent,
    RateLimitExceeded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SecuritySeverity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityEvent {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub event_type: SecurityEventType,
    pub severity: SecuritySeverity,
    pub user_id: Option<String>,
    pub user_agent: Option<String>,
    pub ip_address: Option<String>,
    pub resource: Option<String>,
    pub action: Option<String>,
    pub details: HashMap<String, String>,
    pub error_message: Option<String>,
}

impl SecurityEvent {
    pub fn new(
        event_type: SecurityEventType,
        severity: SecuritySeverity,
        user_id: Option<String>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            event_type,
            severity,
            user_id,
            user_agent: None,
            ip_address: None,
            resource: None,
            action: None,
            details: HashMap::new(),
            error_message: None,
        }
    }

    pub fn with_request_context(
        mut self,
        user_agent: Option<String>,
        ip_address: Option<String>,
    ) -> Self {
        self.user_agent = user_agent;
        self.ip_address = ip_address;
        self
    }

    pub fn with_resource(mut self, resource: String) -> Self {
        self.resource = Some(resource);
        self
    }

    pub fn with_action(mut self, action: String) -> Self {
        self.action = Some(action);
        self
    }

    pub fn with_detail<K: ToString, V: ToString>(mut self, key: K, value: V) -> Self {
        self.details.insert(key.to_string(), value.to_string());
        self
    }

    pub fn with_error(mut self, error: String) -> Self {
        self.error_message = Some(error);
        self
    }
}

#[derive(Clone)]
pub struct SecurityAuditor {
    // In a real implementation, this might connect to a database
    // or external logging system
    threat_detector: ThreatDetector,
}

impl SecurityAuditor {
    pub fn new() -> Self {
        Self {
            threat_detector: ThreatDetector::with_default_config(),
        }
    }

    pub fn with_threat_detector(threat_detector: ThreatDetector) -> Self {
        Self { threat_detector }
    }

    /// Log a security event with enhanced threat analysis
    pub async fn log_event(&self, event: SecurityEvent) {
        // Perform threat analysis
        let threat_assessment = self.threat_detector.analyze_event(&event);

        // Log based on original severity and threat assessment
        let effective_severity =
            self.determine_effective_severity(&event.severity, &threat_assessment);

        match effective_severity {
            SecuritySeverity::Low => {
                info!(
                    event_id = %event.id,
                    event_type = ?event.event_type,
                    user_id = ?event.user_id,
                    resource = ?event.resource,
                    action = ?event.action,
                    threat_level = ?threat_assessment.threat_level,
                    confidence_score = threat_assessment.confidence_score,
                    "Security event logged"
                );
            }
            SecuritySeverity::Medium => {
                warn!(
                    event_id = %event.id,
                    event_type = ?event.event_type,
                    user_id = ?event.user_id,
                    resource = ?event.resource,
                    action = ?event.action,
                    error = ?event.error_message,
                    threat_level = ?threat_assessment.threat_level,
                    confidence_score = threat_assessment.confidence_score,
                    threat_indicators = threat_assessment.threat_indicators.len(),
                    "Security warning logged"
                );
            }
            SecuritySeverity::High | SecuritySeverity::Critical => {
                error!(
                    event_id = %event.id,
                    event_type = ?event.event_type,
                    severity = ?effective_severity,
                    user_id = ?event.user_id,
                    ip_address = ?event.ip_address,
                    user_agent = ?event.user_agent,
                    resource = ?event.resource,
                    action = ?event.action,
                    details = ?event.details,
                    error = ?event.error_message,
                    threat_level = ?threat_assessment.threat_level,
                    confidence_score = threat_assessment.confidence_score,
                    threat_indicators = ?threat_assessment.threat_indicators,
                    recommended_actions = ?threat_assessment.recommended_actions,
                    "Critical security event logged"
                );
            }
        }

        // Log threat assessment details for high-risk events
        if matches!(
            threat_assessment.threat_level,
            ThreatLevel::High | ThreatLevel::Critical
        ) {
            warn!(
                threat_assessment_id = %event.id,
                threat_level = ?threat_assessment.threat_level,
                confidence_score = threat_assessment.confidence_score,
                indicators_count = threat_assessment.threat_indicators.len(),
                recommended_actions = ?threat_assessment.recommended_actions,
                "Threat assessment completed"
            );

            // Execute recommended actions for critical threats
            if matches!(threat_assessment.threat_level, ThreatLevel::Critical) {
                self.execute_automated_response(&event, &threat_assessment)
                    .await;
            }
        }

        // TODO: In production, also store in database and/or send to SIEM
        // self.store_in_database(&event).await;
        // self.send_to_siem(&event).await;
        // self.store_threat_assessment(&threat_assessment).await;

        // For critical events, consider immediate alerting
        if matches!(event.severity, SecuritySeverity::Critical) {
            self.send_alert(&event).await;
        }
    }

    /// Log authentication attempt
    pub async fn log_auth_attempt(
        &self,
        user_id: Option<String>,
        ip_address: Option<String>,
        user_agent: Option<String>,
    ) {
        let event = SecurityEvent::new(
            SecurityEventType::AuthenticationAttempt,
            SecuritySeverity::Low,
            user_id,
        )
        .with_request_context(user_agent, ip_address)
        .with_action("login_attempt".to_string());

        self.log_event(event).await;
    }

    /// Log successful authentication
    pub async fn log_auth_success(
        &self,
        user_id: String,
        ip_address: Option<String>,
        user_agent: Option<String>,
    ) {
        let event = SecurityEvent::new(
            SecurityEventType::AuthenticationSuccess,
            SecuritySeverity::Low,
            Some(user_id),
        )
        .with_request_context(user_agent, ip_address)
        .with_action("login_success".to_string());

        self.log_event(event).await;
    }

    /// Log failed authentication
    pub async fn log_auth_failure(
        &self,
        attempted_user: Option<String>,
        reason: String,
        ip_address: Option<String>,
        user_agent: Option<String>,
    ) {
        let event = SecurityEvent::new(
            SecurityEventType::AuthenticationFailure,
            SecuritySeverity::Medium,
            attempted_user,
        )
        .with_request_context(user_agent, ip_address)
        .with_action("login_failure".to_string())
        .with_error(reason);

        self.log_event(event).await;
    }

    /// Log authorization failure
    pub async fn log_authz_failure(
        &self,
        user_id: Option<String>,
        resource: String,
        action: String,
        required_capability: String,
    ) {
        let event = SecurityEvent::new(
            SecurityEventType::AuthorizationFailure,
            SecuritySeverity::Medium,
            user_id,
        )
        .with_resource(resource)
        .with_action(action)
        .with_detail("required_capability", required_capability)
        .with_error("Insufficient capabilities".to_string());

        self.log_event(event).await;
    }

    /// Log input validation failure
    pub async fn log_validation_failure(
        &self,
        user_id: Option<String>,
        input_type: String,
        validation_error: String,
    ) {
        let event = SecurityEvent::new(
            SecurityEventType::InputValidationFailure,
            SecuritySeverity::Medium,
            user_id,
        )
        .with_action("input_validation".to_string())
        .with_detail("input_type", input_type)
        .with_error(validation_error);

        self.log_event(event).await;
    }

    /// Determine effective severity based on original severity and threat assessment
    fn determine_effective_severity(
        &self,
        original_severity: &SecuritySeverity,
        threat_assessment: &ThreatAssessment,
    ) -> SecuritySeverity {
        // Escalate severity based on threat level
        match (&original_severity, &threat_assessment.threat_level) {
            (_, ThreatLevel::Critical) => SecuritySeverity::Critical,
            (SecuritySeverity::Low, ThreatLevel::High) => SecuritySeverity::High,
            (SecuritySeverity::Medium, ThreatLevel::High) => SecuritySeverity::High,
            (SecuritySeverity::Low, ThreatLevel::Medium) => SecuritySeverity::Medium,
            _ => original_severity.clone(),
        }
    }

    /// Execute automated response actions for critical threats
    async fn execute_automated_response(
        &self,
        event: &SecurityEvent,
        assessment: &ThreatAssessment,
    ) {
        warn!(
            event_id = %event.id,
            threat_level = ?assessment.threat_level,
            "Executing automated threat response"
        );

        // Execute recommended actions
        for action in &assessment.recommended_actions {
            if action.contains("Block IP address") {
                if let Some(ip) = &event.ip_address {
                    self.request_ip_block(ip).await;
                }
            } else if action.contains("Alert security team") {
                self.send_security_alert(event, assessment).await;
            } else if action.contains("account lockdown") {
                if let Some(user_id) = &event.user_id {
                    self.request_account_lockdown(user_id).await;
                }
            }
        }
    }

    /// Request IP address blocking (placeholder for actual implementation)
    async fn request_ip_block(&self, ip_address: &str) {
        error!(
            ip_address = %ip_address,
            "AUTOMATED RESPONSE: IP block requested"
        );
        // TODO: Integrate with firewall/WAF API
        // self.firewall_api.block_ip(ip_address).await;
    }

    /// Send security alert (placeholder for actual implementation)
    async fn send_security_alert(&self, event: &SecurityEvent, assessment: &ThreatAssessment) {
        error!(
            event_id = %event.id,
            threat_level = ?assessment.threat_level,
            confidence_score = assessment.confidence_score,
            "AUTOMATED RESPONSE: Security team alert sent"
        );
        // TODO: Integrate with alerting system (email, Slack, PagerDuty, etc.)
        // self.alerting_system.send_alert(&alert).await;
    }

    /// Request account lockdown (placeholder for actual implementation)
    async fn request_account_lockdown(&self, user_id: &str) {
        error!(
            user_id = %user_id,
            "AUTOMATED RESPONSE: Account lockdown requested"
        );
        // TODO: Integrate with user management system
        // self.user_manager.lockdown_account(user_id).await;
    }

    /// Get threat detector for external access
    pub fn threat_detector(&self) -> &ThreatDetector {
        &self.threat_detector
    }

    /// Get threat statistics for monitoring
    pub fn get_threat_statistics(&self) -> super::threat_detection::ThreatStatistics {
        self.threat_detector.get_threat_statistics()
    }

    /// Cleanup old threat data
    pub fn cleanup_old_data(&self) {
        self.threat_detector.cleanup_old_data();
    }

    /// Log suspicious activity
    pub async fn log_suspicious_activity(
        &self,
        user_id: Option<String>,
        activity_type: String,
        details: HashMap<String, String>,
    ) {
        let mut event = SecurityEvent::new(
            SecurityEventType::SuspiciousActivity,
            SecuritySeverity::High,
            user_id,
        )
        .with_action(activity_type);

        for (key, value) in details {
            event = event.with_detail(key, value);
        }

        self.log_event(event).await;
    }

    /// Log capability violation
    pub async fn log_capability_violation(
        &self,
        user_id: Option<String>,
        attempted_capability: String,
        actual_capabilities: Vec<String>,
    ) {
        let event = SecurityEvent::new(
            SecurityEventType::CapabilityViolation,
            SecuritySeverity::High,
            user_id,
        )
        .with_action("capability_check".to_string())
        .with_detail("attempted_capability", attempted_capability)
        .with_detail("actual_capabilities", actual_capabilities.join(","));

        self.log_event(event).await;
    }

    /// Send alert for critical events
    async fn send_alert(&self, event: &SecurityEvent) {
        // TODO: Implement alerting mechanism
        // This could be:
        // - Email notification
        // - Slack/Teams message
        // - PagerDuty alert
        // - SMS notification

        warn!(
            event_id = %event.id,
            "CRITICAL SECURITY ALERT: {}",
            event.error_message.as_deref().unwrap_or("Unknown critical event")
        );
    }
}

impl Default for SecurityAuditor {
    fn default() -> Self {
        Self::new()
    }
}

// Convenience macros for logging common security events
#[macro_export]
macro_rules! log_security_event {
    ($auditor:expr, $event_type:expr, $severity:expr, $user_id:expr) => {
        $auditor
            .log_event(SecurityEvent::new($event_type, $severity, $user_id))
            .await
    };
}

#[macro_export]
macro_rules! log_auth_failure {
    ($auditor:expr, $user:expr, $reason:expr) => {
        $auditor
            .log_auth_failure($user, $reason.to_string(), None, None)
            .await
    };
}

#[macro_export]
macro_rules! log_authz_failure {
    ($auditor:expr, $user:expr, $resource:expr, $action:expr, $capability:expr) => {
        $auditor
            .log_authz_failure(
                $user,
                $resource.to_string(),
                $action.to_string(),
                $capability.to_string(),
            )
            .await
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_security_event_creation() {
        let event = SecurityEvent::new(
            SecurityEventType::AuthenticationFailure,
            SecuritySeverity::Medium,
            Some("user123".to_string()),
        )
        .with_resource("login".to_string())
        .with_action("password_auth".to_string())
        .with_detail("reason", "invalid_password")
        .with_error("Authentication failed".to_string());

        assert_eq!(event.event_type, SecurityEventType::AuthenticationFailure);
        assert_eq!(event.severity, SecuritySeverity::Medium);
        assert_eq!(event.user_id, Some("user123".to_string()));
        assert_eq!(event.resource, Some("login".to_string()));
        assert_eq!(event.action, Some("password_auth".to_string()));
        assert_eq!(
            event.details.get("reason"),
            Some(&"invalid_password".to_string())
        );
        assert_eq!(
            event.error_message,
            Some("Authentication failed".to_string())
        );
        assert!(!event.id.is_empty());
    }

    #[tokio::test]
    async fn test_auditor_logging() {
        let auditor = SecurityAuditor::new();

        // Test various logging methods
        auditor
            .log_auth_attempt(
                Some("user123".to_string()),
                Some("192.168.1.1".to_string()),
                None,
            )
            .await;
        auditor
            .log_auth_success("user123".to_string(), Some("192.168.1.1".to_string()), None)
            .await;
        auditor
            .log_auth_failure(
                Some("user123".to_string()),
                "Invalid password".to_string(),
                None,
                None,
            )
            .await;
        auditor
            .log_validation_failure(
                Some("user123".to_string()),
                "script_name".to_string(),
                "Contains invalid characters".to_string(),
            )
            .await;

        // No assertions needed - just testing that methods don't panic
    }
}
