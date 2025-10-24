# AI Assistant - Response Format Fixes

## Issues Found

1. **AI was not returning JSON** - Logs showed "Response is plain text, not JSON"
2. **Responses were truncated** - Responses ending too soon

## Root Causes

### Issue 1: Plain Text Instead of JSON

**Problem:**

- The AI was sometimes wrapping responses in markdown code blocks
- The AI was not strictly following the JSON-only instruction
- System prompt was not explicit enough about ONLY returning JSON

**Symptoms:**

```
AI Assistant: Response is plain text, not JSON
```

### Issue 2: Truncated Responses

**Problem:**

- `max_tokens: 4096` was sometimes insufficient for complex scripts
- Using Claude 3 Haiku which is fast but less capable
- No detection of truncation in responses

**Symptoms:**

- Incomplete code in responses
- JSON responses ending abruptly
- Missing closing braces

## Fixes Applied

### Fix 1: Enhanced System Prompt

**Changes:**

- Added explicit "CRITICAL" instruction at the start
- Multiple reminders to return ONLY JSON
- Clear examples of correct JSON responses
- Instructions to escape special characters
- Warning against markdown code blocks

**New System Prompt Header:**

```javascript
const systemPrompt = `You are an AI assistant for aiwebengine...

CRITICAL: You MUST respond with ONLY valid JSON. No markdown, no code blocks, no explanations outside the JSON.

...

