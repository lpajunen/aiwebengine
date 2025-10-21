# HTTPS Quick Start

Quick guide to deploy aiwebengine with HTTPS on softagen.com.

## Prerequisites Checklist

- [ ] DNS records configured (all domains point to your server IP)
- [ ] Ports 80 and 443 open in firewall
- [ ] Docker and Docker Compose installed
- [ ] `.env` file configured with secrets

## DNS Setup

Configure these DNS records with your domain registrar:

```text
Type   Name                Value
----   ----                -----
A      softagen.com        YOUR_SERVER_IP
A      www.softagen.com    YOUR_SERVER_IP
A      test.softagen.com   YOUR_SERVER_IP
```

Or use CNAME for subdomains:

```text
Type   Name                Value
----   ----                -----
A      softagen.com        YOUR_SERVER_IP
CNAME  www.softagen.com    softagen.com
CNAME  test.softagen.com   softagen.com
```

## Verify DNS

```bash
dig softagen.com +short
dig www.softagen.com +short
dig test.softagen.com +short
```

All should return your server IP.

## Deploy

```bash
# 1. Navigate to project directory
cd /path/to/aiwebengine

# 2. Start services
docker-compose up -d

# 3. Watch logs for certificate issuance (first time only)
docker-compose logs -f caddy

# Wait for: "certificate obtained successfully"
```

## Test

```bash
# Test main domain (should return 200)
curl -I https://softagen.com

# Test www redirect (should return 301)
curl -I https://www.softagen.com
# Location: https://softagen.com

# Test test subdomain redirect (should return 301)
curl -I https://test.softagen.com
# Location: https://softagen.com
```

## Verify in Browser

Visit these URLs:

1. <https://softagen.com> → Should load with valid HTTPS
2. <https://www.softagen.com> → Should redirect to softagen.com
3. <https://test.softagen.com> → Should redirect to softagen.com
4. <http://softagen.com> → Should redirect to HTTPS

## Common Issues

### "Certificate not yet issued"

**Wait 30-60 seconds** for Let's Encrypt to validate and issue certificate.

```bash
# Check logs
docker-compose logs caddy | grep -i certificate
```

### "Connection refused"

Check ports are open:

```bash
sudo ufw allow 80/tcp
sudo ufw allow 443/tcp
sudo ufw allow 443/udp
```

### "DNS not resolving"

Wait for DNS propagation (can take up to 48 hours, usually 5-15 minutes).

```bash
# Check current DNS
dig softagen.com +trace
```

## Architecture

```text
Internet
   ↓
[Port 80/443]
   ↓
Caddy (caddy:2-alpine)
   ├─ Automatic HTTPS
   ├─ Certificate management
   └─ Domain redirects
   ↓
[Internal HTTP :3000]
   ↓
aiwebengine (Rust app)
```

## Important Files

- `Caddyfile` - Production Caddy configuration
- `Caddyfile.dev` - Development Caddy configuration
- `docker-compose.yml` - Production deployment with Caddy
- `docker-compose.dev.yml` - Development deployment

## Maintenance

### View Logs

```bash
# Caddy logs
docker-compose logs -f caddy

# Application logs
docker-compose logs -f aiwebengine
```

### Restart Services

```bash
# Restart all
docker-compose restart

# Restart only Caddy (reload config)
docker-compose restart caddy
```

### Update Services

```bash
# Pull latest images
docker-compose pull

# Rebuild and restart
docker-compose up -d --build
```

## URLs Reference

| URL                          | Behavior                          |
|------------------------------|-----------------------------------|
| `https://softagen.com`       | Main application (HTTPS)          |
| `http://softagen.com`        | Redirects to HTTPS                |
| `https://www.softagen.com`   | Redirects to softagen.com         |
| `http://www.softagen.com`    | Redirects to https://softagen.com |
| `https://test.softagen.com`  | Redirects to softagen.com         |
| `http://test.softagen.com`   | Redirects to https://softagen.com |

## Security Features

✅ Automatic HTTPS (Let's Encrypt)  
✅ Automatic certificate renewal  
✅ HTTP → HTTPS redirect  
✅ Security headers (X-Frame-Options, etc.)  
✅ Gzip compression  
✅ HTTP/3 support

## Need Help?

See full documentation: `docs/engine-administrators/HTTPS_SETUP.md`
