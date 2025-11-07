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
  const scripts = listScripts();
  for (const script of scripts) {
    const content = getScript(script);
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
│ +   const posts = scriptStorage.getItem('posts');       │
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
      this.backups.set(scriptName, getScript(scriptName));
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
- Store in scriptStorage or browser localStorage

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
