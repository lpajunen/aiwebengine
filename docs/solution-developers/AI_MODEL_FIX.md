# Model Name Fix

## Issue

Error when calling Anthropic API:
```
AI Assistant: API error - Status: 404
AI Assistant: Error body: {"type":"error","error":{"type":"not_found_error","message":"model: claude-3-5-sonnet-20241022"}}
```

## Root Cause

The model name `claude-3-5-sonnet-20241022` doesn't exist. This was an incorrect model identifier.

## Fix

Changed model name to the correct Claude 3.5 Sonnet identifier:

**Before (incorrect):**
```javascript
model: "claude-3-5-sonnet-20241022"
```

**After (correct):**
```javascript
model: "claude-3-5-sonnet-20240620"
```

## Valid Anthropic Model Names (as of October 2024)

### Claude 3.5 (Latest)
- `claude-3-5-sonnet-20240620` ✅ - Most capable, best for complex tasks

### Claude 3 (Previous generation)
- `claude-3-opus-20240229` - Highest capability, most expensive
- `claude-3-sonnet-20240229` - Balanced performance and cost
- `claude-3-haiku-20240307` - Fastest, cheapest, good for simple tasks

## Current Configuration

The editor now uses:
```javascript
model: "claude-3-5-sonnet-20240620",
max_tokens: 8192
```

This provides:
- Excellent instruction following (crucial for JSON responses)
- High-quality code generation
- Reliable structured outputs
- Sufficient tokens for complex scripts

## Alternative Options

### For Cost Savings (Use Haiku)
```javascript
model: "claude-3-haiku-20240307",
max_tokens: 8192
```
- ~7.5x cheaper
- Still works with enhanced system prompt
- May be less reliable for JSON format

### For Maximum Capability (Use Opus)
```javascript
model: "claude-3-opus-20240229",
max_tokens: 8192
```
- Most capable model
- Best reasoning and code quality
- ~3x more expensive than Sonnet

## Testing

After the fix, restart the server and test:

```bash
cargo run
```

Then open the editor and try:
```
Create a hello world API
```

You should see in logs:
```
AI Assistant: Success - Model: claude-3-5-sonnet-20240620
AI Assistant: Successfully parsed structured response of type: create_script
```

## Status

✅ **Fixed** - Model name corrected to `claude-3-5-sonnet-20240620`
