# HTTPS Setup Guide

This guide explains how to set up HTTPS for aiwebengine using Caddy as a reverse proxy.

## Overview

The production setup uses:
- **Caddy** as a reverse proxy (handles HTTPS, certificates, redirects)
- **aiwebengine** as the backend application server (HTTP only, internal)

### Benefits of This Architecture

✅ **Automatic HTTPS** - Caddy obtains and renews Let's Encrypt certificates automatically  
✅ **Zero code changes** - Application remains simple, runs on HTTP internally  
✅ **Automatic redirects** - www and test subdomains redirect to main domain  
✅ **Security headers** - Caddy adds security headers automatically  
✅ **HTTP/3 support** - Modern protocol support out of the box  
✅ **Compression** - Automatic gzip compression for responses  

## Domain Configuration

### Production Domains

The following domains are configured:

1. **`softagen.com`** (primary)
   - Serves the application over HTTPS
   - Automatic Let's Encrypt certificate

2. **`www.softagen.com`** (redirect)
   - Permanently redirects to `https://softagen.com`
   - Preserves the URI path

3. **`test.softagen.com`** (redirect)
   - Permanently redirects to `https://softagen.com`
   - Preserves the URI path

### Redirect Behavior

```
https://www.softagen.com/api/foo  →  https://softagen.com/api/foo
https://test.softagen.com/editor  →  https://softagen.com/editor
http://softagen.com/anything      →  https://softagen.com/anything
```

## Prerequisites

Before deploying, ensure:

### 1. DNS Configuration

All domains must point to your server's IP address:

```
A    softagen.com      →  YOUR_SERVER_IP
A    www.softagen.com  →  YOUR_SERVER_IP
A    test.softagen.com →  YOUR_SERVER_IP
```

Or use CNAME records for subdomains:

```
A     softagen.com      →  YOUR_SERVER_IP
CNAME www.softagen.com  →  softagen.com
CNAME test.softagen.com →  softagen.com
```

**Verify DNS propagation:**
```bash
# Check DNS resolution
dig softagen.com
dig www.softagen.com
dig test.softagen.com

# Or use nslookup
nslookup softagen.com
```

### 2. Firewall Configuration

Open required ports on your server:

```bash
# Allow HTTP (required for Let's Encrypt challenge)
sudo ufw allow 80/tcp

# Allow HTTPS
sudo ufw allow 443/tcp
sudo ufw allow 443/udp  # HTTP/3

# Check firewall status
sudo ufw status
```

### 3. Server Requirements

- Docker and Docker Compose installed
- Ports 80 and 443 available (not used by other services)
- Valid email address for Let's Encrypt notifications

## Deployment Steps

### Step 1: Configure Environment

Create or update your `.env` file:

```bash
# OAuth credentials (if needed)
GOOGLE_CLIENT_ID=your-google-client-id
GOOGLE_CLIENT_SECRET=your-google-client-secret
JWT_SECRET=your-production-jwt-secret
SESSION_SECRET=your-production-session-secret

# Database
POSTGRES_PASSWORD=your-secure-database-password

# Monitoring (optional)
GRAFANA_PASSWORD=your-grafana-password
```

**Important:** Use strong, unique secrets in production!

### Step 2: Review Caddyfile

Check the `Caddyfile` configuration:

```bash
cat Caddyfile
```

The configuration should include your domains. Modify if needed:

```caddy
# Main domain
softagen.com {
    reverse_proxy aiwebengine:3000
    encode gzip
    # ... other settings
}

# Redirects
www.softagen.com {
    redir https://softagen.com{uri} permanent
}

test.softagen.com {
    redir https://softagen.com{uri} permanent
}
```

### Step 3: Deploy with Docker Compose

```bash
# Build and start services
docker-compose up -d

# Check logs
docker-compose logs -f caddy
docker-compose logs -f aiwebengine
```

