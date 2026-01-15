# Security Policy

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues.**

If you discover a security vulnerability in aiwebengine, please report it privately to:

**Email:** lpajunen@gmail.com

Please include:

- Description of the vulnerability
- Steps to reproduce the issue
- Affected versions
- Potential impact
- Suggested fix (if any)

### What to Expect

- **Initial Response:** Within 48 hours
- **Status Updates:** Every 3-5 days until resolved
- **Fix Timeline:** Depends on severity
  - Critical: 1-7 days
  - High: 7-14 days
  - Medium: 14-30 days
  - Low: 30-90 days

### Disclosure Policy

We follow coordinated disclosure:

1. You report the vulnerability privately
2. We confirm and investigate the issue
3. We develop and test a fix
4. We release a security update
5. We publicly disclose the vulnerability after users have had time to update

**Please allow 90 days** before public disclosure unless we agree on a different timeline.

## Supported Versions

We support security updates for the following versions:

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |

**Note:** We are currently in early development (v0.1.x). Once we reach v1.0, we will support the latest major version and one previous major version.

## Security Best Practices

When deploying aiwebengine in production:

### 1. Configuration Security

- **Never commit secrets** to version control
- Use environment variables for all sensitive configuration
- Rotate JWT secrets regularly
- Use strong, randomly generated secrets (minimum 32 characters)
- Keep `config.toml` excluded from version control

### 2. Authentication & Authorization

- Enable authentication for all non-public endpoints
- Use strong password requirements
- Implement rate limiting on authentication endpoints
- Review and restrict OAuth provider configurations
- Regularly audit user roles and permissions

### 3. Network Security

- Use HTTPS/TLS for all production deployments
- Keep TLS certificates up to date
- Configure proper CORS policies
- Use secure session cookies with HttpOnly and Secure flags
- Implement proper CSP (Content Security Policy) headers

### 4. Script Security

- Review all scripts before deployment
- Use restricted (non-privileged) scripts whenever possible
- Audit privileged script access regularly
- Implement script approval workflows
- Monitor script execution logs

### 5. Database Security

- Use strong database passwords
- Restrict database network access
- Keep database backups encrypted
- Regularly update database software
- Use prepared statements (SQLx) to prevent SQL injection

### 6. Dependency Management

- Regularly update dependencies (`cargo update`)
- Monitor security advisories (`cargo audit`)
- Review dependency changes before updating
- Use lock files (`Cargo.lock`) in production

### 7. Logging & Monitoring

- Enable comprehensive logging
- Monitor for suspicious activity
- Set up alerts for security events
- Regularly review logs
- Keep logs secure and encrypted

### 8. Updates & Patches

- Subscribe to security announcements
- Test updates in staging before production
- Have a rollback plan
- Document your update process
- Keep production environments up to date

## Known Security Considerations

### Current Status (v0.1.x)

aiwebengine is in early development. The following features are under active development:

- **Authentication System:** OAuth2 providers (Google, Microsoft, Apple) are implemented but under active refinement
- **Authorization:** Role-based access control is functional but being enhanced
- **Script Sandbox:** QuickJS provides isolation but additional hardening is ongoing
- **Rate Limiting:** Basic implementation in place, advanced features planned
- **Audit Logging:** Partial implementation, comprehensive logging in progress

### Production Use Warning

⚠️ **aiwebengine is not yet recommended for production use with sensitive data.**

Current version (0.1.x) is suitable for:

- Development environments
- Internal tools with trusted users
- Testing and evaluation
- Non-critical applications

Wait for v1.0 release for:

- Production applications with sensitive data
- Public-facing services
- Critical business applications
- Applications requiring SOC2/ISO27001 compliance

## Security Advisories

Security advisories will be published in:

- GitHub Security Advisories: https://github.com/lpajunen/aiwebengine/security/advisories
- Release notes when patches are available
- Email notifications to maintainers

## Security Hall of Fame

We appreciate security researchers who help make aiwebengine more secure. Contributors who responsibly disclose vulnerabilities will be credited here (with permission).

---

**Thank you for helping keep aiwebengine and our community secure!**
