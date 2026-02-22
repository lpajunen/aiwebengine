# Editor Development Ideas

This document collects ideas for improving the aiwebengine editor, with a focus on leveraging AI to create, edit, and test scripts.

## AI-Assisted Script Development

### Multi-Script Context Options

Currently, the AI assistant only includes the current script in its context when providing answers. This limits the AI's ability to help with scenarios where scripts interact with each other (e.g., one script provides a GraphQL API and another consumes it).

#### Option 1: Script Selection UI (Recommended)

**Description:** Add a UI component that allows users to explicitly select additional scripts to include in the AI context.

**Advantages:**

- User has full control over what context is included
- Reduces token usage by only including relevant scripts
- Clear and explicit - users understand what the AI "sees"

**Implementation:**

- Add a multi-select dropdown in the AI assistant panel
- Display selected scripts as chips/tags
- Include selected scripts' content in the API request
- Remember selected scripts for the current session

**UI Mockup:**

```
AI Assistant
├─ Additional Scripts: [▼]
│  ├─ ☑ core.js
│  ├─ ☐ graphql-api.js
│  └─ ☐ auth-helpers.js
└─ Token Budget: 2,450 / 8,000 tokens used
```

#### Option 2: Automatic Dependency Detection

**Description:** Analyze the current script for references to other scripts (e.g., GraphQL endpoints, API calls) and automatically include them.

**Advantages:**

- Seamless user experience - no extra steps
- Smart context inclusion based on actual dependencies

**Disadvantages:**

- More complex to implement
- May include unnecessary scripts
- Pattern matching could be unreliable

**Detection Patterns:**

- `executeGraphQL()` calls → include GraphQL schema registration scripts
- `fetch('/api/users')` calls → include scripts that register that route
- `registerWebStream()` references → include related stream handlers

#### Option 3: "Include All Scripts" Toggle

**Description:** Add a simple checkbox to include all scripts in the workspace.

**Advantages:**

- Simplest implementation
- Comprehensive context

**Disadvantages:**

- High token usage with many scripts
- May hit API limits
- Irrelevant context could reduce AI quality

#### Option 4: Script Reference Syntax

**Description:** Allow users to reference other scripts in their prompt using a special syntax.

**Example:** "Using the API defined in `@core.js`, create a client script..."

**Advantages:**

- Natural in the prompt
- Explicit control per request
- No UI changes needed

**Disadvantages:**

- Requires users to remember syntax
- Manual specification each time

**Implementation:**

```javascript
// Parse prompt for @script-name.js references
const scriptRefs = prompt.match(/@([\w-]+\.js)/g);
// Load and include referenced scripts
```

#### Option 5: Context Window with Smart Truncation

**Description:** Include recent/related scripts with intelligent truncation.

**Advantages:**

- Automatic context management
- Handles large codebases

**Disadvantages:**

- Complex heuristics needed
- May miss important context

**Heuristics:**

- Prioritize by: recency, size, relevance score
- Truncate long scripts to function signatures only
- Include full content only for most relevant scripts

### Automatic Context Gathering (Copilot-Style)

Implement techniques similar to GitHub Copilot for intelligent context gathering:

#### Phase 1: Keyword + Pattern Matching (Quick Win)

**Implementation Steps:**

1. Pre-process user prompt to extract key terms
2. Search script names and content for keyword matches
3. Detect common patterns:
   - GraphQL queries/mutations/subscriptions
   - API endpoint paths
   - Function names
   - Variable references
4. Score each script by relevance
5. Include top 3-5 most relevant scripts automatically

**Example Patterns:**

```javascript
// Detect GraphQL operations
const graphqlPatterns = [
  /registerGraphQL(Query|Mutation|Subscription)\s*\(/g,
  /executeGraphQL\s*\(/g,
];

// Detect route registrations
const routePatterns = [
  /register\s*\(\s*['"]([^'"]+)['"]/g,
  /fetch\s*\(\s*['"]([^'"]+)['"]/g,
];
```

**UI Indicator:**

```
AI Assistant
└─ Auto-included: core.js, graphql-schema.js, auth.js [?]
   (click to see why these were included)
```

#### Phase 2: Reference Graph

**Description:** Build a dependency graph showing which scripts call which endpoints.

**Graph Structure:**

```javascript
{
  "core.js": {
    "provides": ["/api/users", "/api/posts"],
    "consumes": [],
    "graphql": ["getUser", "createPost"]
  },
  "client.js": {
    "provides": ["/app/dashboard"],
    "consumes": ["/api/users"],
    "graphql": []
  }
}
```

**Benefits:**

- Auto-include provider scripts when user asks about consumers
- Understand script relationships
- Suggest related scripts when creating new ones

**Implementation:**

1. Parse all scripts on load/save
2. Extract route registrations and API calls
3. Build and cache dependency map
4. Use for context selection

#### Phase 3: Semantic Search with Embeddings

**Description:** True semantic similarity search using embeddings.

**Approaches:**

**A) Anthropic Embeddings:**

- Use Claude's native embedding capabilities (if available)
- Pre-compute embeddings for all scripts
- Store in browser localStorage or backend cache
- Search by cosine similarity

**B) Lightweight Browser-Based:**

- Use TensorFlow.js with Universal Sentence Encoder
- Client-side embedding computation
- No external API calls needed
- ~50MB model size

**C) Backend Embedding Service:**

- Add embedding endpoint to aiwebengine
- Use sentence-transformers or similar
- Store embeddings in PostgreSQL with pgvector extension

**Comparison:**

| Approach | Pros | Cons |
|----------|------|------|
| Anthropic | No extra infrastructure | API costs, latency |
| Browser | No API costs, instant | Large download, less accurate |
| Backend | Best accuracy, shared cache | Infrastructure complexity |

**Implementation Example:**

```javascript
// 1. Pre-compute and cache embeddings
async function indexScripts() {
  const scripts = scriptStorage.listScripts();
  for (const script of scripts) {
    const content = scriptStorage.getScript(script);
    const embedding = await computeEmbedding(content);
    embeddings[script] = embedding;
  }
}

// 2. Search by similarity
async function findRelevantScripts(query, topK = 5) {
  const queryEmbedding = await computeEmbedding(query);
  const scores = Object.entries(embeddings).map(([script, emb]) => ({
    script,
    similarity: cosineSimilarity(queryEmbedding, emb),
  }));
  return scores.sort((a, b) => b.similarity - a.similarity).slice(0, topK);
}
```

### AI Testing Capabilities

#### Automated Test Generation

**Description:** AI generates test cases for scripts.

**User Flow:**

1. Select a script in editor
2. Click "Generate Tests" button
3. AI analyzes script and creates test scenarios
4. User reviews and runs tests

**Test Types:**

- Unit tests for handler functions
- Integration tests for API endpoints
- Edge case testing
- Security testing (XSS, injection, etc.)

**Example Output:**

```javascript
// Tests for user-api.js
const tests = [
  {
    name: "GET /api/users returns user list",
    method: "GET",
    path: "/api/users",
    expectedStatus: 200,
    expectedContentType: "application/json",
  },
  {
    name: "GET /api/users handles empty database",
    method: "GET",
    path: "/api/users",
    setup: "clearDatabase()",
    expectedBody: "[]",
  },
];
```

#### Interactive Testing UI

**Description:** Built-in request testing with AI assistance.

**Features:**

- Send HTTP requests directly from editor
- AI suggests test inputs based on script analysis
- View response with syntax highlighting
- Save test cases for later
- Compare responses across script versions

**UI Layout:**

```
┌─ Test Panel ───────────────┐
│ Request                     │
│ [POST ▼] /api/users        │
│ Headers: [+]                │
│ Body:                       │
│ { "name": "Alice" }        │
│ [Send] [AI Suggest]        │
├─────────────────────────────┤
│ Response                    │
│ Status: 200 OK              │
│ { "id": 1, "name": "Alice" }│
│ [Save Test] [Compare]      │
└─────────────────────────────┘
```

#### AI-Powered Debugging

**Description:** AI helps diagnose script errors.

**Triggers:**

- User reports an error
- Exception caught in logs
- Failed test case

**AI Analysis:**

1. Analyze error message and stack trace
2. Review script code for common issues
3. Check related scripts for breaking changes
4. Suggest fixes with explanations

**Example Interaction:**

```
User: "My GraphQL query is returning undefined"

AI: I found the issue in your script:
1. Line 45: You're calling `executeGraphQL()` without await
2. The function is async but you're not handling the Promise
3. Suggested fix: Add `await` or use `.then()`

[Show Diff] [Apply Fix]
```

### AI Code Review

#### Pre-Save Analysis

**Description:** AI reviews code before saving.

**Checks:**

- Syntax errors
- Common security issues
- Performance anti-patterns
- Best practices for aiwebengine
- Missing error handling
- Incomplete init() functions

**UI:**

```
┌─ Save Script ──────────────┐
│ ⚠ AI Review Found Issues:  │
│                             │
│ ⚠ Security                  │
│   • SQL injection risk at   │
│     line 23 (user input)    │
│                             │
│ ⚠ Best Practices           │
│   • Missing try-catch in    │
│     handler function        │
│                             │
│ [Fix All] [Ignore] [Cancel]│
└─────────────────────────────┘
```

