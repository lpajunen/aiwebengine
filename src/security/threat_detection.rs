use chrono::{DateTime, Duration, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::warn;

use super::{SecurityEvent, SecurityEventType};

use sqlx::PgPool;

/// Threat detection and anomaly analysis system
#[derive(Clone)]
pub struct ThreatDetector {
    /// Database pool (optional for memory-only mode)
    pool: Option<PgPool>,
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
    pub fn new(pool: Option<PgPool>, config: ThreatDetectionConfig) -> Self {
        Self { pool, config }
    }

    pub fn with_default_config(pool: Option<PgPool>) -> Self {
        Self::new(pool, ThreatDetectionConfig::default())
    }

    /// Analyze a security event for threats and anomalies
    pub async fn analyze_event(&self, event: &SecurityEvent) -> ThreatAssessment {
        let mut threat_indicators = Vec::new();

        // If no database, we can only do stateless analysis
        if self.pool.is_none() {
            // Perform basic stateless analysis
            threat_indicators.extend(self.analyze_input_validation_failure(event).await);
            if event.event_type == SecurityEventType::SuspiciousActivity {
                threat_indicators.extend(self.analyze_suspicious_activity(event).await);
            }

            // Return early with limited assessment
            return self.create_assessment(threat_indicators);
        }

        match event.event_type {
            SecurityEventType::AuthenticationFailure => {
                if let Some(ip) = &event.ip_address {
                    threat_indicators.extend(self.analyze_authentication_failure(ip).await);
                }
            }
            SecurityEventType::AuthorizationFailure => {
                if let Some(user_id) = &event.user_id {
                    threat_indicators.extend(self.analyze_authorization_failure(user_id).await);
                }
            }
            SecurityEventType::InputValidationFailure => {
                threat_indicators.extend(self.analyze_input_validation_failure(event).await);
            }
            SecurityEventType::SuspiciousActivity => {
                threat_indicators.extend(self.analyze_suspicious_activity(event).await);
            }
            _ => {
                // Basic analysis for other event types
                threat_indicators.extend(self.analyze_general_activity(event).await);
            }
        }

        // Calculate overall confidence score
        self.create_assessment(threat_indicators)
    }

    /// Analyze failed authentication attempts for brute force patterns
    async fn analyze_authentication_failure(&self, ip_address: &str) -> Vec<ThreatIndicator> {
        let mut indicators = Vec::new();

        let pool = match &self.pool {
            Some(p) => p,
            None => return indicators,
        };

        let now = Utc::now();
        let cutoff = now - Duration::minutes(self.config.auth_failure_window_minutes);

        // Record failure
        let _ = sqlx::query(
            "INSERT INTO failed_auth_attempts (identifier, attempt_time, type) VALUES ($1, $2, 'authentication')"
        )
        .bind(ip_address)
        .bind(now)
        .execute(pool)
        .await;

        // Count recent failures
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM failed_auth_attempts WHERE identifier = $1 AND type = 'authentication' AND attempt_time > $2"
        )
        .bind(ip_address)
        .bind(cutoff)
        .fetch_one(pool)
        .await
        .unwrap_or(0);

        if count as usize >= self.config.max_failed_auth_attempts {
            indicators.push(ThreatIndicator {
                indicator_type: "brute_force_authentication".to_string(),
                severity: 80.0,
                description: format!(
                    "Potential brute force attack: {} failed authentication attempts from {} in {} minutes",
                    count,
                    ip_address,
                    self.config.auth_failure_window_minutes
                ),
                evidence: vec![
                    format!("IP: {}", ip_address),
                    format!("Failed attempts: {}", count),
                    format!("Time window: {} minutes", self.config.auth_failure_window_minutes),
                ],
            });

            warn!(
                ip_address = %ip_address,
                failed_attempts = count,
                "Potential brute force attack detected"
            );
        }

        indicators
    }

    /// Analyze failed authorization attempts for privilege escalation
    async fn analyze_authorization_failure(&self, user_id: &str) -> Vec<ThreatIndicator> {
        let mut indicators = Vec::new();

        let pool = match &self.pool {
            Some(p) => p,
            None => return indicators,
        };

        let now = Utc::now();
        let cutoff = now - Duration::minutes(self.config.authz_failure_window_minutes);

        // Record failure
        let _ = sqlx::query(
            "INSERT INTO failed_auth_attempts (identifier, attempt_time, type) VALUES ($1, $2, 'authorization')"
        )
        .bind(user_id)
        .bind(now)
        .execute(pool)
        .await;

        // Count recent failures
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM failed_auth_attempts WHERE identifier = $1 AND type = 'authorization' AND attempt_time > $2"
        )
        .bind(user_id)
        .bind(cutoff)
        .fetch_one(pool)
        .await
        .unwrap_or(0);

        if count as usize >= self.config.max_failed_authz_attempts {
            indicators.push(ThreatIndicator {
                indicator_type: "privilege_escalation_attempt".to_string(),
                severity: 70.0,
                description: format!(
                    "Potential privilege escalation: {} failed authorization attempts by user {} in {} minutes",
                    count,
                    user_id,
                    self.config.authz_failure_window_minutes
                ),
                evidence: vec![
                    format!("User ID: {}", user_id),
                    format!("Failed attempts: {}", count),
                    format!("Time window: {} minutes", self.config.authz_failure_window_minutes),
                ],
            });
        }

        indicators
    }

    /// Analyze input validation failures for injection attempts
    async fn analyze_input_validation_failure(
        &self,
        event: &SecurityEvent,
    ) -> Vec<ThreatIndicator> {
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
    async fn analyze_suspicious_activity(&self, event: &SecurityEvent) -> Vec<ThreatIndicator> {
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
        if let Some(user_agent) = &event.user_agent
            && (user_agent.contains("bot")
                || user_agent.contains("crawler")
                || user_agent.contains("scanner")
                || user_agent.len() < 20)
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

        indicators
    }

    /// General activity analysis
    async fn analyze_general_activity(&self, event: &SecurityEvent) -> Vec<ThreatIndicator> {
        let mut indicators = Vec::new();

        // Check for rapid sequential requests from same IP
        if let Some(ip) = &event.ip_address {
            // This would need more sophisticated tracking, but for now we can
            // detect basic patterns based on event details
            if let Some(details) = event.details.get("request_count")
                && let Ok(count) = details.parse::<i32>()
                && count > 100
            {
                indicators.push(ThreatIndicator {
                    indicator_type: "rapid_request_pattern".to_string(),
                    severity: 60.0,
                    description: format!("High request volume detected: {} requests", count),
                    evidence: vec![format!("IP: {}", ip), format!("Request count: {}", count)],
                });
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
    pub async fn get_threat_statistics(&self) -> ThreatStatistics {
        let pool = match &self.pool {
            Some(p) => p,
            None => {
                return ThreatStatistics {
                    active_threats: 0,
                    monitored_ips: 0,
                    auth_failure_count: 0,
                    authz_failure_count: 0,
                    last_updated: Utc::now(),
                };
            }
        };

        let auth_failures: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM failed_auth_attempts WHERE type = 'authentication'",
        )
        .fetch_one(pool)
        .await
        .unwrap_or(0);

        let authz_failures: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM failed_auth_attempts WHERE type = 'authorization'",
        )
        .fetch_one(pool)
        .await
        .unwrap_or(0);

        let suspicious_ips: i64 =
            sqlx::query_scalar("SELECT COUNT(DISTINCT identifier) FROM suspicious_activity")
                .fetch_one(pool)
                .await
                .unwrap_or(0);

        ThreatStatistics {
            active_threats: (auth_failures + authz_failures) as usize,
            monitored_ips: suspicious_ips as usize,
            auth_failure_count: auth_failures as usize,
            authz_failure_count: authz_failures as usize,
            last_updated: Utc::now(),
        }
    }

    /// Clean up old threat data to prevent memory leaks
    pub async fn cleanup_old_data(&self) {
        let pool = match &self.pool {
            Some(p) => p,
            None => return,
        };

        let now = Utc::now();
        let auth_cutoff = now - Duration::minutes(self.config.auth_failure_window_minutes * 2);
        let authz_cutoff = now - Duration::minutes(self.config.authz_failure_window_minutes * 2);

        let _ = sqlx::query(
            "DELETE FROM failed_auth_attempts WHERE type = 'authentication' AND attempt_time < $1",
        )
        .bind(auth_cutoff)
        .execute(pool)
        .await;

        let _ = sqlx::query(
            "DELETE FROM failed_auth_attempts WHERE type = 'authorization' AND attempt_time < $1",
        )
        .bind(authz_cutoff)
        .execute(pool)
        .await;
    }

    /// Helper to create threat assessment from indicators
    fn create_assessment(&self, threat_indicators: Vec<ThreatIndicator>) -> ThreatAssessment {
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreatStatistics {
    pub active_threats: usize,
    pub monitored_ips: usize,
    pub auth_failure_count: usize,
    pub authz_failure_count: usize,
    pub last_updated: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::SecuritySeverity;
    use tokio::sync::Mutex;

    static THREAT_TEST_LOCK: Mutex<()> = Mutex::const_new(());

    #[tokio::test]
    async fn test_threat_detector_creation() {
        let _lock = THREAT_TEST_LOCK.lock().await;
        let pool = sqlx::PgPool::connect(
            "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
        )
        .await
        .unwrap();
        sqlx::query("DELETE FROM suspicious_activity")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM failed_auth_attempts")
            .execute(&pool)
            .await
            .unwrap();

        let detector = ThreatDetector::with_default_config(Some(pool));
        let stats = detector.get_threat_statistics().await;
        assert_eq!(stats.active_threats, 0);
    }

    #[tokio::test]
    async fn test_brute_force_detection() {
        let _lock = THREAT_TEST_LOCK.lock().await;
        let pool = sqlx::PgPool::connect_lazy(
            "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
        )
        .unwrap();
        let detector = ThreatDetector::with_default_config(Some(pool));

        // Simulate multiple failed auth attempts
        for _ in 0..6 {
            let event = SecurityEvent::new(
                SecurityEventType::AuthenticationFailure,
                SecuritySeverity::Medium,
                None,
            )
            .with_request_context(None, Some("192.168.1.100".to_string()));

            let assessment = detector.analyze_event(&event).await;

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

    #[tokio::test]
    async fn test_sql_injection_detection() {
        let pool = sqlx::PgPool::connect_lazy(
            "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
        )
        .unwrap();
        let detector = ThreatDetector::with_default_config(Some(pool));

        let event = SecurityEvent::new(
            SecurityEventType::InputValidationFailure,
            SecuritySeverity::High,
            Some("user123".to_string()),
        )
        .with_error("SQL injection attempt: UNION SELECT * FROM users".to_string());

        let assessment = detector.analyze_event(&event).await;

        assert!(
            assessment
                .threat_indicators
                .iter()
                .any(|i| i.indicator_type == "sql_injection_attempt")
        );
        assert!(assessment.confidence_score > 50.0);
    }

    #[tokio::test]
    async fn test_threat_level_calculation() {
        let pool = sqlx::PgPool::connect_lazy(
            "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
        )
        .unwrap();
        let detector = ThreatDetector::with_default_config(Some(pool));

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