### Step 4: Verify Certificate Issuance

Caddy automatically obtains certificates on first request. Monitor the process:

```bash
# Watch Caddy logs during certificate issuance
docker-compose logs -f caddy

# You should see logs like:
# "certificate obtained successfully"
# "serving from storage"
```

**First certificate issuance may take 30-60 seconds.**

### Step 5: Test HTTPS

```bash
# Test main domain
curl -I https://softagen.com

# Test redirect from www
curl -I https://www.softagen.com

# Test redirect from test subdomain
curl -I https://test.softagen.com

# Check SSL certificate
openssl s_client -connect softagen.com:443 -servername softagen.com
```

## Verification Checklist

- [ ] DNS records point to your server
- [ ] Ports 80 and 443 are open
- [ ] Docker containers are running
- [ ] `https://softagen.com` loads successfully
- [ ] `https://www.softagen.com` redirects to `https://softagen.com`
- [ ] `https://test.softagen.com` redirects to `https://softagen.com`
- [ ] SSL certificate is valid (not self-signed)
- [ ] HTTP automatically redirects to HTTPS

## Certificate Management

### Automatic Renewal

Caddy automatically renews certificates before expiration. No manual intervention needed!

- Renewal occurs ~30 days before expiration
- Zero downtime during renewal
- Certificates stored in `caddy-data` volume

### Certificate Location

Certificates are stored in the Docker volume:

```bash
# Inspect certificate storage
docker volume inspect aiwebengine_caddy-data

# View certificates (inside container)
docker exec aiwebengine-caddy ls -la /data/caddy/certificates/
```

### Manual Certificate Renewal (if needed)

```bash
# Restart Caddy to force certificate check
docker-compose restart caddy

# Or reload configuration
docker exec aiwebengine-caddy caddy reload --config /etc/caddy/Caddyfile
```

## Troubleshooting

### Issue: Certificate Not Issued

**Symptoms:** SSL error, self-signed certificate, or timeout

**Solutions:**

1. **Check DNS propagation:**
   ```bash
   dig softagen.com
   # Should show your server IP
   ```

2. **Verify ports are open:**
   ```bash
   sudo netstat -tulpn | grep -E ':(80|443)'
   # Should show Caddy listening
   ```

3. **Check Caddy logs:**
   ```bash
   docker-compose logs caddy
   # Look for errors like "challenge failed"
   ```

4. **Verify Let's Encrypt can reach your server:**
   ```bash
   curl http://softagen.com/.well-known/acme-challenge/test
   ```

### Issue: Redirect Loop

**Symptoms:** Browser shows "Too many redirects"

**Solution:** Check that aiwebengine is not also trying to redirect to HTTPS internally.

### Issue: 502 Bad Gateway

**Symptoms:** Caddy responds but can't reach aiwebengine

**Solutions:**

1. **Check aiwebengine is running:**
   ```bash
   docker-compose ps aiwebengine
   ```

2. **Check network connectivity:**
   ```bash
   docker exec aiwebengine-caddy wget -O- http://aiwebengine:3000/health
   ```

3. **Check aiwebengine logs:**
   ```bash
   docker-compose logs aiwebengine
   ```

### Issue: Port Already in Use

**Symptoms:** `Error starting userland proxy: listen tcp 0.0.0.0:443: bind: address already in use`

**Solution:** Stop conflicting service:

```bash
# Find process using port 443
sudo lsof -i :443

# Stop the service (example with nginx)
sudo systemctl stop nginx

# Or use different ports in docker-compose.yml:
# ports:
#   - "8080:80"
#   - "8443:443"
```

## Advanced Configuration

### Custom SSL Certificates