#### Collaboration Features

**Description:** AI-assisted code review for teams.

**Features:**

- Diff viewer with AI annotations
- "Explain this change" button
- Suggest improvements to submitted code
- Check for breaking changes in API contracts

### Smart Code Completion

#### Context-Aware Suggestions

**Description:** AI suggests completions based on current context.

**Examples:**

**Scenario 1: Starting a new handler**

```javascript
function handleUserCreate(req) {
  // AI suggests: Let me help you create a user handler
  // [Generate boilerplate] [See examples]
}
```

**Scenario 2: Using available APIs**

```javascript
function init(context) {
  // User types: "regi"
  // AI suggests:
  //   - register(path, handler, method)
  //   - registerGraphQLQuery(...)
  //   - registerWebStream(path)
}
```

**Scenario 3: Common patterns**

```javascript
// User types: "fetch"
// AI suggests complete fetch pattern:
const response = fetch(
  "https://api.example.com",
  JSON.stringify({
    method: "GET",
    headers: {
      Authorization: "{{secret:api_key}}",
    },
  }),
);
const data = JSON.parse(JSON.parse(response).body);
```

### Script Templates and Scaffolding

#### AI-Generated Templates

**Description:** AI creates full script templates based on description.

**User Flow:**

1. Click "New Script"
2. Describe what you want: "REST API for managing tasks"
3. AI generates complete script with:
   - All CRUD operations
   - Error handling
   - Input validation
   - Proper init() function
   - Comments and documentation

**Template Categories:**

- REST APIs (CRUD)
- GraphQL schemas
- Web pages (HTML)
- File upload handlers
- WebSocket/SSE streams
- Authentication helpers
- Data transformation scripts

#### Smart Refactoring

**Description:** AI helps refactor and improve existing scripts.

**Operations:**

- Extract function
- Split into multiple scripts
- Add error handling
- Add logging
- Convert to async/await
- Add TypeScript-style JSDoc comments

**Example:**

```
User: "Extract the validation logic into a separate function"

AI: I'll refactor your script:
- Created validateUserInput() function
- Added JSDoc comments
- Improved error messages

[Preview Changes] [Apply]
```

### Multi-File Operations with Single Prompt

**Description:** Allow AI to create, modify, or delete multiple scripts in response to a single prompt. This is a key feature in modern AI coding assistants like Cursor, GitHub Copilot, and Aider.

**Why This Matters:**

- **Real-world workflows**: Most features span multiple files (API + client + tests)
- **Architectural changes**: Refactoring often requires coordinated changes across files
- **Consistency**: AI can ensure consistent patterns across all related files
- **Efficiency**: User describes intent once, AI handles all implementations

**Use Cases:**

1. **Feature Implementation:**
   - "Create a blog system with posts API, comments API, and frontend"
   - AI creates: `posts-api.js`, `comments-api.js`, `blog-frontend.js`

2. **Refactoring:**
   - "Extract authentication logic from core.js into a separate auth.js"
   - AI modifies: `core.js` (removes auth code), creates: `auth.js` (with extracted code)

3. **API + Client Pattern:**
   - "Create a task management API and a client script that uses it"
   - AI creates: `tasks-api.js` (GraphQL schema), `tasks-client.js` (queries/mutations)

4. **Migration:**
   - "Convert all REST APIs to GraphQL"
   - AI modifies: `users-api.js`, `posts-api.js`, `comments-api.js`, etc.

5. **Testing:**
   - "Add integration tests for all my API endpoints"
   - AI creates: `users-api.test.js`, `posts-api.test.js`, etc.

#### Implementation Approaches

**Approach 1: Sequential Operations (Simplest)**

**How it works:**

1. AI response includes array of file operations
2. Editor executes them sequentially
3. Shows progress indicator
4. Allows cancellation mid-process

**Response Format:**

```json
{
  "type": "multi_file_operation",
  "message": "Creating blog system with 3 scripts",
  "operations": [
    {
      "type": "create_script",
      "script_name": "posts-api.js",
      "code": "// posts API code..."
    },
    {
      "type": "create_script",
      "script_name": "comments-api.js",
      "code": "// comments API code..."
    },
    {
      "type": "edit_script",
      "script_name": "core.js",
      "original_code": "// old code...",
      "code": "// new code..."
    }
  ]
}
```

**UI Flow:**

```
AI Assistant Response:
┌────────────────────────────────────┐
│ Creating blog system with 3 files  │
│                                    │
│ ✓ posts-api.js (created)           │
│ ⟳ comments-api.js (creating...)    │
│ ○ blog-frontend.js (pending)       │
│                                    │
│ [Cancel] [Preview All] [Apply All] │
└────────────────────────────────────┘
```

**Advantages:**

