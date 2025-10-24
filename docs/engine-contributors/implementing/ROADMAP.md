# aiwebengine Development Roadmap

**Last Updated:** October 24, 2025  
**Current Version:** 0.9.0  
**Target Version:** 1.0.0 (Authentication + Stability)

This document provides a prioritized view of all development work needed for aiwebengine. It consolidates features, improvements, and technical debt into a single actionable roadmap.

---

## 📊 Current Status

### Version 0.9.0 Status

- **Core Functionality:** ✅ Operational
- **JavaScript Engine:** ✅ Working with QuickJS
- **GraphQL Support:** ✅ Queries, mutations, subscriptions
- **Security Framework:** 🚧 Implemented but not fully integrated
- **Authentication:** ❌ Not implemented
- **Database Layer:** ❌ Not implemented
- **Production Readiness:** ⚠️ Not recommended (see Critical Prerequisites)

### Next Milestone: v1.0.0 (Target: Q1 2026)

**Theme:** Production-Ready Foundation with Authentication

**Key Goals:**

- Zero panics in production code
- Authentication & user management working
- > 80% test coverage
- Security framework fully operational
- Production deployment validated

---

## 🔥 Critical Prerequisites (Before v1.0)

These **MUST** be completed before authentication or any other major feature work. They are **BLOCKING** for v1.0 release.

### 1. Error Handling & Stability 🔴

**Status:** 🚧 In Progress  
**Owner:** Unassigned  
**Target:** Week 1-2

**Problems:**

- 20+ `unwrap()` calls in production paths create panic risks
- Incomplete mutex poisoning recovery
- No JavaScript execution timeouts
- 1 failing test: `test_register_web_stream_invalid_path`

**Tasks:**

- [ ] Remove all `unwrap()`/`expect()` from `src/lib.rs` (~15 instances)
- [ ] Remove all `unwrap()`/`expect()` from `src/js_engine.rs` (~6 instances)
- [ ] Fix failing test: `test_register_web_stream_invalid_path`
- [ ] Implement JavaScript execution timeouts
- [ ] Add circuit breaker pattern for mutex recovery
- [ ] Add graceful degradation for component failures

**Success Metrics:**

- Zero `unwrap()` calls in production code (grep returns nothing)
- 100% test pass rate (126/126 passing)
- All error paths return proper `Result<T, E>` types

**Guide:** [improvements/error-handling.md](./improvements/error-handling.md)

---

### 2. Security Framework Integration 🔴

**Status:** 🚧 In Progress  
**Owner:** Unassigned  
**Target:** Week 2-3

**Problems:**

- Security modules exist but aren't enforced in execution paths
- `setup_global_functions()` legacy code still present (unused)
- 18 TODO comments in security modules indicating incomplete work
- SecureOperations not connected to actual repository/storage

**Tasks:**

- [ ] Remove legacy `setup_global_functions()` from `src/js_engine.rs:92`
- [ ] Connect `SecureOperations` to repository layer (5 TODOs in operations.rs)
- [ ] Implement async handlers in `secure_globals.rs` (8 TODOs)
- [ ] Connect audit logging to all security operations (5 TODOs in audit.rs)
- [ ] Integrate `SecureSessionManager` before auth work
- [ ] Add integration tests proving security enforcement works

**Success Metrics:**

- All 18 security TODOs resolved or explicitly deferred with documentation
- Security operations enforced on every script execution
- UserContext validated on every request
- Audit events logged for all security-sensitive operations

**Guide:** [improvements/security-hardening.md](./improvements/security-hardening.md)

---

### 3. Testing Infrastructure 🔴

**Status:** 📋 Planned  
**Owner:** Unassigned  
**Target:** Week 3

**Problems:**

- Current: 125/126 tests passing (99.2%) - need 100%
- No security integration tests
- Missing tests for `graphql.rs` module
- No property-based testing for critical algorithms
- Code coverage unknown (no reports generated)

**Tasks:**

- [ ] Fix failing test: `test_register_web_stream_invalid_path`
- [ ] Create `tests/security_integration.rs` with 10+ comprehensive tests
- [ ] Add missing unit tests for `graphql.rs` module
- [ ] Add tests for error handling paths
- [ ] Set up coverage reporting with `cargo llvm-cov`
- [ ] Achieve >80% line coverage (>90% for new code)
- [ ] Add property-based tests for input validation

