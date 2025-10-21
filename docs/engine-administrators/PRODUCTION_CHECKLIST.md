# Production Deployment Checklist

Use this checklist when deploying aiwebengine to production with HTTPS.

## Pre-Deployment

### DNS Configuration

- [ ] Configure A records for all domains:
  - [ ] `softagen.com` → Your server IP
  - [ ] `www.softagen.com` → Your server IP
  - [ ] `test.softagen.com` → Your server IP
- [ ] Verify DNS propagation: `dig softagen.com +short`
- [ ] Verify all subdomains resolve correctly
- [ ] TTL set appropriately (300-3600 seconds recommended)

### Server Setup

- [ ] Server OS updated: `sudo apt update && sudo apt upgrade`
- [ ] Docker installed and running: `docker --version`
- [ ] Docker Compose installed: `docker-compose --version`
- [ ] Firewall configured:
  - [ ] Port 80 open: `sudo ufw allow 80/tcp`
  - [ ] Port 443 open: `sudo ufw allow 443/tcp`
  - [ ] Port 443/udp open (HTTP/3): `sudo ufw allow 443/udp`
  - [ ] SSH port open: `sudo ufw allow 22/tcp`
  - [ ] Firewall enabled: `sudo ufw enable`
- [ ] Verify no other services using ports 80/443
- [ ] Set up SSH key authentication (disable password auth)

### Application Configuration

- [ ] Clone repository: `git clone https://github.com/lpajunen/aiwebengine.git`
- [ ] Copy environment file: `cp .env.example .env`
- [ ] Edit `.env` with production values:
  - [ ] Generate strong JWT_SECRET: `openssl rand -base64 32`
  - [ ] Generate strong SESSION_SECRET: `openssl rand -base64 32`
  - [ ] Set OAuth credentials (if using authentication)
  - [ ] Set database password (if using PostgreSQL)
  - [ ] Set Grafana password (if using monitoring)
- [ ] Review `config.prod.toml` settings
- [ ] Review `Caddyfile` - verify domains are correct
- [ ] Ensure scripts directory has required scripts

## Deployment

### Initial Deployment

- [ ] Build images: `docker-compose build`
- [ ] Start services: `docker-compose up -d`
- [ ] Monitor startup: `docker-compose logs -f`
- [ ] Wait for Caddy certificate issuance (30-60 seconds)
- [ ] Check all containers running: `docker-compose ps`

### Verification

- [ ] Health check: `curl http://localhost:3000/health`
- [ ] HTTPS main domain: `curl -I https://softagen.com`
- [ ] Certificate valid: `curl -vI https://softagen.com 2>&1 | grep "SSL certificate verify ok"`
- [ ] WWW redirect works: `curl -I https://www.softagen.com | grep "Location: https://softagen.com"`
- [ ] Test subdomain redirect: `curl -I https://test.softagen.com | grep "Location: https://softagen.com"`
- [ ] HTTP redirects to HTTPS: `curl -I http://softagen.com | grep "Location: https://"`
- [ ] Check browser - no certificate warnings
- [ ] Test application functionality:
  - [ ] API endpoints working
  - [ ] GraphQL interface accessible (if enabled)
  - [ ] Editor interface loading (if using)
  - [ ] Authentication working (if enabled)

### SSL/TLS Verification

- [ ] Test SSL configuration: Visit <https://www.ssllabs.com/ssltest/>
- [ ] Target grade: A or A+
- [ ] Certificate chain complete
- [ ] No protocol vulnerabilities
- [ ] Strong cipher suites enabled

## Post-Deployment

### Monitoring Setup

- [ ] Configure log rotation: `docker-compose logs --tail=100 > deployment.log`
- [ ] Set up uptime monitoring (UptimeRobot, Pingdom, etc.)
- [ ] Configure certificate expiration alerts
- [ ] Set up error alerting (Sentry, email, etc.)
- [ ] Review Prometheus/Grafana dashboards (if enabled)

### Security Hardening