- Simple to implement
- Clear progress tracking
- Easy rollback (haven't saved yet)

**Disadvantages:**

- No preview of all changes at once
- Sequential execution (slower)

**Approach 2: Batch Preview with Multi-Diff (Recommended)**

**How it works:**

1. AI returns all planned operations
2. Editor shows unified preview of ALL changes
3. User can review, accept/reject individual files
4. Batch apply or selective apply

**UI Design:**

```
┌─ Multi-File Changes ─────────────────────────────────────┐
│ AI Suggestion: Create blog system with API and frontend  │
│                                                           │
│ Files to be affected:                                    │
│ ☑ posts-api.js (new)                                     │
│ ☑ comments-api.js (new)                                  │
│ ☑ blog-frontend.js (new)                                 │
│ ☑ core.js (modified - 23 lines added, 5 removed)        │
│                                                           │
│ [posts-api.js]──────────────────────────────────────────│
│ + function handleGetPosts(req) {                         │
│ +   const posts = sharedStorage.getItem('posts');       │
│ +   return { status: 200, body: posts || '[]' };        │
│ + }                                                       │
│ [Show Full Diff]                                         │
│                                                           │
│ [comments-api.js]───────────────────────────────────────│
│ + function handleGetComments(req) {                      │
│ +   // ... comments handler                              │
│ [Show Full Diff]                                         │
│                                                           │
│ [Reject All] [Apply Selected] [Apply All]                │
└───────────────────────────────────────────────────────────┘
```

**Features:**

- Checkbox per file (selective apply)
- Expandable diff per file
- Overview of all changes
- Token usage summary
- Rollback capability

**Advantages:**

- Complete visibility before committing
- Selective application of changes
- Better UX for complex operations
- Matches VSCode/IDE patterns

**Disadvantages:**

- More complex UI
- Requires multi-file diff viewer

**Approach 3: Transaction-Based with Rollback**

**How it works:**

1. Apply all changes in a "transaction"
2. Keep backup of original state
3. Allow one-click rollback
4. Auto-commit after user confirmation

**Features:**

- Atomic operations (all succeed or all fail)
- Easy undo/redo
- Transaction history
- "Try it out" mode before committing

**Implementation:**

```javascript
class FileTransaction {
  constructor() {
    this.operations = [];
    this.backups = new Map();
  }

  addOperation(type, scriptName, content) {
    // Backup original if editing
    if (type === "edit" || type === "delete") {
      this.backups.set(scriptName, scriptStorage.getScript(scriptName));
    }
    this.operations.push({ type, scriptName, content });
  }

  async apply() {
    for (const op of this.operations) {
      await executeOperation(op);
    }
  }

  async rollback() {
    // Restore all backed up files
    for (const [scriptName, content] of this.backups) {
      upsertScript(scriptName, content);
    }
    // Delete any newly created files
    for (const op of this.operations) {
      if (op.type === "create") {
        deleteScript(op.scriptName);
      }
    }
  }
}
```

**Advantages:**

- Safe experimentation
- Easy to undo mistakes
- Professional developer experience

**Disadvantages:**

- Most complex implementation
- Need state management

#### AI Response Protocol

**Extended JSON Schema:**

```json
{
  "type": "multi_file_operation",
  "message": "Human-readable description of overall change",
  "summary": {
    "files_created": 2,
    "files_modified": 1,
    "files_deleted": 0,
    "total_lines_added": 145,
    "total_lines_removed": 23
  },
  "operations": [
    {
      "type": "create_script",
      "script_name": "posts-api.js",
      "explanation": "GraphQL API for managing blog posts",
      "code": "// full script content...",
      "dependencies": ["core.js"]
    },
    {
      "type": "edit_script",
      "script_name": "core.js",
      "explanation": "Register posts API routes",
      "original_code": "// old init function...",
      "code": "// new init function...",
      "diff_summary": "+3 lines, -1 line"
    },
    {
      "type": "delete_script",
      "script_name": "old-posts.js",
      "explanation": "Replaced by new posts-api.js"
    }
  ],
  "testing_instructions": "Test with: GET /api/posts",
  "breaking_changes": []
}
```

#### System Prompt Updates

**Add to AI system prompt:**

```
MULTI-FILE OPERATIONS:
When a user's request requires multiple files, you can respond with multiple operations:

Response format:
{
  "type": "multi_file_operation",
  "message": "Description of what you're doing",
  "operations": [
    { "type": "create_script", "script_name": "...", "code": "..." },
    { "type": "edit_script", "script_name": "...", "code": "...", "original_code": "..." },
    { "type": "delete_script", "script_name": "..." }
  ]
}

WHEN TO USE MULTI-FILE:
- User asks to "create a system/feature" (implies multiple components)
- Refactoring that spans files
- Creating API + client pairs
- Adding tests for existing code
- Architectural changes

BEST PRACTICES:
1. Keep files focused (single responsibility)
2. Extract common logic to shared utilities
3. Create tests alongside features
4. Update existing files when needed (don't duplicate)
5. Explain what each file does
6. Note any breaking changes
```

#### User Experience Considerations

**Clear Communication:**

- Show file count before executing
- Progress indicators for each file
- Success/failure status per file
- Rollback button always visible

**Safety Measures:**

- Confirm before modifying/deleting existing files
- Show diff for edits (never blind overwrite)
- Warn about breaking changes
- Backup automatically

**Error Handling:**

- If one file fails, what happens to others?
  - Option A: Stop entire operation
  - Option B: Continue, mark failed files
  - Option C: User chooses strategy

**Examples of Multi-File Prompts:**

1. "Create a complete task management system"
2. "Refactor my API scripts to use shared authentication"
3. "Add GraphQL subscriptions to all my APIs"
4. "Create integration tests for my existing scripts"
5. "Split core.js into separate feature modules"
6. "Migrate from REST to GraphQL across all endpoints"

#### Technical Challenges

**Challenge 1: Order Dependencies**

- Some files must be created before others
- Solution: AI specifies dependency order, or editor analyzes imports

**Challenge 2: Conflict Resolution**

- User edits a file while AI operation is pending
- Solution: Detect conflicts, show three-way merge UI

**Challenge 3: Partial Failures**

- One file saves successfully, another fails
- Solution: Transaction-based approach with rollback

**Challenge 4: Token Limits**

- Multiple large files exceed context window
- Solution: Summarize files, show only relevant sections, or split into multiple AI calls

**Challenge 5: Testing Multi-File Changes**

- Need to test all files together
- Solution: Integrated test runner, automatic endpoint testing

#### Recommended Implementation Path

**Phase 1: Basic Multi-File (MVP)**

1. Support `multi_file_operation` response type
2. Sequential execution with progress
3. Simple success/fail indicators
4. Manual rollback (user deletes/reverts files)

**Phase 2: Preview & Selective Apply**

1. Multi-file diff viewer
2. Checkbox per file
3. Preview before apply
4. Selective application

**Phase 3: Transactions & Safety**

1. Automatic backups
2. One-click rollback
3. Transaction history
4. Conflict detection

**Phase 4: Advanced Features**

1. Dependency ordering
2. Integrated testing
3. Breaking change detection
4. Automated migration tools

## Asset Editing (CSS, SVG, Images)

### Problem Statement

The current editor focuses on JavaScript scripts but doesn't provide adequate support for editing assets like CSS files and SVG images. These assets are critical for building complete web applications but currently require:

- Download → External editor → Re-upload workflow
- No syntax highlighting or validation
- No live preview
- No AI assistance for styling/graphics

### Option 1: In-Editor Asset Editing (Recommended)

**Description:** Redesign the Assets tab to mirror the Scripts tab layout, providing a unified editing experience for text-based assets with Monaco Editor.

**UI Layout (Similar to Scripts Tab):**

```
┌─────────────────────────────────────────────────┐
│ Assets Tab                                      │
├──────────────────┬──────────────────────────────┤
│ Assets           │                              │
│ [+ New] [Upload] │ Current File: editor.css     │
│                  │ [Save] [Delete]              │
├──────────────────┼──────────────────────────────┤
│ 📁 css/          │                              │
│   📄 editor.css  │  /* Monaco Editor */         │
│   📄 engine.css  │  .button {                   │
│ 📁 images/       │    background: #007acc;      │
│   🖼️ logo.png    │    padding: 10px;            │
│   🖼️ icon.svg    │  }                           │
│ 📁 fonts/        │                              │
│   📄 inter.woff2 │                              │
│                  │                              │
└──────────────────┴──────────────────────────────┘
```

**Features:**

**Asset Sidebar:**

- List of all assets organized by type/folder
- Icons indicating file type (text vs binary)
- **[+ New]** button - Create new text-based asset
- **[Upload]** button - Upload files (text or binary)
- Click to select and edit

**Asset Editor Panel:**

- Monaco Editor for text-based formats
- Auto-detect language by extension:
  - `.css` → CSS
  - `.svg` → XML
  - `.json` → JSON
  - `.html` → HTML
  - `.md` → Markdown
  - `.txt` → Plain text
  - `.js` → JavaScript
  - `.xml` → XML
- **[Save]** button - Save changes (enabled when modified)
- **[Delete]** button - Delete asset (with confirmation)
- Binary format indicator - Show message: "Binary file - cannot be edited in text editor"

**File Type Detection:**

```javascript
// Detect if asset is text or binary
function isTextAsset(path) {
  const textExtensions = [
    ".css",
    ".svg",
    ".json",
    ".html",
    ".md",
    ".txt",
    ".js",
    ".xml",
    ".csv",
    ".yaml",
    ".yml",
    ".toml",
  ];
  const ext = path.substring(path.lastIndexOf("."));
  return textExtensions.includes(ext);
}

// Get Monaco language mode
function getLanguageMode(path) {
  const ext = path.substring(path.lastIndexOf("."));
  const languageMap = {
    ".css": "css",
    ".svg": "xml",
    ".json": "json",
    ".html": "html",
    ".md": "markdown",
    ".txt": "plaintext",
    ".js": "javascript",
    ".xml": "xml",
    ".yaml": "yaml",
    ".yml": "yaml",
  };
  return languageMap[ext] || "plaintext";
}

// In loadAsset() method
if (isTextAsset(assetPath)) {
  // Load in Monaco editor
  const content = assetStorage.fetchAsset(assetPath);
  const language = getLanguageMode(assetPath);
  this.monacoEditor.setValue(content);
  monaco.editor.setModelLanguage(this.monacoEditor.getModel(), language);
  this.showEditor();
} else {
  // Binary file - show info panel instead
  this.hideEditor();
  this.showBinaryInfo(assetPath);
}
```

**Binary Asset Handling:**

When a binary asset (image, font, etc.) is selected:

- Hide Monaco editor
- Show info panel with:
  - File name and path
  - File size
  - MIME type
  - Preview thumbnail (for images)
  - Message: "This is a binary file and cannot be edited as text"
  - **[Download]** button
  - **[Replace]** button (upload new version)
  - **[Delete]** button

**New Asset Creation:**

Clicking **[+ New]** opens dialog:

```
┌─────────────────────────────────────────┐
│ Create New Asset                        │
├─────────────────────────────────────────┤
│ File Name: [styles/custom.css        ] │
│                                         │
│ Type:  [CSS ▼]                          │
│   - CSS                                 │
│   - SVG                                 │
│   - JSON                                │
│   - HTML                                │
│   - Markdown                            │
│   - Plain Text                          │
│                                         │
│ [Cancel]                     [Create]   │
└─────────────────────────────────────────┘
```

**Upload Asset:**

Clicking **[Upload]** opens file picker:

- Supports both text and binary files
- Multiple file selection enabled
- Shows upload progress
- Auto-detects MIME type
- Confirms before overwriting existing files

**Advantages:**

- **Consistent UX** - Same workflow as Scripts tab
- **Simple implementation** - Monaco already supports these languages
- **No new dependencies** - Uses existing editor infrastructure
- **Clear distinction** - Text vs binary files handled appropriately
- **Immediate value** - Edit CSS, SVG, JSON, etc. inline
- **Familiar patterns** - Users already know how to use Scripts tab

**Disadvantages:**

- No visual editing for SVG (could be Phase 2)
- No live preview initially (could be Phase 2)
- Binary files still require download/upload cycle

### Option 2: Split-Pane Preview System

**Description:** Dual-pane editor with code on left and live preview on right.

**CSS Preview:**

```
┌─────────────────┬─────────────────┐
│ /* editor.css */│ [Preview Pane]  │
│                 │                 │
│ .button {       │  ┌───────────┐  │
│   background:   │  │  Button   │  │
│   #007acc;      │  └───────────┘  │
│   padding: 10px;│                 │
│ }               │  Sample HTML    │
│                 │  with CSS       │
│                 │  applied        │
└─────────────────┴─────────────────┘
```

**SVG Preview:**

```
┌─────────────────┬─────────────────┐
│ <svg>...</svg>  │  [SVG Render]   │
│                 │                 │
│ <circle         │      ⚫         │
│   cx="50"       │                 │
│   cy="50"       │  + Zoom controls│
│   r="40"/>      │  + Pan          │
│                 │  + Download     │
└─────────────────┴─────────────────┘
```

**Implementation:**

- Add `.split-view` CSS class to editor container
- Create preview iframe for CSS (inject styles)
- Create SVG renderer with zoom/pan controls
- Hot reload on content change (debounced)

**Advantages:**

- Immediate visual feedback
- Better for designers/non-coders
- Catch rendering issues quickly

**Disadvantages:**

- More complex UI
- Performance concerns with live updates
- Need sample HTML for CSS preview

### Option 3: Visual SVG Editor Integration

**Description:** Embed visual SVG editor like SVG-edit or Boxy SVG.

**Tools to Consider:**

- **SVG-edit** (Open source, embeddable)
- **Method Draw** (Simplified SVG-edit fork)
- **Custom canvas-based editor**

**Features:**

- Drag and drop shapes
- Visual manipulation (resize, rotate, color)
- Toggle between visual and code view
- Export optimized SVG

**UI Flow:**

```
[Code View] [Visual View] [Split View]
     ↑           ↑             ↑
     └───────────┴─────────────┘
              Toggles
```

**Advantages:**

- Non-developers can create/edit SVG
- Professional graphics workflow
- No need to know SVG syntax

**Disadvantages:**

- Large dependency (~500KB+)
- More complex integration
- Learning curve for visual tool

### Option 4: AI-Assisted Asset Editing (Strategic)

**Description:** Extend existing AI assistant to help with CSS and SVG modifications.

**Use Cases:**

**CSS:**

- "Make the buttons larger and blue"
- "Add a dark mode variant"
- "Center this layout vertically"
- "Extract colors to CSS variables"
- "Add hover animations"

**SVG:**

- "Add a drop shadow to this icon"
- "Change all blue elements to red"
- "Simplify this path"
- "Generate a loading spinner"
- "Add gradient background"

**System Prompt Extension:**

```
ASSET EDITING:
When editing CSS or SVG files:
- Understand visual intent from natural language
- Preserve existing structure where possible
- Use modern CSS features (flexbox, grid, variables)
- Optimize SVG paths and remove unnecessary attributes
- Suggest accessibility improvements
```

**Response Format:**

```json
{
  "type": "edit_asset",
  "asset_path": "/styles/main.css",
  "explanation": "Added dark mode with CSS variables",
  "original_code": "/* old CSS */",
  "code": "/* new CSS with dark mode */",
  "preview_url": "/api/assets/styles/main.css?preview=true"
}
```

**Advantages:**

- Leverages existing AI infrastructure
- Natural language interface for styling
- Lowers barrier for non-CSS experts
- Unique differentiator vs other editors

**Disadvantages:**

- AI must understand CSS/SVG semantics
- Token usage increases
- May need examples/training

### Option 5: Asset Type Registry System

**Description:** Plugin-like architecture for different asset types.

**Registry Definition:**

```javascript
const ASSET_EDITORS = {
  ".css": {
    editor: "monaco",
    language: "css",
    preview: {
      type: "iframe",
      template: "css-preview.html",
    },
    aiContext: "CSS styling for web components",
    toolbar: ["format", "minify", "extract-vars"],
  },

  ".svg": {
    editor: "monaco",
    language: "xml",
    preview: {
      type: "render",
      component: "SVGPreview",
    },
    visualEditor: "svg-edit",
    aiContext: "SVG graphics and icons",
    toolbar: ["optimize", "visual-edit", "export-png"],
  },

  ".json": {
    editor: "monaco",
    language: "json",
    preview: {
      type: "tree",
      component: "JSONTree",
    },
    aiContext: "JSON configuration and data",
    toolbar: ["format", "validate", "schema"],
  },
};
```

**Benefits:**

- Extensible for new asset types
- Standardized interface
- Easy to add new editors
- Configurable per asset type

**Implementation:**

```javascript
loadAsset(assetPath) {
  const ext = this.getFileExtension(assetPath);
  const config = ASSET_EDITORS[ext] || ASSET_EDITORS['.txt'];

  this.setupEditor(config.editor, config.language);
  this.setupPreview(config.preview);
  this.setupToolbar(config.toolbar);
  this.setupAIContext(config.aiContext);
}
```

### Option 6: External Tool Integration

**Description:** Integrate with external specialized tools for complex work.

**Integration Points:**

**CSS:**

- "Open in CodePen" for complex experiments
- Link to Tailwind Play for utility classes
- CSS Grid/Flexbox visual builders

**SVG:**

- Export to Figma/Sketch
- Import from design tools
- Link to SVG optimization tools (SVGO)

**Workflow:**

1. Export asset from aiwebengine
2. Edit in specialized tool
3. Import back (with git-style diff)
4. AI reviews changes

**Advantages:**

- Leverage best-in-class tools
- Don't reinvent the wheel
- Professional workflows

**Disadvantages:**

- Context switching
- Import/export friction
- Requires external accounts

### Recommended Implementation Path

**Phase 1: Foundation (Quick Win)**

1. Add Monaco language detection for assets (.css, .svg, .json)
2. Proper syntax highlighting
3. Save assets through existing API
4. Update sidebar to show both scripts and editable assets

**Estimated Effort:** 2-4 hours

**Phase 2: Live Preview**

1. Add split-pane layout toggle
2. CSS preview with sample component library
3. SVG preview with zoom/pan controls
4. Hot reload on content change

**Estimated Effort:** 1-2 days

**Phase 3: AI Integration (Strategic)**

1. Extend AI assistant context to include current asset
2. Add asset-type specific prompts
3. Support diff preview for asset changes
4. Natural language styling commands

**Estimated Effort:** 2-3 days

**Phase 4: Advanced Features**

1. Visual SVG editor toggle
2. CSS variable extraction tool
3. Asset optimization (minify, compress)
4. Accessibility checking

**Estimated Effort:** 1 week

### Technical Considerations

**Monaco Language Support:**

Monaco includes built-in support for:

- CSS (with IntelliSense)
- SCSS/LESS
- HTML
- XML (for SVG)
- JSON

**Preview Rendering:**

```javascript
// CSS Preview
function renderCSSPreview(cssContent) {
  const iframe = document.getElementById("preview-iframe");
  const doc = iframe.contentDocument;

  doc.body.innerHTML = `
    <style>${cssContent}</style>
    <div class="preview-components">
      <button class="btn btn-primary">Button</button>
      <input class="form-input" placeholder="Input">
      <!-- More components -->
    </div>
  `;
}

// SVG Preview
function renderSVGPreview(svgContent) {
  const container = document.getElementById("svg-preview");
  container.innerHTML = svgContent;

  // Add pan/zoom with panzoom.js
  panzoom(container.querySelector("svg"));
}
```

**Asset API Extensions:**

May need to add:

- `GET /api/assets/:path/metadata` - Get asset info
- `POST /api/assets/:path/optimize` - Optimize CSS/SVG
- `POST /api/assets/:path/preview` - Generate preview

### AI Prompt Examples

**CSS Examples:**

```
User: "Make this button bigger and add hover effect"
AI Response:
{
  "type": "edit_asset",
  "asset_path": "/styles/buttons.css",
  "explanation": "Increased button size and added smooth hover transition",
  "code": ".btn { padding: 12px 24px; transition: all 0.3s; }\n.btn:hover { transform: scale(1.05); }"
}

User: "Convert this to use CSS variables for theming"
AI: Extracts colors/sizes to :root variables

User: "Add dark mode support"
AI: Creates @media (prefers-color-scheme: dark) rules
```

**SVG Examples:**

```
User: "Change the icon color to red"
AI: Modifies fill/stroke attributes

User: "Add a drop shadow"
AI: Adds <filter> with feDropShadow

User: "Optimize this SVG"
AI: Removes unnecessary attributes, simplifies paths

User: "Make this icon responsive"
AI: Adds viewBox, removes fixed width/height
```

### UI Mockups

**Asset Editor with Preview:**

```
┌─────────────────────────────────────────────────┐
│ Editor: editor.css                    [⚙ Tools]│
├──────────────────┬──────────────────────────────┤
│                  │                              │
│ /* Main Styles */│    [Live Preview]            │
│                  │                              │
│ :root {          │    ┌──────────┐             │
│   --primary:     │    │  Button  │ ← Hover me  │
│   #007acc;       │    └──────────┘             │
│ }                │                              │
│                  │    Lorem ipsum dolor sit     │
│ .btn {           │    consectetur adipiscing    │
│   background:    │                              │
│   var(--primary);│    [Components: v]           │
│   padding: 10px; │    [x] Buttons               │
│ }                │    [x] Forms                 │
│                  │    [ ] Cards                 │
│                  │                              │
├──────────────────┴──────────────────────────────┤
│ 🤖 AI Assistant                                 │
│ > Make buttons larger and add animations        │
└─────────────────────────────────────────────────┘
```

**SVG Visual Editor:**

```
┌─────────────────────────────────────────────────┐
│ [Code] [Visual] [Split]        icon.svg    [⚙] │
├──────────────────┬──────────────────────────────┤
│                  │                              │
│ <svg viewBox="0  │    ┌────────────────┐       │
│   0 100 100">    │    │                │       │
│   <circle        │    │      ⚫        │       │
│     cx="50"      │    │    Selected    │       │
│     cy="50"      │    │                │       │
│     r="40"       │    └────────────────┘       │
│     fill="#00f"  │                              │
│   />             │    [Tools]                   │
│ </svg>           │    ◯ Circle  □ Rect         │
│                  │    ✏ Path    T Text         │
│                  │    🎨 Fill: #0000ff          │
│                  │    ✂ Stroke: none            │
│                  │    📏 Size: 40px             │
│                  │                              │
├──────────────────┴──────────────────────────────┤
│ 🤖 AI: "Add a drop shadow to this circle"      │
└─────────────────────────────────────────────────┘
```

### Related Features

**Asset Organization:**

- Folder structure in sidebar (css/, images/, icons/)
- Search/filter assets by type
- Batch operations (optimize all SVGs)
- Asset dependencies (which scripts use this CSS?)

**Asset Optimization:**

- CSS minification
- SVG path optimization (SVGO)
- Image compression
- Unused CSS detection

**Collaboration:**

- Asset version history
- Comments on specific lines
- Share asset previews with team
- AI suggests improvements

### Success Metrics

**Phase 1 Success:**

- Can edit CSS/SVG in Monaco with syntax highlighting
- Can save assets back to server
- No need to download/re-upload

**Phase 2 Success:**

- Live preview updates within 500ms
- Visual feedback for all changes
- 80% of edits done without leaving editor

**Phase 3 Success:**

- 50% of CSS edits use AI assistance
- Natural language commands work for common tasks
- Diff preview shows clear before/after

**Phase 4 Success:**

- Visual SVG editor used for icon creation
- CSS optimizations reduce file size 20%+
- Complete asset workflow in one tool

## Markdown Document Editing

### Problem Statement

While aiwebengine focuses on JavaScript scripts, many use cases require rich text content:

- Documentation pages
- Blog posts and articles
- README files
- Help/support content
- User-facing content pages
- API documentation

Currently, there's no good way to create and edit markdown documents within the editor. Users must either:

- Hardcode HTML in JavaScript strings
- Store content externally and fetch it
- Build separate content management systems

### Use Cases

**Documentation Site:**

```javascript
// Current approach (awkward)
function serveDocs(req) {
  return {
    status: 200,
    body: `
      <h1>Documentation</h1>
      <p>This is hard to maintain...</p>
    `,
    contentType: "text/html",
  };
}

// Desired approach
function serveDocs(req) {
  const content = loadDocument("/docs/getting-started.md");
  return renderMarkdown(content);
}
```

**Blog System:**

- Write posts in markdown
- Store in aiwebengine storage
- Render with consistent styling
- Support front matter (metadata)

**Content Pages:**

- About page
- Terms of service
- Privacy policy
- Landing pages

### Option 1: Markdown Editor with Live Preview (Recommended)

**Description:** Split-pane editor with markdown on left and rendered HTML on right.

**UI Layout:**

````
┌─────────────────────────────────────────────────┐
│ Document: getting-started.md          [⚙ Tools]│
├──────────────────┬──────────────────────────────┤
│                  │                              │
│ # Getting Started│  Getting Started             │
│                  │                              │
│ Welcome to       │  Welcome to **aiwebengine**! │
│ **aiwebengine**! │                              │
│                  │  ## Installation             │
│ ## Installation  │                              │
│                  │  Run the following:          │
│ Run the following│                              │
│                  │  npm install aiwebengine     │
│ ```bash          │                              │
│ npm install      │  Quick Start                 │
│ ```              │                              │
│                  │  • Create a script           │
│ ## Quick Start   │  • Define handler            │
│                  │  • Deploy                    │
│ - Create script  │                              │
│ - Define handler │                              │
│                  │                              │
├──────────────────┴──────────────────────────────┤
│ 🤖 AI: "Expand the installation section"       │
└─────────────────────────────────────────────────┘
````

**Features:**

- Monaco editor with markdown language mode
- Live HTML preview with synchronized scrolling
- Syntax highlighting for code blocks
- Table of contents generation
- Export to HTML/PDF

**Implementation:**

```javascript
// Markdown rendering with marked.js
import { marked } from "marked";
import DOMPurify from "dompurify";

function renderMarkdownPreview(content) {
  const html = marked.parse(content);
  const clean = DOMPurify.sanitize(html);

  const preview = document.getElementById("markdown-preview");
  preview.innerHTML = clean;

  // Syntax highlighting for code blocks
  preview.querySelectorAll("pre code").forEach((block) => {
    hljs.highlightBlock(block);
  });
}

// Auto-update on content change
monacoEditor.onDidChangeModelContent(() => {
  debounce(() => {
    const content = monacoEditor.getValue();
    renderMarkdownPreview(content);
  }, 300);
});
```

**Advantages:**

- Familiar markdown syntax
- Live preview prevents syntax errors
- Easy to learn for non-developers
- Portable content (markdown is standard)

**Disadvantages:**

- Need markdown parser library (~50KB)
- Preview styling must match production

### Option 2: WYSIWYG Markdown Editor

**Description:** Rich text editor that outputs clean markdown.

**Tools to Consider:**

- **Toast UI Editor** (Markdown WYSIWYG)
- **Editor.js** (Block-based editor)
- **Milkdown** (WYSIWYG markdown)
- **Tiptap** (Headless editor)

**UI Modes:**

```
[Markdown] [WYSIWYG] [Split]
     ↑         ↑         ↑
     └─────────┴─────────┘
        Toggle modes
```

**WYSIWYG Mode:**

```
┌─────────────────────────────────────────┐
│ [B] [I] [H1] [H2] [Link] [Image] [Code]│
├─────────────────────────────────────────┤
│                                         │
│  Getting Started                        │
│  ═══════════════                        │
│                                         │
│  Welcome to aiwebengine!                │
│                                         │
│  Installation                           │
│  ────────────                           │
│                                         │
│  Run the following command:             │
│                                         │
│  ┌───────────────────────────┐         │
│  │ npm install aiwebengine   │         │
│  └───────────────────────────┘         │
│                                         │
└─────────────────────────────────────────┘
```

**Advantages:**

- Lower barrier for non-technical users
- Familiar word-processor-like interface
- Prevents markdown syntax errors
- Direct formatting

**Disadvantages:**

- Larger dependency (200-500KB)
- More complex integration
- Can generate non-standard markdown

### Option 3: AI-Assisted Content Creation

**Description:** Use AI to generate, improve, and structure markdown content.

**AI Capabilities:**

**Content Generation:**

```
User: "Create a getting started guide for aiwebengine"
AI: Generates complete markdown document with:
- Introduction
- Installation steps
- Quick start tutorial
- Common use cases
- Next steps

User: "Write a blog post about serverless JavaScript"
AI: Creates structured article with sections
```

**Content Improvement:**

```
User: "Make this more concise"
AI: Reduces word count while preserving meaning

User: "Add code examples"
AI: Inserts relevant code snippets

User: "Improve SEO"
AI: Adds meta descriptions, better headings

User: "Check grammar and spelling"
AI: Corrects errors, improves clarity
```

**Content Restructuring:**

```
User: "Add a table of contents"
AI: Generates TOC from headings

User: "Split this into multiple documents"
AI: Creates logical document hierarchy

User: "Convert this list to a table"
AI: Reformats content as markdown table
```

**System Prompt Extension:**

```
MARKDOWN DOCUMENT EDITING:
When working with markdown documents:
- Use proper markdown syntax (headings, lists, links)
- Structure content logically with clear hierarchy
- Add code blocks with language specifications
- Use tables for structured data
- Include internal links for navigation
- Consider SEO with descriptive headings
- Maintain consistent formatting style
- Suggest images/diagrams where helpful
```

**Advantages:**

- Dramatically speeds up content creation
- Improves content quality
- Helps non-writers create professional docs
- Unique competitive advantage

**Disadvantages:**

- Token usage for longer documents
- AI may need guidance on tone/style
- Review needed for accuracy

### Option 4: Front Matter Support

**Description:** Support YAML front matter for document metadata.

**Front Matter Example:**

```markdown
---
title: Getting Started with aiwebengine
author: Lasse Pajunen
date: 2025-11-09
tags: [tutorial, getting-started, javascript]
description: Learn how to build your first aiwebengine application
published: true
---

# Getting Started

Content goes here...
```

**Parsing Front Matter:**

```javascript
import matter from "gray-matter";

function parseMarkdownDocument(content) {
  const { data, content: markdown } = matter(content);

  return {
    metadata: data, // { title, author, date, tags, ... }
    content: markdown,
    html: marked.parse(markdown),
  };
}
```

**Use Cases:**

- Blog post metadata (title, date, author)
- SEO tags (description, keywords)
- Publishing status (draft, published)
- Custom fields (category, featured image)
- Template selection (layout, theme)

**Metadata Editor UI:**

```
┌─────────────────────────────────────────┐
│ Metadata                                │
├─────────────────────────────────────────┤
│ Title: [Getting Started              ] │
│ Author: [Lasse Pajunen              ] │
│ Date: [2025-11-09                   ] │
│ Tags: [tutorial] [getting-started   ] │
│ Published: [✓]                          │
│                                         │
│ [Edit YAML] [Add Field +]               │
└─────────────────────────────────────────┘
```

**Advantages:**

- Standard metadata format
- Flexible field definitions
- Easy to parse and query
- Used by static site generators

**Disadvantages:**

- Need YAML parser
- UI complexity for metadata editing

### Option 5: Document Templates

**Description:** Pre-built markdown templates for common document types.

**Template Library:**

```javascript
const DOCUMENT_TEMPLATES = {
  "blog-post": {
    name: "Blog Post",
    icon: "📝",
    content: `---
title: Untitled Post
date: ${new Date().toISOString().split("T")[0]}
author: 
tags: []
---

# Title

## Introduction

Write your introduction here...

## Main Content

Your main points...

## Conclusion

Wrap up your thoughts...
`,
  },

  "api-docs": {
    name: "API Documentation",
    icon: "📚",
    content: `# API Documentation

## Overview

Brief description of the API.

## Authentication

Explain authentication requirements.

## Endpoints

### GET /api/resource

**Description:** What this endpoint does.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| id   | string | Yes | Resource ID |

**Example Request:**
\`\`\`bash
curl https://api.example.com/resource/123
\`\`\`

**Example Response:**
\`\`\`json
{
  "id": "123",
  "data": "..."
}
\`\`\`
`,
  },

  readme: {
    name: "README",
    icon: "📄",
    content: `# Project Name

Brief description of your project.

## Features

- Feature 1
- Feature 2
- Feature 3

## Installation

\`\`\`bash
npm install
\`\`\`

## Usage

\`\`\`javascript
// Example code
\`\`\`

## Contributing

Contribution guidelines.

## License

MIT License
`,
  },

  tutorial: {
    name: "Tutorial",
    icon: "🎓",
    content: `# Tutorial: [Topic]

## What You'll Learn

- Learning objective 1
- Learning objective 2
- Learning objective 3

## Prerequisites

- Prerequisite 1
- Prerequisite 2

## Step 1: [First Step]

Detailed instructions...

## Step 2: [Second Step]

More instructions...

## Next Steps

What to do after completing this tutorial.
`,
  },
};
```

**Template Selection UI:**

```
┌─────────────────────────────────────────┐
│ New Document                            │
├─────────────────────────────────────────┤
│                                         │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐ │
│  │   📝    │  │   📚    │  │   📄    │ │
│  │  Blog   │  │   API   │  │ README  │ │
│  │  Post   │  │  Docs   │  │         │ │
│  └─────────┘  └─────────┘  └─────────┘ │
│                                         │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐ │
│  │   🎓    │  │   📋    │  │   ✏️    │ │
│  │Tutorial │  │  Guide  │  │  Blank  │ │
│  │         │  │         │  │         │ │
│  └─────────┘  └─────────┘  └─────────┘ │
│                                         │
│ [Cancel] [AI Generate Custom Template] │
└─────────────────────────────────────────┘
```

**AI Template Generation:**

```
User: "Create a template for product documentation"
AI: Generates custom template with sections:
- Product Overview
- Key Features
- Installation
- Configuration
- Usage Examples
- Troubleshooting
- FAQ
```

**Advantages:**

- Quick start for common document types
- Consistent structure
- Reduces decision fatigue
- Easy to customize

**Disadvantages:**

- Need to maintain template library
- May not fit all use cases

### Option 6: Document Storage and Organization

**Description:** Structured storage for markdown documents separate from scripts.

**Storage API:**

```javascript
// New document storage endpoints
POST   /api/documents          // Create document
GET    /api/documents          // List all documents
GET    /api/documents/:path    // Get document content
PUT    /api/documents/:path    // Update document
DELETE /api/documents/:path    // Delete document