**Success Metrics:**

- 100% test pass rate
- > 80% line coverage overall
- > 90% coverage for security-critical modules
- All security enforcement validated by tests

**Guide:** [improvements/testing-strategy.md](./improvements/testing-strategy.md)

---

### 4. Configuration & Secrets Management 🔴

**Status:** 📋 Planned  
**Owner:** Unassigned  
**Target:** Week 4

**Problems:**

- No configuration structure for authentication
- No secrets management (needed for JWT keys, OAuth secrets)
- Environment-specific security settings not defined
- No key rotation mechanism

**Tasks:**

- [ ] Add `AuthConfig` struct to `src/config.rs`
- [ ] Create `.env.example` with all required variables
- [ ] Implement secrets validation (minimum length, presence)
- [ ] Add environment-specific auth configs (dev/staging/prod)
- [ ] Document secrets management in `docs/engine-administrators/`
- [ ] Add configuration tests

**Success Metrics:**

- Secrets loaded from environment variables only
- Validation prevents weak/missing secrets
- Configuration tested in all environments
- Documentation complete for administrators

**Guide:** [improvements/configuration-management.md](./improvements/configuration-management.md)

---

## 🟠 High Priority (Required for v1.0)

These are production-critical features required for v1.0 release but can only start after Critical Prerequisites are complete.

### 5. Authentication System 🟠

**Status:** 📋 Fully Planned  
**Owner:** Unassigned  
**Prerequisites:** Items 1-4 above must be complete  
**Effort:** 6-8 weeks  
**Target:** Weeks 5-12

**What's Needed:**

- OAuth2/OIDC integration (Google, Microsoft, Apple)
- Session management with encrypted storage
- JWT token generation and validation
- User repository (✅ partially implemented)
- Authentication middleware
- JavaScript API for user context

**Current State:**

- ✅ User repository implemented (`src/user_repository.rs`)
- ✅ Authentication plan documented (AUTH_TODO.md)
- ✅ Security prerequisites designed
- ❌ OAuth providers not implemented
- ❌ Session management not implemented
- ❌ Middleware not implemented

**Tasks:**

- [ ] Implement secure session storage with encryption (Phase 0.5)
- [ ] Implement CSRF protection framework (Phase 0.5)
- [ ] Add data encryption layer (Phase 0.5)
- [ ] Build OAuth2 provider implementations (Phase 3)
- [ ] Create authentication middleware (Phase 4)
- [ ] Implement authentication routes (Phase 5)
- [ ] Add JavaScript integration APIs (Phase 6)
- [ ] Comprehensive testing and security validation (Phase 7)

**Success Metrics:**

- Users can authenticate with Google, Microsoft, Apple
- Sessions persist correctly and securely
- JavaScript handlers can access user context
- All security requirements met (see AUTH_TODO.md)

**Guide:** [features/authentication.md](./features/authentication.md)

---

### 6. Production Configuration Management 🟠

**Status:** 📋 Planned  
**Owner:** Unassigned  
**Effort:** 1-2 weeks  
**Target:** Week 13-14

**What's Needed:**

- Environment-specific configuration profiles
- Configuration validation with detailed errors
- Hot-reloading for non-critical settings
- Configuration schema documentation

**Guide:** [improvements/configuration-management.md](./improvements/configuration-management.md)

---

### 7. Performance & Scalability 🟠

**Status:** 💭 Needs Planning  
**Owner:** Unassigned  
**Effort:** 2-3 weeks  
**Target:** Week 15-17

**What's Needed:**

- Script compilation and caching
- Connection pooling (when database added)
- Request/response caching
- Memory usage optimization
- Multi-threaded JavaScript execution

**Guide:** [improvements/performance.md](./improvements/performance.md)

---

## 🟡 Medium Priority (Important for v1.1)

These enhance the platform but aren't blocking for initial production release.

### 8. Database Integration 🟡

**Status:** 💭 Needs Planning  
**Owner:** Unassigned  
**Effort:** 3-4 weeks  
**Target:** v1.1

**What's Needed:**

- PostgreSQL connection and query support
- Query builder with type safety
- Connection pooling
- Migration system
- Database configuration management

**Current State:**

- ❌ No database layer implemented
- Session storage currently in-memory only
- Would enable: persistent sessions, user data, application data

**Guide:** [features/database-integration.md](./features/database-integration.md)