- [ ] Review security headers: `curl -I https://softagen.com`
- [ ] Enable fail2ban (if available)
- [ ] Configure automated security updates
- [ ] Set up backup schedule for:
  - [ ] Database data
  - [ ] SSL certificates: `docker volume backup`
  - [ ] Application configuration
  - [ ] Scripts directory
- [ ] Document disaster recovery procedure
- [ ] Test backup restoration process

### Documentation

- [ ] Document production server details (IP, SSH access, etc.)
- [ ] Document environment variables and secrets (in secure location)
- [ ] Document backup procedures
- [ ] Document rollback procedures
- [ ] Update team wiki/runbook with deployment info

## Maintenance Schedule

### Daily

- [ ] Check service status: `docker-compose ps`
- [ ] Review error logs: `docker-compose logs --tail=50 aiwebengine | grep -i error`
- [ ] Monitor disk space: `df -h`

### Weekly

- [ ] Review access logs for anomalies
- [ ] Check certificate expiration: `echo | openssl s_client -connect softagen.com:443 2>/dev/null | openssl x509 -noout -dates`
- [ ] Review performance metrics
- [ ] Check for application updates: `git fetch origin`

### Monthly

- [ ] Update Docker images: `docker-compose pull`
- [ ] Restart services: `docker-compose up -d`
- [ ] Review and rotate logs
- [ ] Test backup restoration
- [ ] Review and update dependencies
- [ ] Security audit

### Quarterly

- [ ] Full SSL/TLS audit: <https://www.ssllabs.com/ssltest/>
- [ ] Review and update security policies
- [ ] Disaster recovery drill
- [ ] Performance optimization review
- [ ] Documentation review and updates

## Rollback Procedure

If something goes wrong:

### Quick Rollback

```bash
# Stop services
docker-compose down

# Restore from previous working version
git checkout <previous-commit>

# Rebuild and start
docker-compose build
docker-compose up -d
```

### Certificate Rollback

```bash
# Restore Caddy certificates from backup
docker run --rm -v aiwebengine_caddy-data:/data -v $(pwd):/backup alpine \
  tar xzf /backup/caddy-data-backup-YYYYMMDD.tar.gz -C /data

# Restart Caddy
docker-compose restart caddy
```

## Troubleshooting Quick Reference

### Services Won't Start

```bash
# Check logs
docker-compose logs

# Check disk space
df -h

# Check ports
sudo netstat -tulpn | grep -E ':(80|443|3000)'
```

### Certificate Issues

```bash
# View Caddy logs
docker-compose logs caddy | grep -i cert

# Force certificate renewal
docker-compose restart caddy

# Check certificate details
echo | openssl s_client -connect softagen.com:443 -servername softagen.com 2>/dev/null | openssl x509 -noout -text
```

### Application Errors

```bash
# Check application logs
docker-compose logs aiwebengine

# Check health endpoint
curl http://localhost:3000/health

# Restart application only
docker-compose restart aiwebengine
```

### Performance Issues

```bash
# Check resource usage
docker stats

# Check container logs
docker-compose logs --tail=100

# Review metrics in Grafana (if enabled)
```

## Emergency Contacts

Document your emergency contacts:

- [ ] Server provider support: _______________
- [ ] DNS provider support: _______________
- [ ] Team lead: _______________
- [ ] On-call engineer: _______________
- [ ] Backup administrator: _______________

## Additional Resources

- [HTTPS Setup Guide](HTTPS_SETUP.md) - Detailed HTTPS configuration
- [HTTPS Quick Start](HTTPS_QUICK_START.md) - Quick reference
- [Docker Documentation](DOCKER.md) - Docker deployment details
- [Configuration Guide](CONFIGURATION.md) - Application configuration

## Sign-Off

Deployment completed by: _______________  
Date: _______________  
Deployment version/commit: _______________  
All checks passed: [ ]  
Post-deployment monitoring confirmed: [ ]

---

**Note:** Keep this checklist updated as your deployment process evolves.