// Folder support
GET    /api/documents/folder/:path  // List folder contents
POST   /api/documents/folder/:path  // Create folder
```

**Folder Structure:**

```
documents/
├── blog/
│   ├── 2025-11-01-first-post.md
│   ├── 2025-11-05-second-post.md
│   └── drafts/
│       └── upcoming-post.md
├── docs/
│   ├── getting-started.md
│   ├── api-reference.md
│   └── tutorials/
│       ├── basic-tutorial.md
│       └── advanced-tutorial.md
└── pages/
    ├── about.md
    ├── contact.md
    └── privacy-policy.md
```

**Sidebar Organization:**

```
┌───────────────────────┐
│ Documents             │
├───────────────────────┤
│ 📁 blog               │
│   📄 first-post.md    │
│   📄 second-post.md   │
│   📁 drafts           │
│ 📁 docs               │
│   📄 getting-start... │
│   📄 api-reference.md │
│   📁 tutorials        │
│ 📁 pages              │
│   📄 about.md         │
│   📄 contact.md       │
│                       │
│ [+ New Doc] [+ Folder]│
└───────────────────────┘
```

**Advantages:**

- Clean separation from scripts
- Better organization for many docs
- Easier to find content
- Supports hierarchical structure

**Disadvantages:**

- More complex storage system
- Need folder management UI

### Recommended Implementation Path

**Phase 1: Basic Markdown Editing**

1. Add Monaco markdown language support
2. Simple preview pane with marked.js
3. Save documents as assets or in separate storage
4. Syntax highlighting for code blocks

**Estimated Effort:** 4-6 hours

**Phase 2: Enhanced Preview & Styling**

1. Synchronized scrolling between editor and preview
2. Custom CSS themes for preview
3. Table of contents generation
4. Export to HTML

**Estimated Effort:** 1 day

**Phase 3: AI-Assisted Writing**

1. Content generation from prompts
2. Improvement suggestions
3. Grammar and style checking
4. SEO optimization suggestions

**Estimated Effort:** 2-3 days

**Phase 4: Advanced Features**

1. Front matter support with metadata editor
2. Document templates library
3. WYSIWYG mode toggle
4. Image upload and embedding
5. Folder-based organization

**Estimated Effort:** 1 week

### Technical Considerations

**Markdown Parser Options:**

- **marked** - Fast, lightweight (20KB)
- **markdown-it** - Extensible, plugins (45KB)
- **remark** - AST-based, powerful (60KB)
- **micromark** - Spec-compliant (50KB)

**Recommended:** marked.js for simplicity and size

**Syntax Highlighting:**

```javascript
import hljs from "highlight.js";
import "highlight.js/styles/vs2015.css";

