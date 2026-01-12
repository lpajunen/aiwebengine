#!/usr/bin/env bash
# Check DNS domain configuration for local development

set -e

DNS_DOMAIN="${DNS_DOMAIN:-local.softagen.com}"
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

echo "Checking DNS configuration for local development..."
echo ""

# Check if DIGITALOCEAN_TOKEN is set
if [ -n "$DIGITALOCEAN_TOKEN" ]; then
    echo -e "${GREEN}✓${NC} DIGITALOCEAN_TOKEN is set"
    TOKEN_STATUS="configured"
else
    echo -e "${YELLOW}⚠${NC}  DIGITALOCEAN_TOKEN not set"
    TOKEN_STATUS="not-configured"
fi

# Check if DNS domain resolves
if command -v dig &> /dev/null; then
    echo "Checking DNS resolution for $DNS_DOMAIN..."
    if dig +short "$DNS_DOMAIN" | grep -q .; then
        IP=$(dig +short "$DNS_DOMAIN" | head -n 1)
        echo -e "${GREEN}✓${NC} DNS resolves: $DNS_DOMAIN → $IP"
        DNS_STATUS="resolves"
    else
        echo -e "${YELLOW}⚠${NC}  DNS does not resolve: $DNS_DOMAIN"
        DNS_STATUS="no-resolve"
    fi
elif command -v nslookup &> /dev/null; then
    echo "Checking DNS resolution for $DNS_DOMAIN..."
    if nslookup "$DNS_DOMAIN" &> /dev/null; then
        IP=$(nslookup "$DNS_DOMAIN" | grep "Address:" | tail -n 1 | awk '{print $2}')
        echo -e "${GREEN}✓${NC} DNS resolves: $DNS_DOMAIN → $IP"
        DNS_STATUS="resolves"
    else
        echo -e "${YELLOW}⚠${NC}  DNS does not resolve: $DNS_DOMAIN"
        DNS_STATUS="no-resolve"
    fi
else
    echo -e "${YELLOW}⚠${NC}  Cannot check DNS (dig/nslookup not available)"
    DNS_STATUS="unknown"
fi

# Check /etc/hosts
if grep -q "$DNS_DOMAIN" /etc/hosts 2>/dev/null; then
    echo -e "${GREEN}✓${NC} $DNS_DOMAIN found in /etc/hosts"
    HOSTS_STATUS="found"
else
    echo -e "${YELLOW}⚠${NC}  $DNS_DOMAIN not in /etc/hosts"
    HOSTS_STATUS="not-found"
fi

echo ""
echo "═══════════════════════════════════════════════"
echo "Recommended setup based on your configuration:"
echo "═══════════════════════════════════════════════"
echo ""

# Provide recommendations
if [ "$TOKEN_STATUS" = "configured" ] && [ "$DNS_STATUS" = "resolves" ]; then
    echo -e "${GREEN}✓ Full DNS setup available${NC}"
    echo "  Run: make docker-dns"
    echo "  Access at: https://$DNS_DOMAIN"
    echo "  SSL: Let's Encrypt (real certificate)"
elif [ "$HOSTS_STATUS" = "found" ]; then
    echo -e "${YELLOW}✓ /etc/hosts configuration available${NC}"
    echo "  Run: make docker-localhost"
    echo "  Access at: https://$DNS_DOMAIN"
    echo "  SSL: Self-signed certificate (browser warning)"
else
    echo -e "${YELLOW}✓ Localhost-only setup${NC}"
    echo "  Run: make docker-localhost"
    echo "  Access at: https://localhost"
    echo "  SSL: Self-signed certificate (browser warning)"
    echo ""
    echo "To use $DNS_DOMAIN without external DNS:"
    echo "  sudo sh -c 'echo \"127.0.0.1 $DNS_DOMAIN\" >> /etc/hosts'"
    echo ""
    echo "To enable Let's Encrypt with DNS-01 challenge:"
    echo "  1. Ensure $DNS_DOMAIN points to your public IP"
    echo "  2. Set DIGITALOCEAN_TOKEN in .env file"
    echo "  3. Run: make docker-dns"
fi

echo ""
