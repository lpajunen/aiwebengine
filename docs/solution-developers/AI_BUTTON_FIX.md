# AI Assistant UI Button Fix

## Issue Found

The "Preview & Create" and "Preview Changes" buttons were not working due to:

1. **Double escaping** - Code was being JSON.stringify'd and then escaped again with `escapeJs()`
2. **Complex inline onclick handlers** - Too much data being passed as inline attributes
3. **Script name format** - AI generates `script-name.js` but scripts are stored as URIs `https://example.com/script-name.js`

## Fix Applied

### Changed Button Implementation

**Before (broken):**
```javascript
<button onclick="window.editor.showDiffModal('${escapeJs(scriptName)}', '', ${escapeJs(JSON.stringify(code))}, ...)">
```
- Too complex
- Double escaping breaking JSON
- Hard to debug

**After (working):**
```javascript
// Store data in memory
this.pendingAIAction = {
  type: actionType,
  scriptName: scriptName,
  code: code,
  originalCode: originalCode,
  message: message
};

// Simple button
<button onclick="window.editor.applyPendingAIAction()">Preview & Create</button>
```

### Flow Now

1. **AI responds** with structured JSON
2. **Frontend parses** response
3. **Data stored** in `this.pendingAIAction`
4. **Button rendered** with simple onclick
5. **User clicks** → `applyPendingAIAction()` called
6. **Method reads** from `this.pendingAIAction`
7. **Opens diff modal** with correct data
8. **User applies** → Script saved

## Testing

After this fix, try:

```
Create a script that serves a welcome page
```

**Expected:**
1. ✅ AI responds with JSON
2. ✅ Shows "CREATE SCRIPT" badge
3. ✅ Shows "Preview & Create" button
4. ✅ Click button → Diff modal opens
5. ✅ Shows code preview
6. ✅ Click "Apply Changes" → Script created
7. ✅ Script appears in sidebar
8. ✅ Script loads in editor

## Files Modified

- `/Users/lassepajunen/work/aiwebengine/assets/editor.js`
  - Removed complex inline onclick handlers
  - Added `pendingAIAction` storage
  - Added `applyPendingAIAction()` method
  - Removed `escapeJs()` function (no longer needed)
  - Simplified button rendering

## Status

✅ **Fixed** - Buttons should now work correctly