marked.setOptions({
  highlight: function (code, lang) {
    const language = hljs.getLanguage(lang) ? lang : "plaintext";
    return hljs.highlight(code, { language }).value;
  },
});
```

**Sanitization:**

Always sanitize HTML output to prevent XSS:

```javascript
import DOMPurify from "dompurify";

const dirty = marked.parse(userInput);
const clean = DOMPurify.sanitize(dirty);
```

**Storage Strategy:**

**Option A: Store as Assets**

```
/assets/documents/blog/post.md
```

Pros: Uses existing asset system
Cons: Mixed with images/CSS

**Option B: Dedicated Document Storage**

```javascript
// Store in database with metadata
{
  path: '/blog/first-post.md',
  content: '# Title\n\nContent...',
  metadata: {
    title: 'First Post',
    date: '2025-11-09',
    author: 'Lasse'
  },
  created_at: '2025-11-09T10:00:00Z',
  updated_at: '2025-11-09T11:30:00Z'
}
```

Pros: Better metadata, queries, versioning
Cons: Need new database schema

**Option C: Hybrid Approach**

- Documents stored as assets for serving
- Metadata in database for querying/filtering
- Best of both worlds

### AI Prompt Examples

**Content Generation:**

```
User: "Write a tutorial about building a REST API"
AI: Creates complete markdown tutorial with:
- Introduction
- Prerequisites
- Step-by-step instructions
- Code examples
- Testing section
- Next steps