IMPORTANT: 
- Do NOT wrap your response in markdown code blocks
- Do NOT add any text before or after the JSON
- Start your response with { and end with }
- Escape all special characters in strings (newlines as \\n, quotes as \\")

...

EXAMPLES OF CORRECT RESPONSES:

For explanation:
{"type":"explanation","message":"This script registers..."}

For create:
{"type":"create_script","message":"I'll create...","script_name":"hello.js","code":"function..."}

Remember: Response must be PURE JSON only, nothing else!
`;
```

### Fix 2: Markdown Cleanup

**Changes:**

- Added logic to detect and remove markdown code blocks
- Strips `\`\`\`json`and`\`\`\`` wrappers if present
- Logs when cleanup is performed

**New Parsing Code:**

````javascript
// Clean up response - remove markdown code blocks if present
let cleanedResponse = aiResponse.trim();

// Remove markdown code blocks (```json ... ``` or ``` ... ```)
if (cleanedResponse.startsWith("```")) {
  writeLog(`AI Assistant: Removing markdown code blocks`);
  // Remove opening ```json or ```
  cleanedResponse = cleanedResponse.replace(/^```(?:json)?\s*\n?/, "");
  // Remove closing ```
  cleanedResponse = cleanedResponse.replace(/\n?```\s*$/, "");
  cleanedResponse = cleanedResponse.trim();
}
````

### Fix 3: Model Upgrade

**Before:**

```javascript
model: "claude-3-haiku-20240307",  // Fast but less capable
max_tokens: 4096
```

**After:**

```javascript
model: "claude-3-5-sonnet-20240620",  // Much better at following instructions
max_tokens: 8192  // Doubled to prevent truncation
```

**Benefits:**

- Claude 3.5 Sonnet is MUCH better at following JSON format instructions
- Higher intelligence means more reliable structured responses
- 8192 tokens allows for longer, more complete scripts
- Better code generation quality overall

### Fix 4: Truncation Detection

**Changes:**

- Added `stop_reason` logging to detect truncation
- Warns when response is cut off due to token limits
- Returns stop_reason to frontend for potential UI warnings

**New Detection Code:**

```javascript
const stopReason = data.stop_reason || "unknown";
writeLog(`AI Assistant: Stop reason: ${stopReason}`);

if (stopReason === "max_tokens") {
  writeLog(
    `AI Assistant: WARNING - Response truncated due to max_tokens limit`,
  );
}
```

### Fix 5: Enhanced Logging

**Changes:**

- Logs response length in characters
- Logs first 100 chars of raw response
- Logs cleaned response after markdown removal
- Logs parsed response type when successful
- Logs detailed error when parsing fails

**New Logs:**

```javascript
writeLog(`AI Assistant: Raw response length: ${aiResponse.length} chars`);
writeLog(
  `AI Assistant: Raw response start: ${aiResponse.substring(0, 100)}...`,
);
writeLog(
  `AI Assistant: Successfully parsed structured response of type: ${parsedResponse.type}`,
);
```

## Testing the Fixes

### Test 1: Verify JSON Response

**Before:**

```
AI Assistant: Response is plain text, not JSON
```

**After:**

```
AI Assistant: Successfully parsed structured response of type: create_script
```

### Test 2: Verify Complete Responses

**Before:**

- Incomplete code
- Missing closing braces
- Truncated mid-function

**After:**

- Complete scripts with proper structure
- All closing braces present
- Full init() functions

### Test 3: Check Logs

**Look for these logs:**

```
AI Assistant: Raw response length: 2543 chars
AI Assistant: Stop reason: end_turn  (Good - natural completion)
AI Assistant: Successfully parsed structured response of type: create_script
```

**Warning signs to watch for:**

```
AI Assistant: Stop reason: max_tokens  (Response was cut off)
AI Assistant: Removing markdown code blocks  (AI didn't follow instructions)
AI Assistant: Response is plain text, not JSON  (Parsing failed)
```

## Expected Behavior Now

### For "Create Script" Request

**User asks:** "Create a hello world API"

**AI should respond with:**

```json
{
  "type": "create_script",
  "message": "I'll create a simple hello world API endpoint",
  "script_name": "hello.js",
  "code": "function handleHello(req) {\n  return {\n    status: 200,\n    body: 'Hello World!',\n    contentType: 'text/plain'\n  };\n}\n\nfunction init(context) {\n  register('/hello', 'handleHello', 'GET');\n  return { success: true };\n}"
}
```

**Logs should show:**

```
AI Assistant: Raw response length: 345 chars
AI Assistant: Stop reason: end_turn
AI Assistant: Successfully parsed structured response of type: create_script
```

### For "Explain Script" Request

**User asks:** "Explain this script"

**AI should respond with:**

```json
{
  "type": "explanation",
  "message": "This script registers a GET endpoint at /api/users that returns a list of users..."
}
```

**Logs should show:**

```
AI Assistant: Successfully parsed structured response of type: explanation
```

## Cost Considerations

**Claude 3 Haiku:**

- Input: $0.25 per million tokens
- Output: $1.25 per million tokens
- Fast, cheap, but less reliable for structured outputs

**Claude 3.5 Sonnet (new default):**

- Input: $3.00 per million tokens
- Output: $15.00 per million tokens
- Slower, more expensive, but MUCH more reliable

**Estimated cost per request:**

- Haiku: ~$0.002 per request (512 tokens output)
- Sonnet: ~$0.015 per request (512 tokens output)
- ~7.5x more expensive, but worth it for reliability

**If cost is a concern:**
You can switch back to Haiku, but the enhanced system prompt and cleanup logic should still help:

```javascript
model: "claude-3-haiku-20240307",
max_tokens: 8192,  // Keep the higher limit
```

## Rollback Instructions

If you need to revert to the old settings:

```javascript
// In scripts/feature_scripts/editor.js, change:

model: "claude-3-5-sonnet-20240620",  // Change back to:
model: "claude-3-haiku-20240307",

max_tokens: 8192,  // Change back to:
max_tokens: 4096,
```

But keep the enhanced system prompt and cleanup logic - they help with both models.

## Summary

✅ **Enhanced system prompt** - Multiple explicit JSON-only instructions  
✅ **Markdown cleanup** - Removes code block wrappers if present  
✅ **Model upgrade** - Claude 3.5 Sonnet for better instruction following  
✅ **Increased tokens** - 8192 max_tokens to prevent truncation  
✅ **Truncation detection** - Logs warnings when responses are cut off  
✅ **Better logging** - More detailed diagnostics for debugging

The AI should now consistently return valid JSON responses and complete code without truncation!