If you have your own certificates (not using Let's Encrypt):

```caddy
softagen.com {
    tls /path/to/cert.pem /path/to/key.pem
    reverse_proxy aiwebengine:3000
}
```

### Rate Limiting

Add rate limiting to protect against abuse:

```caddy
softagen.com {
    rate_limit {
        zone softagen {
            key {remote_host}
            events 100
            window 1m
        }
    }
    reverse_proxy aiwebengine:3000
}
```

### IP Whitelisting

Restrict access to certain IPs:

```caddy
softagen.com {
    @allowed {
        remote_ip 1.2.3.4 5.6.7.8
    }
    handle @allowed {
        reverse_proxy aiwebengine:3000
    }
    handle {
        abort
    }
}
```

### Staging Environment

To use `test.softagen.com` as a separate staging environment instead of redirecting:

```caddy
# Production
softagen.com {
    reverse_proxy aiwebengine:3000
}

# Staging (separate container)
test.softagen.com {
    reverse_proxy aiwebengine-staging:3000
}

# Redirect www
www.softagen.com {
    redir https://softagen.com{uri} permanent
}
```

## Monitoring

### Check Certificate Expiration

```bash
# Via OpenSSL
echo | openssl s_client -servername softagen.com -connect softagen.com:443 2>/dev/null | \
  openssl x509 -noout -dates

# Via curl
curl -vI https://softagen.com 2>&1 | grep -i "expire"
```

### Access Logs

Caddy logs are available in multiple ways:

```bash
# Docker logs
docker-compose logs -f caddy

# Log file (if configured)
docker exec aiwebengine-caddy tail -f /var/log/caddy/access.log

# View all logs in volume
docker exec aiwebengine-caddy ls -la /var/log/caddy/
```

### Metrics

For production monitoring, consider adding:
- Prometheus metrics from Caddy
- Uptime monitoring (UptimeRobot, Pingdom)
- SSL certificate monitoring

## Security Best Practices

✅ Keep Caddy updated: `docker-compose pull caddy`  
✅ Use strong secrets in `.env` file  
✅ Enable firewall (ufw) with only necessary ports  
✅ Regular security audits: https://www.ssllabs.com/ssltest/  
✅ Monitor certificate expiration  
✅ Keep backups of `caddy-data` volume  
✅ Use fail2ban to prevent brute force attacks  

## Backup and Restore

### Backup Certificates

```bash
# Backup Caddy data (includes certificates)
docker run --rm -v aiwebengine_caddy-data:/data -v $(pwd):/backup alpine \
  tar czf /backup/caddy-data-backup-$(date +%Y%m%d).tar.gz -C /data .
```

### Restore Certificates

```bash
# Restore from backup
docker run --rm -v aiwebengine_caddy-data:/data -v $(pwd):/backup alpine \
  tar xzf /backup/caddy-data-backup-YYYYMMDD.tar.gz -C /data
```

## Migration from Other Reverse Proxies

### From Nginx

Caddy configuration is much simpler. Compare:

**Nginx:**
```nginx
server {
    listen 80;
    server_name softagen.com;
    return 301 https://$server_name$request_uri;
}

server {
    listen 443 ssl http2;
    server_name softagen.com;
    
    ssl_certificate /etc/letsencrypt/live/softagen.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/softagen.com/privkey.pem;
    
    location / {
        proxy_pass http://aiwebengine:3000;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }
}
```

**Caddy:**
```caddy
softagen.com {
    reverse_proxy aiwebengine:3000
}
```

### From Apache

Similar simplification - Caddy handles HTTPS, certificates, and proxying with minimal config.

## Support

For issues specific to:
- **Caddy:** https://caddy.community/
- **Let's Encrypt:** https://community.letsencrypt.org/
- **aiwebengine:** Check project documentation or GitHub issues

## Additional Resources

- [Caddy Documentation](https://caddyserver.com/docs/)
- [Let's Encrypt Documentation](https://letsencrypt.org/docs/)
- [SSL Labs Test](https://www.ssllabs.com/ssltest/)
- [Mozilla SSL Configuration Generator](https://ssl-config.mozilla.org/)