User: "Create API documentation for /api/users endpoint"
AI: Generates structured API docs with:
- Endpoint description
- Parameters table
- Example request/response
- Error codes
```

**Content Improvement:**

```
User: "Make this more technical"
AI: Increases technical depth, adds jargon

User: "Simplify for beginners"
AI: Uses simpler language, adds more explanation

User: "Add more examples"
AI: Inserts relevant code/use case examples

User: "Check for broken links"
AI: Validates URLs, suggests fixes
```

**Formatting:**

```
User: "Convert this text to a bullet list"
AI: Reformats as markdown list

User: "Add section headings"
AI: Structures content with ## headings

User: "Create a comparison table"
AI: Converts content to markdown table
```

### Document Rendering in Scripts

**Integration with Scripts:**

```javascript
// Helper function to load and render markdown
function loadDocument(path) {
  const response = fetch(`/api/documents${path}`);
  const markdown = JSON.parse(response).content;
  return markdown;
}

function renderDocument(path) {
  const markdown = loadDocument(path);

  // Server-side rendering would need marked.js ported to JS runtime
  // For now, return raw markdown and let client render
  return {
    status: 200,
    body: markdown,
    contentType: "text/markdown; charset=UTF-8",
  };
}

// Blog post example
function serveBlogPost(req) {
  const slug = req.path.split("/").pop();
  const markdown = loadDocument(`/blog/${slug}.md`);

  // Parse front matter
  const lines = markdown.split("\n");
  let metadata = {};
  let content = markdown;

  if (lines[0] === "---") {
    // Simple front matter parsing
    const endIndex = lines.indexOf("---", 1);
    // Parse YAML-like metadata
    // ... metadata extraction logic ...
    content = lines.slice(endIndex + 1).join("\n");
  }

  return {
    status: 200,
    body: JSON.stringify({
      metadata: metadata,
      content: content,
    }),
    contentType: "application/json",
  };
}

