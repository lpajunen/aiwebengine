#!/usr/bin/env bash
# Remove stale ACME DNS-01 challenge records from the DigitalOcean zone.
#
# Why this exists: caddy-dns/digitalocean cannot delete the records it writes.
# Its cleanup path recovers the DigitalOcean record ID with a type assertion,
#
#   if dns, ok := record.(DNS); ok { raw = dns.ID }
#   id, err := strconv.Atoi(raw)
#
# but certmagic hands cleanup a plain libdns.RR rather than the provider's own
# DNS wrapper, so the assertion fails, the ID stays empty and every cleanup ends
# as `strconv.Atoi: parsing "": invalid syntax`. That is upstream behaviour at
# the latest version (libdns/digitalocean 2025-06-06), not a misconfiguration
# here, so the records accumulate once per issuance attempt.
#
# Accumulation is what actually breaks issuance. DigitalOcean's nameservers
# answer with only a rotating subset of a large TXT RRset, and the value Caddy
# just wrote is not reliably in it — so Let's Encrypt cannot see the answer to
# the challenge it set, and the order never validates. Emptying the RRset before
# a run is what keeps the next challenge visible.
#
# Only `_acme-challenge*` TXT records are touched; every other record in the
# zone is left alone. Deleting a leftover challenge record is always safe: it is
# scratch data for an exchange that has already finished.

set -euo pipefail

DNS_DOMAIN="${DNS_DOMAIN:-local.softagen.com}"
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

if [ -z "${DIGITALOCEAN_TOKEN:-}" ]; then
    echo -e "${RED}✗${NC} DIGITALOCEAN_TOKEN not set"
    exit 1
fi

# The zone is the registrable domain, not the name being certified:
# local.softagen.com is a name inside the softagen.com zone.
ZONE="${ACME_DNS_ZONE:-$(echo "$DNS_DOMAIN" | awk -F. '{print $(NF-1)"."$NF}')}"

api() {
    curl -sS -H "Authorization: Bearer $DIGITALOCEAN_TOKEN" "$@"
}

echo "Clearing stale ACME challenge records in $ZONE..."

records=$(api "https://api.digitalocean.com/v2/domains/$ZONE/records?per_page=200") || {
    echo -e "${RED}✗${NC} could not list records for $ZONE"
    exit 1
}

ids=$(printf '%s' "$records" | python3 -c '
import sys, json
try:
    payload = json.load(sys.stdin)
except ValueError:
    sys.exit("unreadable response from the DigitalOcean API")
if "domain_records" not in payload:
    sys.exit(payload.get("message", "the DigitalOcean API refused the request"))
for record in payload["domain_records"]:
    if record["type"] == "TXT" and record["name"].startswith("_acme-challenge"):
        print(record["id"])
') || {
    echo -e "${RED}✗${NC} $ids"
    exit 1
}

if [ -z "$ids" ]; then
    echo -e "${GREEN}✓${NC} no stale challenge records"
    exit 0
fi

count=0
while read -r id; do
    [ -z "$id" ] && continue
    status=$(api -o /dev/null -w '%{http_code}' \
        -X DELETE "https://api.digitalocean.com/v2/domains/$ZONE/records/$id")
    if [ "$status" = "204" ]; then
        count=$((count + 1))
    else
        echo -e "${YELLOW}⚠${NC}  record $id not deleted (HTTP $status)"
    fi
done <<< "$ids"

echo -e "${GREEN}✓${NC} removed $count stale challenge record(s)"