---

### 9. HTTP Feature Enhancements 🟡

**Status:** 📋 Planned  
**Owner:** Unassigned  
**Effort:** 2-3 weeks  
**Target:** v1.1

**Features Needed:**

#### CORS Support

- Configurable cross-origin resource sharing
- Preflight request handling
- Per-route CORS configuration

#### File Upload Handling

- Multipart form data parsing
- File storage with configurable backends
- File validation (size, type, content)

#### Response Compression

- Gzip compression middleware
- Brotli compression support
- Content-type based rules

**Guide:** [features/http-enhancements.md](./features/http-enhancements.md)

---

### 10. Monitoring & Observability 🟡

**Status:** 📋 Planned  
**Owner:** Unassigned  
**Effort:** 2 weeks  
**Target:** v1.1

**What's Needed:**

- Structured logging with correlation IDs
- Prometheus metrics collection
- Distributed tracing (OpenTelemetry)
- Health check endpoints
- Application performance monitoring (APM)
- Operational dashboards

**Guide:** [improvements/monitoring.md](./improvements/monitoring.md)

---

### 11. Development Tools 🟡

**Status:** 📋 Planned  
**Owner:** Unassigned  
**Effort:** 1-2 weeks  
**Target:** v1.1

**What's Needed:**

- Hot reloading development server
- Enhanced error pages with stack traces
- Development middleware pipeline
- Project scaffolding CLI

**Guide:** [improvements/development-tools.md](./improvements/development-tools.md)

---

## 🟢 Low Priority (Future Enhancements - v2.0+)

These are valuable but not critical for initial production use.

### 12. Template Engine 🟢

**Status:** 💭 Needs Planning  
**Owner:** Unassigned  
**Target:** v2.0+

**What's Needed:**

- Server-side template rendering (Handlebars/Tera)
- Template caching
- Template inheritance and partials
- Integration with JavaScript handlers

**Guide:** [features/template-engine.md](./features/template-engine.md)

---

### 13. Email Support 🟢

**Status:** 💭 Needs Planning  
**Owner:** Unassigned  
**Target:** v2.0+

**What's Needed:**

- SMTP client integration
- Email template system
- Async email queue
- Delivery tracking and retries

**Guide:** [features/email-support.md](./features/email-support.md)

---

### 14. Background Job Processing 🟢

**Status:** 💭 Needs Planning  
**Owner:** Unassigned  
**Target:** v2.0+

**What's Needed:**

- Job queue with persistent storage
- Worker process management
- Job scheduling and retry logic
- Job monitoring and status tracking

**Guide:** [features/background-jobs.md](./features/background-jobs.md)

---

### 15. Model Context Protocol (MCP) Support 🟢

**Status:** 💭 Needs Planning  
**Owner:** Unassigned  
**Target:** v2.0+

**What's Needed:**

- MCP specification implementation
- Tool and prompt registration APIs
- MCP message handling and routing
- MCP client integration

**Guide:** [features/mcp-integration.md](./features/mcp-integration.md)

---

### 16. Internationalization (i18n) 🟢

**Status:** 💭 Needs Planning  
**Owner:** Unassigned  
**Target:** v2.0+

**What's Needed:**

- Translation key management
- Locale file support (JSON/YAML)
- Translation function in JavaScript
- Pluralization and number formatting

**Guide:** [features/internationalization.md](./features/internationalization.md)

---

### 17. Advanced Authentication Features 🟢

**Status:** 💭 Needs Planning  
**Owner:** Unassigned  
**Prerequisites:** Basic authentication (item 5) complete  
**Target:** v2.0+

**Features:**

- Multi-factor authentication (TOTP, WebAuthn)
- Social login extensions (GitHub, Discord, Twitter)
- SAML support for enterprise SSO
- API key authentication for service-to-service
- Advanced session management (clustering, distributed storage)

**Guide:** [features/authentication-advanced.md](./features/authentication-advanced.md)

---

## 📅 Detailed Timeline

### Phase 0: Critical Prerequisites (Weeks 1-4)

**Week 1-2: Stability & Error Handling**

- Days 1-2: Remove all `unwrap()` calls
- Days 3-4: Fix failing tests
- Day 5: Add timeout mechanisms

**Week 2-3: Security Integration**

- Days 1-2: Remove legacy code, connect SecureOperations
- Days 3-4: Complete async handlers and audit integration
- Day 5: Integration testing