function init(context) {
  register("/blog/:slug", "serveBlogPost", "GET");
}
```

### Success Metrics

**Phase 1 Success:**

- Can create and edit markdown documents
- Live preview works smoothly
- Documents saved and retrieved correctly

**Phase 2 Success:**

- Preview styling matches production
- Code blocks highlighted correctly
- TOC generates automatically

**Phase 3 Success:**

- AI generates quality content on demand
- Improvement suggestions are useful
- 30%+ reduction in writing time

**Phase 4 Success:**

- Front matter editing is intuitive
- Templates speed up document creation
- Complete documentation workflow in editor

## Script and Asset Versioning

### Problem Statement

Currently, aiwebengine has no versioning system for scripts or assets. Once you save a change, the previous version is lost. This creates several problems:

**Risk of Data Loss:**

- Accidental overwrites
- Bad deployments
- No way to undo mistakes

**No Audit Trail:**

- Who changed what and when?
- Why was a change made?
- What was the previous working state?

**Collaboration Challenges:**

- Concurrent editing conflicts
- No merge capabilities
- Hard to review changes

**Testing and Rollback:**

- Can't safely test changes
- No rollback to known-good state
- Difficult to debug regressions

### Use Cases

**Accidental Overwrite:**

```
Developer: Saves script with breaking change
System: Previous version lost forever
Developer: "I need to undo this but can't!"
```

**Production Incident:**

```
User: "The API stopped working after today's deployment"
Developer: "What changed? I can't see the diff"
System: No history available
```

**Collaborative Development:**

```
Developer A: Editing user-api.js
Developer B: Also editing user-api.js
System: Last save wins, work lost
```

**Safe Experimentation:**

```
Developer: "I want to refactor but keep the working version"
System: Currently requires copy-paste backup
```

### Option 1: Simple Versioning (Recommended First Step)

**Description:** Keep last N versions with timestamps.

**Storage Structure:**

```javascript
// In database
{
  script_name: "user-api.js",
  versions: [
    {
      version: 5,
      content: "// current content...",
      created_at: "2025-11-09T12:00:00Z",
      created_by: "lasse@example.com",
      message: "Added user deletion endpoint",
      size: 2048
    },
    {
      version: 4,
      content: "// previous content...",
      created_at: "2025-11-09T10:30:00Z",
      created_by: "lasse@example.com",
      message: "Fixed validation bug",
      size: 1987
    },
    // ... keep last 10 versions
  ],
  current_version: 5
}
```

**API Endpoints:**

```javascript
GET    /api/scripts/:name/versions        // List all versions
GET    /api/scripts/:name/versions/:num   // Get specific version
POST   /api/scripts/:name/restore/:num    // Restore old version
POST   /api/scripts/:name                 // Create new version (with message)
GET    /api/scripts/:name/diff/:v1/:v2    // Compare versions
```

**UI - Version History Panel:**

```
┌─────────────────────────────────────────┐
│ Version History: user-api.js            │
├─────────────────────────────────────────┤
│ ● v5  Nov 9, 12:00  Lasse    [Current] │
│   "Added user deletion endpoint"        │
│   [View] [Diff] [Restore]               │
│                                         │
│ ○ v4  Nov 9, 10:30  Lasse              │
│   "Fixed validation bug"                │
│   [View] [Diff] [Restore]               │
│                                         │
│ ○ v3  Nov 8, 16:45  Lasse              │
│   "Added pagination support"            │
│   [View] [Diff] [Restore]               │
│                                         │
│ ○ v2  Nov 8, 14:20  Lasse              │
│   "Initial user API"                    │
│   [View] [Diff] [Restore]               │
└─────────────────────────────────────────┘
```

**Save Dialog with Version Message:**

```
┌─────────────────────────────────────────┐
│ Save Script: user-api.js                │
├─────────────────────────────────────────┤
│ Describe your changes:                  │
│ ┌─────────────────────────────────────┐ │
│ │ Added user deletion endpoint        │ │
│ │                                     │ │
│ └─────────────────────────────────────┘ │
│                                         │
│ [Cancel]                 [Save Version] │
└─────────────────────────────────────────┘
```

**Advantages:**

- Simple to implement
- Solves most common problems
- Low storage overhead (text compresses well)
- No external dependencies

**Disadvantages:**

- Limited to N versions
- No branching
- No merge capabilities
- Basic diff only

### Option 2: Git Integration

**Description:** Use Git as the versioning backend.

**Implementation:**

```javascript
// Initialize git repo for scripts
git init /data/scripts
git config user.name "aiwebengine"
git config user.email "system@aiwebengine.local"

// On script save
async function saveScript(scriptName, content, userId, message) {
  // Write file
  writeFile(`/data/scripts/${scriptName}`, content);

  // Git commit
  exec(`cd /data/scripts && git add ${scriptName}`);
  exec(`cd /data/scripts && git commit -m "${message}" --author="${userId}"`);

  return { success: true, hash: getLatestHash() };
}

// Get versions
async function getVersions(scriptName) {
  const log = exec(`cd /data/scripts && git log --follow -- ${scriptName}`);
  return parseGitLog(log);
}

// Restore version
async function restoreVersion(scriptName, hash) {
  exec(`cd /data/scripts && git checkout ${hash} -- ${scriptName}`);
}

// Diff versions
async function diffVersions(scriptName, hash1, hash2) {
  return exec(`cd /data/scripts && git diff ${hash1}..${hash2} -- ${scriptName}`);
}
```

**UI Features:**

- Git commit history
- Branch visualization
- Merge conflict resolution
- Blame view (who changed which line)

**Advantages:**

- Industry-standard versioning
- Powerful branching/merging
- Complete history forever
- Familiar to developers
- Can push to GitHub/GitLab

**Disadvantages:**

- Requires Git installed
- More complex implementation
- Overkill for simple use cases
- Git learning curve for users

### Option 3: Snapshot-Based Versioning

**Description:** Create named snapshots at key moments.

**Snapshot Types:**

```javascript
const SNAPSHOT_TYPES = {
  MANUAL: "manual", // User creates snapshot
  AUTO: "auto", // Auto-save every N minutes
  DEPLOY: "deploy", // Before deployment
  BACKUP: "backup", // Daily backup
};
```

**Snapshot Creation:**

```
User clicks "Create Snapshot"
┌─────────────────────────────────────────┐
│ Create Snapshot                         │
├─────────────────────────────────────────┤
│ Name: [Production Deploy 2025-11-09   ]│
│                                         │
│ Description:                            │
│ ┌─────────────────────────────────────┐ │
│ │ Stable version before refactoring   │ │
│ │ All features working and tested     │ │
│ └─────────────────────────────────────┘ │
│                                         │
│ Include:                                │
│ ☑ All scripts (15 files)                │
│ ☑ All assets (23 files)                 │
│ ☑ Configuration                         │
│                                         │
│ [Cancel]                  [Create]      │
└─────────────────────────────────────────┘
```

**Snapshot Browser:**

```
┌─────────────────────────────────────────┐
│ Snapshots                               │
├─────────────────────────────────────────┤
│ 📸 Production Deploy 2025-11-09         │
│    Nov 9, 12:00 • Manual • 38 files     │
│    "Stable version before refactoring"  │
│    [Restore] [View] [Download]          │
│                                         │
│ 📸 Auto-save 2025-11-09 10:30           │
│    Nov 9, 10:30 • Auto • 38 files       │
│    [Restore] [View] [Download]          │
│                                         │
│ 📸 Pre-deployment 2025-11-08            │
│    Nov 8, 16:00 • Deploy • 36 files     │
│    "Before adding auth system"          │
│    [Restore] [View] [Download]          │
└─────────────────────────────────────────┘
```

**Advantages:**

- Simple mental model
- Named checkpoints
- Can snapshot entire system
- Easy rollback

**Disadvantages:**

- Manual snapshot creation
- Storage grows quickly
- Less granular than per-save versions

### Option 4: Time-Travel Debugging

**Description:** Store every version with timestamps, browse history like video playback.

**Time-Travel UI:**

```
┌─────────────────────────────────────────┐
│ Time Travel: user-api.js                │
├─────────────────────────────────────────┤
│                                         │
│ ◀◀ ◀ ▶ ▶▶  [========●=======]  Now     │
│                                         │
│ Nov 8              Nov 9                │
│ 14:00             12:00                 │
│                                         │
│ Currently viewing:                      │
│ Nov 9, 2025 10:30:45                    │
│ By: lasse@example.com                   │
│ Changes: +15 lines, -3 lines            │
│                                         │
├─────────────────────────────────────────┤
│ function handleGetUser(req) {           │
│   const userId = req.params.id;         │
│   // ... code at this point in time ... │
│ }                                       │
└─────────────────────────────────────────┘
```

**Features:**

- Scrub timeline to any point
- See code at any moment
- Animate changes over time
- Find when bug was introduced

**Advantages:**

- Visual and intuitive
- Great for debugging
- Find exact moment of change
- No manual versioning needed

**Disadvantages:**

- Very complex implementation
- Large storage requirements
- Performance challenges

### Option 5: Differential Storage

**Description:** Only store diffs between versions to save space.

**Storage Optimization:**

```javascript
// Instead of storing full content each time
versions: [
  { v: 1, content: "full content..." }, // 2KB
  { v: 2, content: "full content..." }, // 2KB
  { v: 3, content: "full content..." }, // 2KB
];
// Total: 6KB for 3 versions

