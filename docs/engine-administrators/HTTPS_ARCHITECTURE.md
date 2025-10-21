# HTTPS Architecture Overview

## Production Architecture

```text
┌─────────────────────────────────────────────────────────────────┐
│                         Internet                                 │
└───────────────────────────┬─────────────────────────────────────┘
                            │
                            │ DNS Resolution
                            │
                ┌───────────┴───────────┐
                │                       │
        softagen.com          www.softagen.com
        test.softagen.com     (all → same IP)
                │
                │
                ▼
        ┌───────────────┐
        │   Port 80     │ ◄──── HTTP requests (redirected to HTTPS)
        │   Port 443    │ ◄──── HTTPS requests
        │   Port 443/udp│ ◄──── HTTP/3
        └───────┬───────┘
                │
                ▼
    ┌──────────────────────────┐
    │     Caddy Server         │
    │   (caddy:2-alpine)       │
    │                          │
    │  ✓ TLS Termination       │
    │  ✓ Let's Encrypt         │
    │  ✓ Auto Cert Renewal     │
    │  ✓ Domain Redirects      │
    │  ✓ Security Headers      │
    │  ✓ Gzip Compression      │
    └──────────┬───────────────┘
               │
               │ Reverse Proxy (HTTP)
               │ Internal Network
               │
               ▼
    ┌──────────────────────────┐
    │   aiwebengine            │
    │   (Rust/Axum)            │
    │                          │
    │   Port: 3000 (internal)  │
    │   Protocol: HTTP         │
    │                          │
    │  ✓ Request Handling      │
    │  ✓ JavaScript Runtime    │
    │  ✓ GraphQL API           │
    │  ✓ Business Logic        │
    └──────────┬───────────────┘
               │
               │
       ┌───────┴────────┐
       │                │
       ▼                ▼
   ┌──────────┐    ┌──────────┐
   │PostgreSQL│    │  Redis   │
   │(optional)│    │(optional)│
   └──────────┘    └──────────┘
```

## URL Flow Examples

### Example 1: Main Domain Access

```text
User Browser
    │
    │ http://softagen.com/api/data
    ▼
Caddy
    │ 301 Redirect
    │ Location: https://softagen.com/api/data
    ▼
User Browser
    │
    │ https://softagen.com/api/data
    ▼
Caddy (TLS Termination)
    │ Decrypts HTTPS
    │ http://aiwebengine:3000/api/data
    ▼
aiwebengine
    │ Processes request
    │ Returns response
    ▼
Caddy
    │ Encrypts response
    │ Adds security headers
    ▼
User Browser (Receives HTTPS response)
```

### Example 2: WWW Redirect

```text
User Browser
    │
    │ https://www.softagen.com/page
    ▼
Caddy
    │ 301 Permanent Redirect
    │ Location: https://softagen.com/page
    ▼
User Browser
    │ Follows redirect
    │ https://softagen.com/page
    ▼
Caddy → aiwebengine → Response
```

### Example 3: Test Subdomain Redirect

```text
User Browser
    │
    │ https://test.softagen.com/dashboard
    ▼
Caddy
    │ 301 Permanent Redirect
    │ Location: https://softagen.com/dashboard
    ▼
User Browser
    │ Follows redirect
    │ https://softagen.com/dashboard
    ▼
Caddy → aiwebengine → Response
```

## Certificate Management Flow

```text
Initial Deployment
    │
    ▼
Caddy Starts
    │
    │ Checks for certificate
    │ (not found - first run)
    ▼
Let's Encrypt ACME Challenge
    │
    │ 1. Caddy creates .well-known/acme-challenge
    │ 2. Let's Encrypt validates domain ownership
    │ 3. Issues certificate (if validation passes)
    ▼
Certificate Stored
    │
    │ Location: /data/caddy/certificates/
    │ Auto-renewal: ~30 days before expiry
    ▼
HTTPS Serving
```

## Docker Network Architecture

```text
┌─────────────────────────────────────────────────────────┐
│              aiwebengine-network (bridge)                │
│                                                          │
│  ┌──────────────┐          ┌──────────────┐            │
│  │    Caddy     │◄────────►│ aiwebengine  │            │
│  │ :80, :443    │   HTTP   │    :3000     │            │
│  └──────────────┘          └──────┬───────┘            │
│                                    │                     │
│                      ┌─────────────┴─────────────┐      │
│                      │                           │      │
│              ┌───────▼──────┐          ┌────────▼─────┐│
│              │  PostgreSQL  │          │    Redis     ││
│              │    :5432     │          │    :6379     ││
│              └──────────────┘          └──────────────┘│
│                                                          │
└─────────────────────────────────────────────────────────┘
              │                                │
              │ Host Port Mapping              │
              │ (only Caddy exposed)           │
              ▼                                ▼
        Host :80, :443                  (internal only)
```

## Security Layers