**Week 3: Testing Infrastructure**

- Days 1-2: Create security integration tests
- Days 3-4: Add missing tests, achieve coverage targets
- Day 5: Set up coverage reporting

**Week 4: Configuration & Secrets**

- Days 1-2: Add AuthConfig, create .env.example
- Days 3-4: Implement validation, environment configs
- Day 5: Testing and documentation

### Phase 1: Authentication (Weeks 5-12)

**Weeks 5-6: Session & OAuth Foundation**

- Secure session management with encryption
- CSRF protection framework
- OAuth provider implementations

**Weeks 7-8: Middleware & Routes**

- Authentication middleware with rate limiting
- Login/callback/logout routes
- Security header integration

**Weeks 9-10: JavaScript Integration**

- User context in request objects
- JavaScript authentication APIs
- Protected resource examples

**Weeks 11-12: Testing & Validation**

- Comprehensive testing (unit, integration, e2e)
- Security penetration testing
- Performance validation
- Documentation completion

### Phase 2: Production Readiness (Weeks 13-17)

**Weeks 13-14: Configuration Management**

- Environment profiles
- Configuration validation
- Hot-reloading

**Weeks 15-17: Performance & Scalability**

- Script caching
- Performance optimization
- Load testing
- Production deployment validation

### v1.0 Release (Week 18)

**Success Criteria:**

- All critical prerequisites complete
- Authentication working with 3+ providers
- > 80% test coverage, all tests passing
- Security framework fully operational
- Production deployment validated
- Complete documentation

---

## 🎯 Success Metrics by Phase

### Phase 0 (Prerequisites) - Gate for Phase 1

- [ ] Zero `unwrap()` calls in production code
- [ ] 100% test pass rate (126/126 tests)
- [ ] All 18 security TODOs resolved
- [ ] > 80% test coverage
- [ ] Security integration tests passing
- [ ] Configuration supports authentication

### Phase 1 (Authentication) - Gate for v1.0

- [ ] Users can authenticate with 3+ OAuth providers
- [ ] Sessions persist securely across restarts
- [ ] JavaScript handlers access user context
- [ ] All authentication security requirements met
- [ ] Authentication tests at >90% coverage
- [ ] No authentication-related security vulnerabilities

### Phase 2 (Production) - Gate for v1.0 Release

- [ ] Performance benchmarks met (see improvements/performance.md)
- [ ] Production deployment tested in staging
- [ ] Monitoring and alerting operational
- [ ] Documentation complete and accurate
- [ ] Administrator guides updated
- [ ] Example applications demonstrating features

---

## 🚦 How to Use This Roadmap

### For Contributors

**Starting new work:**

1. Check this roadmap for priority and status
2. Ensure prerequisites are complete
3. Read the relevant guide in `/features/` or `/improvements/`
4. Follow implementation guidelines in `/guides/`
5. Submit PR following [CONTRIBUTING.md](./CONTRIBUTING.md)

**Updating status:**

1. Change status indicators (🚧, ✅, etc.)
2. Update task checkboxes as work progresses
3. Add owner name when claiming work
4. Update target dates if timeline changes

### For Maintainers

**Reviewing priorities:**

- Critical (🔴) items are non-negotiable for v1.0
- High (🟠) items are required for production use
- Medium (🟡) items improve quality of life
- Low (🟢) items are future enhancements

**Accepting contributions:**

- Ensure work aligns with roadmap priorities
- Verify prerequisites are met
- Check that success metrics are defined
- Validate comprehensive testing

---

## 🔄 Roadmap Updates

This roadmap is a living document updated as:

- Work is completed (status changes)
- Priorities shift based on user needs
- New requirements emerge
- Technical constraints are discovered

**Update frequency:** At minimum, after each sprint (2 weeks)

**Last major review:** October 24, 2025 (reorganization)

---

## 📞 Questions About the Roadmap?

- **Priority questions:** Open a GitHub Discussion
- **Timeline concerns:** Comment on relevant GitHub Issue
- **New feature proposals:** See [CONTRIBUTING.md](./CONTRIBUTING.md)
- **Status updates:** Submit PR updating this file

---

_This roadmap consolidates information from TODO.md, SECURITY_TODO.md, AUTH_TODO.md, and URGENT_TODO.md into a single prioritized view._
