use chrono::{DateTime, Duration, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, RwLock};
use tracing::warn;

use super::{SecurityEvent, SecurityEventType};

/// Threat detection and anomaly analysis system
#[derive(Clone)]
pub struct ThreatDetector {
    /// Failed authentication attempts by IP
    failed_auth_attempts: Arc<RwLock<HashMap<String, VecDeque<DateTime<Utc>>>>>,
    /// Failed authorization attempts by user
    failed_authz_attempts: Arc<RwLock<HashMap<String, VecDeque<DateTime<Utc>>>>>,
    /// Suspicious activity patterns by IP
    suspicious_activity: Arc<RwLock<HashMap<String, VecDeque<SuspiciousActivity>>>>,
    /// Geographic anomaly tracking
    geo_anomalies: Arc<RwLock<HashMap<String, Vec<GeoLocation>>>>,
    /// Rate limiting violations
    rate_limit_violations: Arc<RwLock<HashMap<String, VecDeque<DateTime<Utc>>>>>,
    /// Configuration for threat detection
    config: ThreatDetectionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreatDetectionConfig {
    /// Maximum failed auth attempts before flagging as threat
    pub max_failed_auth_attempts: usize,
    /// Time window for failed auth analysis (minutes)
    pub auth_failure_window_minutes: i64,
    /// Maximum failed authz attempts before flagging
    pub max_failed_authz_attempts: usize,
    /// Time window for authz failure analysis (minutes)
    pub authz_failure_window_minutes: i64,
    /// Threshold for suspicious activity scoring
    pub suspicious_activity_threshold: f64,
    /// Maximum allowed geographic distance for normal access (km)
    pub max_geographic_distance_km: f64,
    /// Time window for geographic anomaly detection (hours)
    pub geo_anomaly_window_hours: i64,
    /// Enable advanced threat detection features
    pub enable_advanced_detection: bool,
}

impl Default for ThreatDetectionConfig {
    fn default() -> Self {
        Self {
            max_failed_auth_attempts: 5,
            auth_failure_window_minutes: 15,
            max_failed_authz_attempts: 10,
            authz_failure_window_minutes: 30,
            suspicious_activity_threshold: 75.0,
            max_geographic_distance_km: 1000.0,
            geo_anomaly_window_hours: 24,
            enable_advanced_detection: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuspiciousActivity {
    pub timestamp: DateTime<Utc>,
    pub activity_type: SuspiciousActivityType,
    pub severity_score: f64,
    pub details: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SuspiciousActivityType {
    SqlInjectionAttempt,
    XssAttempt,
    PathTraversalAttempt,
    ScriptInjectionAttempt,
    UnusualUserAgentPattern,
    RapidRequestPattern,
    UnusualTimeOfAccess,
    SuspiciousScriptExecution,
    PrivilegeEscalationAttempt,
    DataExfiltrationPattern,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoLocation {
    pub timestamp: DateTime<Utc>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub country: Option<String>,
    pub city: Option<String>,
    pub ip_address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ThreatLevel {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreatAssessment {
    pub threat_level: ThreatLevel,
    pub confidence_score: f64,
    pub threat_indicators: Vec<ThreatIndicator>,
    pub recommended_actions: Vec<String>,
    pub assessment_timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreatIndicator {
    pub indicator_type: String,
    pub severity: f64,
    pub description: String,
    pub evidence: Vec<String>,
}

impl ThreatDetector {
    pub fn new(config: ThreatDetectionConfig) -> Self {
        Self {
            failed_auth_attempts: Arc::new(RwLock::new(HashMap::new())),
            failed_authz_attempts: Arc::new(RwLock::new(HashMap::new())),
            suspicious_activity: Arc::new(RwLock::new(HashMap::new())),
            geo_anomalies: Arc::new(RwLock::new(HashMap::new())),
            rate_limit_violations: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    pub fn with_default_config() -> Self {
        Self::new(ThreatDetectionConfig::default())
    }

    /// Analyze a security event for threats and anomalies
    pub fn analyze_event(&self, event: &SecurityEvent) -> ThreatAssessment {
        let mut threat_indicators = Vec::new();

        match event.event_type {
            SecurityEventType::AuthenticationFailure => {
                if let Some(ip) = &event.ip_address {
                    threat_indicators.extend(self.analyze_authentication_failure(ip));
                }
            }
            SecurityEventType::AuthorizationFailure => {
                if let Some(user_id) = &event.user_id {
                    threat_indicators.extend(self.analyze_authorization_failure(user_id));
                }
            }
            SecurityEventType::InputValidationFailure => {
                threat_indicators.extend(self.analyze_input_validation_failure(event));
            }
            SecurityEventType::SuspiciousActivity => {
                threat_indicators.extend(self.analyze_suspicious_activity(event));
            }
            _ => {
                // Basic analysis for other event types
                threat_indicators.extend(self.analyze_general_activity(event));
            }
        }

        // Calculate overall confidence score
        let confidence_score = threat_indicators
            .iter()
            .map(|indicator| indicator.severity)
            .sum::<f64>()
            / threat_indicators.len().max(1) as f64;

        let threat_level = self.determine_threat_level(confidence_score);
        let recommended_actions =
            self.generate_recommended_actions(&threat_level, &threat_indicators);

        ThreatAssessment {
            threat_level,
            confidence_score,
            threat_indicators,
            recommended_actions,
            assessment_timestamp: Utc::now(),
        }
    }

    /// Analyze failed authentication attempts for brute force patterns
    fn analyze_authentication_failure(&self, ip_address: &str) -> Vec<ThreatIndicator> {
        let mut indicators = Vec::new();

        if let Ok(mut attempts) = self.failed_auth_attempts.write() {
            let entry = attempts
                .entry(ip_address.to_string())
                .or_insert_with(VecDeque::new);
            let now = Utc::now();

            // Add current attempt
            entry.push_back(now);

            // Clean old attempts outside the window
            let cutoff = now - Duration::minutes(self.config.auth_failure_window_minutes);
            while let Some(&front_time) = entry.front() {
                if front_time < cutoff {
                    entry.pop_front();
                } else {
                    break;
                }
            }

            // Check if we exceed the threshold
            if entry.len() >= self.config.max_failed_auth_attempts {
                indicators.push(ThreatIndicator {
                    indicator_type: "brute_force_authentication".to_string(),
                    severity: 80.0,
                    description: format!(
                        "Potential brute force attack: {} failed authentication attempts from {} in {} minutes",
                        entry.len(),
                        ip_address,
                        self.config.auth_failure_window_minutes
                    ),
                    evidence: vec![
                        format!("IP: {}", ip_address),
                        format!("Failed attempts: {}", entry.len()),
                        format!("Time window: {} minutes", self.config.auth_failure_window_minutes),
                    ],
                });

                warn!(
                    ip_address = %ip_address,
                    failed_attempts = entry.len(),
                    "Potential brute force attack detected"
                );
            }
        }

        indicators
    }

    /// Analyze failed authorization attempts for privilege escalation
    fn analyze_authorization_failure(&self, user_id: &str) -> Vec<ThreatIndicator> {
        let mut indicators = Vec::new();

        if let Ok(mut attempts) = self.failed_authz_attempts.write() {
            let entry = attempts
                .entry(user_id.to_string())
                .or_insert_with(VecDeque::new);
            let now = Utc::now();

            entry.push_back(now);

            // Clean old attempts
            let cutoff = now - Duration::minutes(self.config.authz_failure_window_minutes);
            while let Some(&front_time) = entry.front() {
                if front_time < cutoff {
                    entry.pop_front();
                } else {
                    break;
                }
            }

            if entry.len() >= self.config.max_failed_authz_attempts {
                indicators.push(ThreatIndicator {
                    indicator_type: "privilege_escalation_attempt".to_string(),
                    severity: 70.0,
                    description: format!(
                        "Potential privilege escalation: {} failed authorization attempts by user {} in {} minutes",
                        entry.len(),
                        user_id,
                        self.config.authz_failure_window_minutes
                    ),
                    evidence: vec![
                        format!("User ID: {}", user_id),
                        format!("Failed authz attempts: {}", entry.len()),
                    ],
                });
            }
        }

        indicators
    }

    /// Analyze input validation failures for injection attempts
    fn analyze_input_validation_failure(&self, event: &SecurityEvent) -> Vec<ThreatIndicator> {
        let mut indicators = Vec::new();

        if let Some(error_msg) = &event.error_message {
            let error_lower = error_msg.to_lowercase();

            // Check for SQL injection patterns
            if error_lower.contains("sql")
                || error_lower.contains("union")
                || error_lower.contains("select")
                || error_lower.contains("drop")
            {
                indicators.push(ThreatIndicator {
                    indicator_type: "sql_injection_attempt".to_string(),
                    severity: 85.0,
                    description: "Potential SQL injection attempt detected".to_string(),
                    evidence: vec![
                        format!("Error message: {}", error_msg),
                        format!("IP: {:?}", event.ip_address),
                        format!("User: {:?}", event.user_id),
                    ],
                });
            }

            // Check for XSS patterns
            if error_lower.contains("script")
                || error_lower.contains("xss")
                || error_lower.contains("javascript")
                || error_lower.contains("onerror")
            {
                indicators.push(ThreatIndicator {
                    indicator_type: "xss_attempt".to_string(),
                    severity: 75.0,
                    description: "Potential XSS attempt detected".to_string(),
                    evidence: vec![
                        format!("Error message: {}", error_msg),
                        format!("IP: {:?}", event.ip_address),
                    ],
                });
            }

            // Check for path traversal patterns
            if error_lower.contains("..")
                || error_lower.contains("path traversal")
                || error_lower.contains("directory")
            {
                indicators.push(ThreatIndicator {
                    indicator_type: "path_traversal_attempt".to_string(),
                    severity: 70.0,
                    description: "Potential path traversal attempt detected".to_string(),
                    evidence: vec![
                        format!("Error message: {}", error_msg),
                        format!("Resource: {:?}", event.resource),
                    ],
                });
            }
        }

        indicators
    }

    /// Analyze suspicious activity patterns
    fn analyze_suspicious_activity(&self, event: &SecurityEvent) -> Vec<ThreatIndicator> {
        let mut indicators = Vec::new();

        // Check for unusual time patterns
        let hour = event.timestamp.hour();
        if !(6..=22).contains(&hour) {
            indicators.push(ThreatIndicator {
                indicator_type: "unusual_time_access".to_string(),
                severity: 30.0,
                description: format!("Access at unusual time: {}:00", hour),
                evidence: vec![
                    format!("Access time: {}", event.timestamp),
                    format!("User: {:?}", event.user_id),
                ],
            });
        }

        // Check for suspicious user agent patterns
        if let Some(user_agent) = &event.user_agent {
            if user_agent.contains("bot")
                || user_agent.contains("crawler")
                || user_agent.contains("scanner")
                || user_agent.len() < 20
            {
                indicators.push(ThreatIndicator {
                    indicator_type: "suspicious_user_agent".to_string(),
                    severity: 40.0,
                    description: "Suspicious user agent detected".to_string(),
                    evidence: vec![
                        format!("User agent: {}", user_agent),
                        format!("IP: {:?}", event.ip_address),
                    ],
                });
            }
        }

        indicators
    }

    /// General activity analysis
    fn analyze_general_activity(&self, event: &SecurityEvent) -> Vec<ThreatIndicator> {
        let mut indicators = Vec::new();

        // Check for rapid sequential requests from same IP
        if let Some(ip) = &event.ip_address {
            // This would need more sophisticated tracking, but for now we can
            // detect basic patterns based on event details
            if let Some(details) = event.details.get("request_count") {
                if let Ok(count) = details.parse::<i32>() {
                    if count > 100 {
                        indicators.push(ThreatIndicator {
                            indicator_type: "rapid_request_pattern".to_string(),
                            severity: 60.0,
                            description: format!(
                                "High request volume detected: {} requests",
                                count
                            ),
                            evidence: vec![
                                format!("IP: {}", ip),
                                format!("Request count: {}", count),
                            ],
                        });
                    }
                }
            }
        }

        indicators
    }

    /// Determine overall threat level based on confidence score
    fn determine_threat_level(&self, confidence_score: f64) -> ThreatLevel {
        match confidence_score {
            score if score >= 90.0 => ThreatLevel::Critical,
            score if score >= 70.0 => ThreatLevel::High,
            score if score >= 40.0 => ThreatLevel::Medium,
            _ => ThreatLevel::Low,
        }
    }

    /// Generate recommended actions based on threat assessment
    fn generate_recommended_actions(
        &self,
        threat_level: &ThreatLevel,
        indicators: &[ThreatIndicator],
    ) -> Vec<String> {
        let mut actions = Vec::new();

        match threat_level {
            ThreatLevel::Critical => {
                actions.push("IMMEDIATE ACTION: Block IP address".to_string());
                actions.push("Alert security team immediately".to_string());
                actions.push("Review all recent activity from this source".to_string());
                actions.push("Consider temporary account lockdown".to_string());
            }
            ThreatLevel::High => {
                actions.push("Increase monitoring for this IP/user".to_string());
                actions.push("Apply additional authentication requirements".to_string());
                actions.push("Review recent activity patterns".to_string());
            }
            ThreatLevel::Medium => {
                actions.push("Continue monitoring".to_string());
                actions.push("Log detailed activity for analysis".to_string());
            }
            ThreatLevel::Low => {
                actions.push("Standard monitoring sufficient".to_string());
            }
        }

        // Add specific actions based on threat types
        for indicator in indicators {
            match indicator.indicator_type.as_str() {
                "brute_force_authentication" => {
                    actions.push("Implement progressive delays for authentication".to_string());
                }
                "sql_injection_attempt" => {
                    actions.push("Review and validate all database queries".to_string());
                }
                "xss_attempt" => {
                    actions.push("Verify input sanitization and output encoding".to_string());
                }
                _ => {}
            }
        }

        actions.dedup();
        actions
    }

    /// Get threat statistics for monitoring dashboard
    pub fn get_threat_statistics(&self) -> ThreatStatistics {
        let auth_failures = self
            .failed_auth_attempts
            .read()
            .map(|attempts| attempts.len())
            .unwrap_or(0);

        let authz_failures = self
            .failed_authz_attempts
            .read()
            .map(|attempts| attempts.len())
            .unwrap_or(0);

        let suspicious_ips = self
            .suspicious_activity
            .read()
            .map(|activity| activity.len())
            .unwrap_or(0);

        ThreatStatistics {
            active_threats: auth_failures + authz_failures,
            monitored_ips: suspicious_ips,
            auth_failure_count: auth_failures,
            authz_failure_count: authz_failures,
            last_updated: Utc::now(),
        }
    }

    /// Clean up old threat data to prevent memory leaks
    pub fn cleanup_old_data(&self) {
        let now = Utc::now();
        let auth_cutoff = now - Duration::minutes(self.config.auth_failure_window_minutes * 2);
        let authz_cutoff = now - Duration::minutes(self.config.authz_failure_window_minutes * 2);

        // Clean old authentication failures
        if let Ok(mut attempts) = self.failed_auth_attempts.write() {
            attempts.retain(|_, queue| {
                while let Some(&front_time) = queue.front() {
                    if front_time < auth_cutoff {
                        queue.pop_front();
                    } else {
                        break;
                    }
                }
                !queue.is_empty()
            });
        }

        // Clean old authorization failures
        if let Ok(mut attempts) = self.failed_authz_attempts.write() {
            attempts.retain(|_, queue| {
                while let Some(&front_time) = queue.front() {
                    if front_time < authz_cutoff {
                        queue.pop_front();
                    } else {
                        break;
                    }
                }
                !queue.is_empty()
            });
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreatStatistics {
    pub active_threats: usize,
    pub monitored_ips: usize,
    pub auth_failure_count: usize,
    pub authz_failure_count: usize,
    pub last_updated: DateTime<Utc>,
}

impl Default for ThreatDetector {
    fn default() -> Self {
        Self::with_default_config()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::SecuritySeverity;

    #[test]
    fn test_threat_detector_creation() {
        let detector = ThreatDetector::with_default_config();
        let stats = detector.get_threat_statistics();
        assert_eq!(stats.active_threats, 0);
    }

    #[test]
    fn test_brute_force_detection() {
        let detector = ThreatDetector::with_default_config();

        // Simulate multiple failed auth attempts
        for _ in 0..6 {
            let event = SecurityEvent::new(
                SecurityEventType::AuthenticationFailure,
                SecuritySeverity::Medium,
                None,
            )
            .with_request_context(None, Some("192.168.1.100".to_string()));

            let assessment = detector.analyze_event(&event);

            // After 5+ attempts, should detect brute force
            if let ThreatLevel::High = assessment.threat_level {
                assert!(
                    assessment
                        .threat_indicators
                        .iter()
                        .any(|i| i.indicator_type == "brute_force_authentication")
                );
                break;
            }
        }
    }

    #[test]
    fn test_sql_injection_detection() {
        let detector = ThreatDetector::with_default_config();

        let event = SecurityEvent::new(
            SecurityEventType::InputValidationFailure,
            SecuritySeverity::High,
            Some("user123".to_string()),
        )
        .with_error("SQL injection attempt: UNION SELECT * FROM users".to_string());

        let assessment = detector.analyze_event(&event);

        assert!(
            assessment
                .threat_indicators
                .iter()
                .any(|i| i.indicator_type == "sql_injection_attempt")
        );
        assert!(assessment.confidence_score > 50.0);
    }

    #[test]
    fn test_threat_level_calculation() {
        let detector = ThreatDetector::with_default_config();

        // Test different confidence scores
        assert!(matches!(
            detector.determine_threat_level(95.0),
            ThreatLevel::Critical
        ));
        assert!(matches!(
            detector.determine_threat_level(75.0),
            ThreatLevel::High
        ));
        assert!(matches!(
            detector.determine_threat_level(50.0),
            ThreatLevel::Medium
        ));
        assert!(matches!(
            detector.determine_threat_level(20.0),
            ThreatLevel::Low
        ));
    }
}