```text
┌─────────────────────────────────────────────────────────┐
│                    Security Layers                       │
├─────────────────────────────────────────────────────────┤
│ Layer 1: Network                                         │
│  ✓ Firewall (UFW) - Only 80, 443, SSH open             │
│  ✓ Docker network isolation                             │
│  ✓ No direct aiwebengine exposure                       │
├─────────────────────────────────────────────────────────┤
│ Layer 2: Caddy (Edge)                                    │
│  ✓ TLS 1.2+ only                                        │
│  ✓ Strong cipher suites                                 │
│  ✓ Automatic certificate management                     │
│  ✓ Security headers (X-Frame-Options, etc.)            │
│  ✓ HTTP → HTTPS redirect                               │
├─────────────────────────────────────────────────────────┤
│ Layer 3: Application (aiwebengine)                       │
│  ✓ Input validation                                     │
│  ✓ Authentication/Authorization (if enabled)            │
│  ✓ Rate limiting (configured)                           │
│  ✓ CORS policies                                        │
│  ✓ JavaScript sandboxing                                │
├─────────────────────────────────────────────────────────┤
│ Layer 4: Data                                            │
│  ✓ Database access control                              │
│  ✓ Encrypted secrets                                    │
│  ✓ Secure credential storage                            │
└─────────────────────────────────────────────────────────┘
```

## Benefits of This Architecture

### 1. Separation of Concerns

- **Caddy**: Handles all TLS/security/edge concerns
- **aiwebengine**: Focuses purely on business logic
- Easy to update either component independently

### 2. Zero Maintenance HTTPS

- Automatic certificate issuance
- Automatic renewal (no downtime)
- No manual certificate management

### 3. Flexible Deployment

- Add/remove domains easily in Caddyfile
- Easy to add rate limiting, caching, etc.
- Can add multiple backend services

### 4. Production Ready

- Battle-tested reverse proxy pattern
- High performance (Caddy written in Go)
- Minimal resource overhead
- Built-in HTTP/3 support

### 5. Development Friendly

- Same application code for dev and prod
- Different Caddyfile for local development
- Easy to test HTTPS locally if needed

## Performance Characteristics

### Latency Impact

```text
Request without Caddy:
  Client → aiwebengine → Client
  Latency: ~1-10ms

Request with Caddy:
  Client → Caddy → aiwebengine → Caddy → Client
  Additional latency: ~0.1-1ms (negligible)
```

### Throughput

- Caddy: Handles 100,000+ requests/sec (depending on hardware)
- Reverse proxy overhead: <1% in typical scenarios
- Connection pooling and HTTP/2 multiplexing improve performance

### Resource Usage

```text
Caddy Container:
  Memory: ~10-50 MB
  CPU: <1% idle, <5% under load
  
aiwebengine Container:
  Memory: Depends on workload (~50-200 MB typical)
  CPU: Depends on JavaScript execution
```

## Alternative Architectures Considered

### Native TLS in Rust

```text
❌ NOT RECOMMENDED

Internet → aiwebengine (with TLS) → Database

Pros:
  - One less container
  - Slightly lower latency

Cons:
  - Manual certificate management
  - Application restart for cert renewal
  - More complex application code
  - Less flexible for adding edge features
```

### Nginx Reverse Proxy

```text
✓ ALSO GOOD, but more complex

Internet → Nginx → aiwebengine → Database

Pros:
  - Very mature
  - Excellent performance
  - Widely known

Cons:
  - More complex configuration
  - Manual certificate management (need certbot)
  - Requires separate renewal automation
```

### Cloud Load Balancer

```text
✓ GOOD for cloud deployments

Internet → AWS ALB/GCP LB → aiwebengine → Database

Pros:
  - Managed by cloud provider
  - Built-in DDoS protection
  - Auto-scaling

Cons:
  - Cloud vendor lock-in
  - Additional cost
  - Not suitable for self-hosting
```

## Monitoring Recommendations

### Metrics to Track

1. **Certificate Expiration**
   - Alert 30 days before expiry
   - Should auto-renew, but monitor for failures

2. **HTTPS Availability**
   - Uptime monitoring (UptimeRobot, Pingdom)
   - Check frequency: Every 5 minutes

3. **SSL/TLS Configuration**
   - Monthly SSL Labs scan
   - Target: A+ rating

4. **Performance**
   - Response times through Caddy
   - Error rates
   - Request volumes

5. **Security**
   - Failed authentication attempts
   - Unusual traffic patterns
   - Certificate validation errors

## Disaster Recovery

### Backup Critical Data

1. **Caddy Certificates** (stored in `caddy-data` volume)
2. **Application Database** (PostgreSQL data)
3. **Application Configuration** (`.env`, config files)
4. **Custom Scripts** (`scripts/` directory)

### Recovery Steps

1. Deploy new server
2. Restore configuration files
3. Start services with `docker-compose up -d`
4. Restore Caddy certificates from backup (optional - Caddy can reissue)
5. Restore database data
6. Verify HTTPS is working

### RTO/RPO Targets

- **Recovery Time Objective (RTO)**: < 1 hour
- **Recovery Point Objective (RPO)**: < 24 hours (depending on backup frequency)

## See Also

- [HTTPS Setup Guide](HTTPS_SETUP.md) - Detailed setup instructions
- [HTTPS Quick Start](HTTPS_QUICK_START.md) - Quick reference
- [Production Checklist](PRODUCTION_CHECKLIST.md) - Deployment checklist