// Store base + diffs
versions: [
  { v: 1, type: "full", content: "full content..." }, // 2KB
  { v: 2, type: "diff", patch: "+10 lines..." }, // 200B
  { v: 3, type: "diff", patch: "-2 lines..." }, // 150B
];
// Total: 2.35KB for 3 versions
```

**Reconstruction:**

```javascript
function getVersion(scriptName, versionNum) {
  const versions = loadVersions(scriptName);

  // Find last full version before requested
  let content = findLastFullVersion(versions, versionNum);

  // Apply diffs sequentially
  for (let v of getVersionsInRange(lastFull, versionNum)) {
    if (v.type === "diff") {
      content = applyPatch(content, v.patch);
    }
  }

  return content;
}
```

**Advantages:**

- Minimal storage usage
- Unlimited version history
- Efficient for small changes

**Disadvantages:**

- Complex implementation
- Need diff/patch library
- Slower version reconstruction
- Risk if diff chain corrupted

### Recommended Implementation Path

**Phase 1: Basic Versioning (MVP)**

1. Store last 10 versions per script
2. Timestamp and user tracking
3. Simple restore functionality
4. Version list UI

**Estimated Effort:** 2-3 days

**Phase 2: Enhanced History**

1. Diff viewer between versions
2. Version comparison UI
3. Optional commit messages
4. Search version history

**Estimated Effort:** 3-4 days

**Phase 3: Snapshots**

1. Create named snapshots
2. Snapshot browser
3. Full system restore
4. Auto-snapshot before deployments

**Estimated Effort:** 1 week

**Phase 4: Advanced Features**

1. Branching (experimental versions)
2. Merge capabilities
3. Collaborative editing with conflict detection
4. Git backend option

**Estimated Effort:** 2-3 weeks

### Technical Considerations

**Storage Strategy:**

**Option A: Database Storage**

```sql
CREATE TABLE script_versions (
  id SERIAL PRIMARY KEY,
  script_name VARCHAR(255),
  version INTEGER,
  content TEXT,
  created_at TIMESTAMP,
  created_by VARCHAR(255),
  message TEXT,
  size INTEGER,
  hash VARCHAR(64)  -- SHA-256 of content
);

CREATE INDEX idx_script_versions
ON script_versions(script_name, version DESC);
```

**Option B: File System Storage**

```
/data/versions/
  user-api.js/
    v001_2025-11-08T14-20-00.js
    v002_2025-11-08T16-45-00.js
    v003_2025-11-09T10-30-00.js
    manifest.json
```

**Option C: Hybrid**

- Current version in regular storage
- Historical versions in archive storage (S3, compressed)
- Metadata in database

**Retention Policies:**

```javascript
const RETENTION_POLICY = {
  keep_all: "7 days", // Keep every version for 1 week
  keep_daily: "30 days", // Then daily snapshots for month
  keep_weekly: "1 year", // Then weekly for year
  keep_monthly: "forever", // Then monthly forever
};
```

**Diff Algorithm:**

- **Myers diff** - Standard, used by Git
- **Patience diff** - Better for code
- **Histogram diff** - Fast, good results

Libraries:

- `diff` (npm) - Pure JavaScript
- `fast-diff` - Faster alternative
- `diff-match-patch` - Google's implementation

**Conflict Resolution:**

```javascript
// Detect concurrent edits
function detectConflict(scriptName, baseVersion) {
  const currentVersion = getCurrentVersion(scriptName);
  return currentVersion !== baseVersion;
}

// 3-way merge
function mergeVersions(base, yours, theirs) {
  const diff1 = diff(base, yours);
  const diff2 = diff(base, theirs);

  // If changes don't overlap, auto-merge
  if (!hasConflicts(diff1, diff2)) {
    return applyBoth(base, diff1, diff2);
  }

  // Otherwise, present conflict UI
  return {
    conflict: true,
    sections: markConflicts(diff1, diff2),
  };
}
```

### AI Integration with Versioning

**AI-Assisted Commit Messages:**

```
User saves script
AI analyzes changes:
- Added 2 new functions
- Modified 1 function
- Removed 5 lines

AI suggests: "Added user deletion and email verification functions"
User can accept, edit, or write own message
```

**AI Change Summaries:**

```
Viewing version history:

v5 → v6
AI Summary: "Refactored authentication to use JWT tokens
instead of session cookies. Added token refresh endpoint."

v4 → v5
AI Summary: "Fixed bug where null emails caused crashes.
Added email validation."
```

**Smart Rollback:**

```
User: "Undo the change that broke user login"
AI: Analyzes recent versions
AI: "The issue was introduced in v7. Recommending
     rollback to v6 which has working authentication."
User: [Confirm Rollback]
```

### Success Metrics

**Phase 1 Success:**

- No accidental data loss
- Can restore any of last 10 versions
- Version history visible in UI

**Phase 2 Success:**

- Diff viewer clearly shows changes
- Commit messages provide context
- Can find specific past versions

**Phase 3 Success:**

- Snapshots created before risky changes
- Quick rollback to known-good state
- Team members can see change history

**Phase 4 Success:**

- Concurrent editing doesn't lose work
- Merge conflicts handled gracefully
- Complete audit trail of all changes

## Editor UI Improvements

### Context Awareness Indicators

**Token Usage Display:**

- Show current/max tokens in AI panel
- Color-code: green (<50%), yellow (50-80%), red (>80%)
- Warning before hitting limits

**Script Relationship Viewer:**

- Visual graph showing script dependencies
- Click node to load that script
- Highlight scripts referenced in current file

**Recent Scripts:**

- Quick access to recently edited scripts
- Show in sidebar or dropdown
- Remember per-session

### Split View / Multi-Pane

**Description:** Edit multiple scripts simultaneously.

**Layout Options:**

- Vertical split (2 scripts side by side)
- Horizontal split (2 scripts stacked)
- Grid (up to 4 scripts)

**Use Cases:**

- Compare scripts
- Copy code between scripts
- Edit API provider and consumer together
- View documentation while coding

### Integrated Documentation

**Features:**

- Hover over API functions to see docs
- Inline examples
- Search documentation from editor
- Link to full docs
- AI explains any function on demand

**Implementation:**

```javascript
// Hover over "register" shows:
╔═══════════════════════════════════╗
║ register(path, handler, method)   ║
║ Register an HTTP route handler    ║
║                                   ║
║ Example:                          ║
║   register('/api/users',          ║
║            'getUsers', 'GET')     ║
║                                   ║
║ [Full Docs] [AI Explain] [Example]║
╚═══════════════════════════════════╝
```

## Implementation Priorities

### Phase 1: Foundation (Quick Wins)

1. ✅ Basic AI assistant (already implemented)
2. Script reference syntax (`@script.js`)
3. Keyword-based automatic context
4. Token usage display
5. Recent scripts list

### Phase 2: Enhanced Context

1. Script selection UI
2. Dependency detection
3. Auto-include related scripts
4. Context preview

### Phase 3: Advanced AI Features

1. Semantic search with embeddings
2. Test generation
3. Code review suggestions
4. Smart refactoring

### Phase 4: Collaboration & Testing

1. Integrated testing UI
2. Diff viewer
3. AI debugging assistant
4. Multi-script editing

## Technical Considerations

### Token Budget Management

- Average script: ~500-1000 tokens
- System prompt: ~2000 tokens
- User prompt: ~100-500 tokens
- Response budget: ~2000-4000 tokens
- Total context: ~8000 tokens (Claude Haiku)

**Strategy:**

- Limit to 3-5 additional scripts max
- Truncate very large scripts
- Prioritize by relevance score
- Show token usage in real-time

### Caching Strategy

- Cache script embeddings (if using Phase 3)
- Cache dependency graph
- Invalidate on script save
- Store in sharedStorage or browser localStorage

### Performance

- Lazy load scripts only when needed
- Debounce AI requests (user stops typing)
- Show loading indicators
- Allow cancelling in-flight requests
- Background indexing for semantic search

### Privacy & Security

- All AI processing via Anthropic API
- Scripts never leave user's instance
- Secrets injection handled server-side
- No telemetry without opt-in

## Future Ideas (Beyond MVP)

### Collaborative Editing

- Real-time multi-user editing
- Shared AI sessions
- Comment threads in code
- Change approval workflows

### AI Training on Custom Scripts

- Fine-tune on organization's scripts
- Learn internal patterns and conventions
- Custom linting rules
- Company-specific best practices

### Visual Script Builder

- Drag-and-drop interface for simple scripts
- Flow-based programming for beginners
- AI converts visual → code
- Code ← → Visual bidirectional editing

### Natural Language to Script

- "Create an API that stores and retrieves notes"
- AI generates complete, working script
- Interactive refinement
- Deploy with one click

### Script Marketplace

- Share scripts with community
- AI helps adapt scripts to your use case
- One-click install from marketplace
- Automated compatibility checks

## Feedback & Iteration

This document should evolve based on:

- User feedback and feature requests
- Performance metrics (which AI features are most used)
- Technical feasibility discoveries
- Community contributions

**Last Updated:** November 7, 2025
