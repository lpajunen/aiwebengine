# AI Assistant Implementation Plan

## Overview

This document outlines the plan to integrate AI capabilities into the aiwebengine editor, starting with Claude Haiku support. The editor already has a UI component for the AI assistant, but the backend implementation is needed.

**🔒 CRITICAL SECURITY NOTE**: This plan follows secure secret management principles where API keys and secrets NEVER cross the Rust/JavaScript boundary. JavaScript code can only reference secrets by identifier, and the Rust layer injects actual values at point of use. See `SECRET_MANAGEMENT_SECURITY_ANALYSIS.md` for detailed rationale.

## Current State

### ✅ Already Implemented

- **UI Components**: AI assistant panel in `/editor` with:
  - Collapsible panel at bottom of editor
  - Text input for prompts
  - Response display area
  - Submit/Clear buttons
  - Loading states
- **Frontend Logic**: JavaScript in `assets/editor.js`:
  - `toggleAIAssistant()` - Show/hide panel
  - `submitAIPrompt()` - Send prompt to `/api/ai-assistant`
  - `clearAIPrompt()` - Clear input
  - Error handling and UI feedback

### ❌ Not Yet Implemented

- Backend endpoint `/api/ai-assistant` (called by UI but doesn't exist)
- Secrets management system (for storing API keys)
- HTTP client for making external API calls
- Claude API integration
- Rate limiting for AI calls
- Cost tracking/monitoring

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                     Editor UI (Browser)                      │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  AI Assistant Panel                                  │   │
│  │  - Text input for prompts                           │   │
│  │  - Display area for responses                       │   │
│  └─────────────────────────────────────────────────────┘   │
└───────────────────────────┬─────────────────────────────────┘
                            │ POST /api/ai-assistant
                            │ { "prompt": "..." }
                            ▼
┌─────────────────────────────────────────────────────────────┐
│              aiwebengine (Rust Backend)                      │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  Editor Script (scripts/feature_scripts/editor.js) │   │
│  │  - Handles /api/ai-assistant endpoint              │   │
│  │  - Validates input                                  │   │
│  │  - Calls AI.chat() helper                          │   │
│  └──────────────────┬──────────────────────────────────┘   │
│                     │                                        │
│                     ▼                                        │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  JavaScript Runtime (Deno/v8)                       │   │
│  │  - AI.chat(prompt, options) API                     │   │
│  │  - Secrets.get("anthropic_api_key")                 │   │
│  │  - fetch() for HTTP calls                           │   │
│  └──────────────────┬──────────────────────────────────┘   │
│                     │                                        │
│                     ▼                                        │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  Rust Native Implementations                        │   │
│  │  - Secrets Manager (src/secrets.rs)                 │   │
│  │  - HTTP Client (src/http_client.rs)                 │   │
│  │  - AI Integration (src/ai/mod.rs)                   │   │
│  └──────────────────┬──────────────────────────────────┘   │
└────────────────────┬┴──────────────────────────────────────┘
                     │
                     │ HTTPS
                     ▼
┌─────────────────────────────────────────────────────────────┐
│         Anthropic API (api.anthropic.com)                    │
│         - Claude Haiku model                                 │
│         - Messages API                                       │
└─────────────────────────────────────────────────────────────┘
```

## Implementation Phases

### Phase 1: Secrets Management Foundation

**Priority**: CRITICAL  
**Estimated Time**: 2-3 days

#### 1.1 Create Secrets Module (`src/secrets.rs`)

**Features**:

- In-memory secrets store (initial implementation)
- Load secrets from configuration (environment variables, config file)
- API for storing/retrieving secrets
- Thread-safe access using `Arc<RwLock<HashMap<String, String>>>`
- Automatic redaction in logs

**Configuration Support**:

```yaml
# config.yaml
secrets:
  anthropic_api_key: "sk-ant-api03-..." # From environment or direct config
  openai_api_key: "sk-proj-..." # For future expansion
```

Or via environment variables:

```bash
SECRET_ANTHROPIC_API_KEY=sk-ant-api03-...
SECRET_OPENAI_API_KEY=sk-proj-...
```

**API Design**:

```rust
pub struct SecretsManager {
    secrets: Arc<RwLock<HashMap<String, String>>>,
}

impl SecretsManager {
    pub fn new() -> Self;
    pub fn load_from_config(&mut self, config: &Config);
    pub fn load_from_env(&mut self);
    pub fn get(&self, identifier: &str) -> Option<String>;
    pub fn set(&mut self, identifier: String, value: String);
    pub fn exists(&self, identifier: &str) -> bool;
    pub fn list_identifiers(&self) -> Vec<String>;
    pub fn delete(&mut self, identifier: &str) -> bool;
}
```

#### 1.2 Integrate Secrets into JavaScript Runtime (`src/js_engine.rs`)

**Add Global `Secrets` Object** (LIMITED API - Security Critical):

```javascript
// Available in all scripts
Secrets.exists("anthropic_api_key"); // Returns boolean
Secrets.list(); // Returns array of identifiers

// NOTE: NO Secrets.get() - JavaScript must never retrieve secret values!
// Secrets are injected by Rust layer at point of use (HTTP requests, etc.)
```

**Rust Implementation**:

- Create `secrets_exists()` and `secrets_list()` helper functions ONLY
- DO NOT implement `secrets_get()` that returns values
- Expose to JavaScript runtime via `deno_core::extension!`
- Implement automatic redaction in logging

**Security Note**: This aligns with REQ-SEC-008 - secrets never cross the Rust/JavaScript boundary.

#### 1.3 Configuration Schema Updates (`src/config.rs`)

Add secrets configuration:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    // ... existing fields
    #[serde(default)]
    pub secrets: SecretsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SecretsConfig {
    #[serde(flatten)]
    pub values: HashMap<String, String>,
}
```

---

### Phase 2: HTTP Client Implementation

**Priority**: CRITICAL  
**Estimated Time**: 3-4 days

#### 2.1 Create HTTP Client Module (`src/http_client.rs`)

**Features**:

- Async HTTP client using `reqwest`
- Support all HTTP methods (GET, POST, PUT, DELETE, PATCH)
- Request/response body handling (JSON, text, binary)
- Header management
- Timeout enforcement
- Error handling and retry logic
- Certificate validation

**API Design**:

```rust
pub struct HttpClient {
    client: reqwest::Client,
    default_timeout: Duration,
    max_response_size: usize,
}

impl HttpClient {
    pub async fn fetch(
        &self,
        url: String,
        options: FetchOptions,
    ) -> Result<FetchResponse, HttpError>;
}

pub struct FetchOptions {
    pub method: String,
    pub headers: Option<HashMap<String, String>>,
    pub body: Option<String>,
    pub timeout: Option<Duration>,
}

pub struct FetchResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
    pub ok: bool,
}
```

#### 2.2 Expose fetch() to JavaScript with Secret Injection (`src/js_engine.rs`)

**JavaScript API** (Web Fetch API compatible with secure secret injection):

```javascript
const response = await fetch("https://api.anthropic.com/v1/messages", {
  method: "POST",
  headers: {
    "Content-Type": "application/json",
    "x-api-key": "{{secret:anthropic_api_key}}", // Template syntax
    "anthropic-version": "2023-06-01",
  },
  body: JSON.stringify({
    model: "claude-3-haiku-20240307",
    max_tokens: 1024,
    messages: [{ role: "user", content: "Hello" }],
  }),
});

const data = await response.json();
```

**Rust Implementation - Secret Injection**:

```rust
pub async fn js_fetch(
    url: String,
    options: FetchOptions,
) -> Result<FetchResponse, HttpError> {
    let mut final_headers = HashMap::new();

    // Process headers and inject secrets
    for (key, value) in options.headers.unwrap_or_default() {
        // Check for template syntax: "{{secret:identifier}}"
        if value.starts_with("{{secret:") && value.ends_with("}}") {
            // Extract secret identifier
            let secret_id = value
                .strip_prefix("{{secret:")
                .unwrap()
                .strip_suffix("}}")
                .unwrap()
                .trim();

            // Look up secret in SecretsManager (stays in Rust)
            let secret_value = SECRETS_MANAGER
                .read()
                .unwrap()
                .get(secret_id)
                .ok_or_else(|| HttpError::SecretNotFound(secret_id.to_string()))?;

            // Inject actual secret value
            final_headers.insert(key, secret_value);

            // Audit log (identifier only, never value)
            audit_log::log_secret_access(secret_id, "fetch", &url);
        } else {
            // Regular header (not a secret)
            final_headers.insert(key, value);
        }
    }

    // Make HTTP request with injected secrets
    let response = HTTP_CLIENT
        .request(options.method.as_str(), &url)
        .headers(final_headers)
        .body(options.body.unwrap_or_default())
        .send()
        .await?;

    // Return response (no secrets in it)
    Ok(FetchResponse::from(response).await?)
}
```

**Key Implementation Points**:

- Detect `{{secret:identifier}}` pattern in header values
- Look up secret from SecretsManager (value never crosses to JavaScript)
- Replace template with actual value before HTTP request
- Log secret access for audit trail (identifier only)
- Throw error if secret not found
- Handle promise resolution
- Convert between JavaScript and Rust types
- Implement streaming for large responses (future enhancement)

#### 2.3 Security & Validation

**Implement in HTTP Client**:

- URL validation (block local addresses, private IPs)
- Header validation (check for malicious content)
- Response size limits
- Request timeout enforcement
- TLS/SSL certificate validation
- Rate limiting per domain

---

### Phase 3: AI Integration Module

**Priority**: HIGH  
**Estimated Time**: 3-4 days

#### 3.1 Create AI Module (`src/ai/mod.rs`, `src/ai/providers.rs`)

**Structure**:

```
src/ai/
├── mod.rs           # Public API
├── providers.rs     # Provider trait and implementations
├── claude.rs        # Claude/Anthropic implementation
└── types.rs         # Common types
```

**Provider Trait**:

```rust
#[async_trait]
pub trait AIProvider: Send + Sync {
    async fn chat(
        &self,
        messages: Vec<Message>,
        options: ChatOptions,
    ) -> Result<ChatResponse, AIError>;

    fn name(&self) -> &str;
    fn supports_streaming(&self) -> bool;
}

pub struct Message {
    pub role: String,  // "user", "assistant", "system"
    pub content: String,
}

pub struct ChatOptions {
    pub model: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub system_prompt: Option<String>,
}

pub struct ChatResponse {
    pub content: String,
    pub model: String,
    pub usage: TokenUsage,
    pub finish_reason: String,
}

pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}
```

#### 3.2 Claude Provider Implementation (`src/ai/claude.rs`)

**Features**:

- Anthropic Messages API integration
- Claude Haiku model support
- Error handling (rate limits, API errors, network issues)
- Token usage tracking
- Request/response logging (with secret redaction)

**Implementation**:

```rust
pub struct ClaudeProvider {
    http_client: Arc<HttpClient>,
    api_key: String,
    default_model: String,
    api_version: String,
}

impl ClaudeProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            http_client: Arc::new(HttpClient::new()),
            api_key,
            default_model: "claude-3-haiku-20240307".to_string(),
            api_version: "2023-06-01".to_string(),
        }
    }
}

#[async_trait]
impl AIProvider for ClaudeProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        options: ChatOptions,
    ) -> Result<ChatResponse, AIError> {
        // 1. Build request payload
        // 2. Make HTTP POST to api.anthropic.com/v1/messages
        // 3. Parse response
        // 4. Handle errors (rate limits, invalid requests, etc.)
        // 5. Return ChatResponse
    }
}
```

**API Endpoint**:

```
POST https://api.anthropic.com/v1/messages
Headers:
  x-api-key: <api_key>
  anthropic-version: 2023-06-01
  content-type: application/json

Body:
{
  "model": "claude-3-haiku-20240307",
  "max_tokens": 1024,
  "messages": [
    {"role": "user", "content": "Hello, Claude"}
  ]
}
```

#### 3.3 AI Manager (`src/ai/mod.rs`)

**Central AI Service**:

```rust
pub struct AIManager {
    providers: HashMap<String, Arc<dyn AIProvider>>,
    secrets: Arc<SecretsManager>,
    default_provider: String,
}

impl AIManager {
    pub fn new(secrets: Arc<SecretsManager>) -> Self;

    pub fn register_provider(
        &mut self,
        name: String,
        provider: Arc<dyn AIProvider>
    );

    pub async fn chat(
        &self,
        provider: Option<String>,
        messages: Vec<Message>,
        options: ChatOptions,
    ) -> Result<ChatResponse, AIError>;

    pub fn initialize_providers(&mut self) -> Result<(), AIError> {
        // Auto-detect available API keys and initialize providers
        if let Some(anthropic_key) = self.secrets.get("anthropic_api_key") {
            let claude = Arc::new(ClaudeProvider::new(anthropic_key));
            self.register_provider("claude".to_string(), claude);
            self.default_provider = "claude".to_string();
        }
        // Future: OpenAI, etc.
        Ok(())
    }
}
```

#### 3.4 Expose AI API to JavaScript (`src/js_engine.rs`)

**JavaScript API**:

```javascript
// Simple chat API
const response = await AI.chat("What is Rust?");

// With options
const response = await AI.chat("Explain recursion", {
  model: "claude-3-haiku-20240307",
  maxTokens: 500,
  temperature: 0.7,
  provider: "claude"  // Optional, uses default if not specified
});

// Response object
{
  content: "Recursion is a programming technique...",
  model: "claude-3-haiku-20240307",
  usage: {
    inputTokens: 15,
    outputTokens: 120
  },
  finishReason: "end_turn"
}

// Check available providers
AI.providers()  // Returns: ["claude"]

// Conversation with context
const response = await AI.chat([
  { role: "user", content: "Hello" },
  { role: "assistant", content: "Hi! How can I help?" },
  { role: "user", content: "What's the weather?" }
]);
```

**Rust Implementation**:

- Create `js_ai_chat()` async function
- Handle both string and array message formats
- Convert JavaScript objects to Rust types
- Return promise with response

---

### Phase 4: Editor Backend Endpoint

**Priority**: HIGH  
**Estimated Time**: 1-2 days

**Security Note**: This phase already uses secure patterns - `Secrets.exists()` for checking configuration and `AI.chat()` which handles secrets internally in Rust. No changes needed for security compliance.

#### 4.1 Implement `/api/ai-assistant` Endpoint

**Add to `scripts/feature_scripts/editor.js`**:

```javascript
// AI Assistant endpoint
async function apiAIAssistant(req) {
  try {
    // Validate request
    if (req.method !== "POST") {
      return Response.json({ error: "Method not allowed" }, { status: 405 });
    }

    const body = req.body;
    const prompt = body?.prompt;

    if (!prompt || typeof prompt !== "string") {
      return Response.json(
        { error: "Prompt is required and must be a string" },
        { status: 400 },
      );
    }

    if (prompt.length > 4000) {
      return Response.json(
        { error: "Prompt is too long (max 4000 characters)" },
        { status: 400 },
      );
    }

    // Check if AI API key is configured
    if (!Secrets.exists("anthropic_api_key")) {
      return Response.json(
        {
          error:
            "AI assistant is not configured. Please add 'anthropic_api_key' to secrets.",
          configured: false,
        },
        { status: 503 },
      );
    }

    // Build context from current editor state (future enhancement)
    let systemPrompt = `You are an AI assistant helping with JavaScript development in aiwebengine.
Keep your responses concise and practical. Focus on code examples when relevant.`;

    // Call AI API
    const response = await AI.chat(prompt, {
      model: "claude-3-haiku-20240307",
      maxTokens: 1024,
      temperature: 0.7,
      systemPrompt: systemPrompt,
    });

    // Return response
    return Response.json({
      response: response.content,
      timestamp: new Date().toISOString(),
      model: response.model,
      usage: response.usage,
    });
  } catch (error) {
    writeLog("AI Assistant error: " + error.message);

    // Provide helpful error messages
    let errorMessage = "An error occurred while processing your request.";

    if (error.message.includes("rate_limit")) {
      errorMessage = "Rate limit exceeded. Please try again in a moment.";
    } else if (error.message.includes("timeout")) {
      errorMessage = "Request timed out. Please try again.";
    } else if (error.message.includes("invalid_api_key")) {
      errorMessage = "Invalid API key. Please check your configuration.";
    }

    return Response.json(
      {
        error: errorMessage,
        details: error.message,
      },
      { status: 500 },
    );
  }
}

// Update init() to register the endpoint
function init(context) {
  writeLog("Initializing editor.js at " + new Date().toISOString());
  register("/editor", "serveEditor", "GET");
  register("/api/scripts", "apiListScripts", "GET");
  register("/api/scripts/*", "apiGetScript", "GET");
  register("/api/scripts/*", "apiSaveScript", "POST");
  register("/api/logs", "apiGetLogs", "GET");
  register("/api/assets", "apiGetAssets", "GET");
  register("/api/ai-assistant", "apiAIAssistant", "POST"); // NEW
  writeLog("Editor endpoints registered");
  return { success: true };
}
```

#### 4.2 Error Handling & User Feedback

**Handle various scenarios**:

- API key not configured → Clear error message with setup instructions
- Rate limits → Friendly retry message
- Network errors → Suggest checking connection
- Invalid prompts → Validation feedback
- Timeout → Retry suggestion

---

### Phase 5: Configuration & Setup

**Priority**: HIGH  
**Estimated Time**: 1 day

#### 5.1 Update Configuration Files

**Add to `config.example.yaml`**:

```yaml
# AI Configuration
ai:
  enabled: true
  default_provider: "claude"
  providers:
    claude:
      model: "claude-3-haiku-20240307"
      max_tokens: 1024
      temperature: 0.7
      timeout_ms: 30000

# Secrets (DO NOT commit actual values)
secrets:
  # Get your API key from: https://console.anthropic.com/
  anthropic_api_key: "${ANTHROPIC_API_KEY}" # Load from environment
  # Future providers:
  # openai_api_key: "${OPENAI_API_KEY}"
```

**Environment Variable Support**:

```bash
# .env.example
ANTHROPIC_API_KEY=sk-ant-api03-...
```

#### 5.2 Documentation

**Create `docs/solution-developers/AI_ASSISTANT.md`**:

- How to set up API keys
- How to use the AI assistant in the editor
- API reference for `AI.chat()` in scripts
- Rate limits and best practices
- Cost considerations
- Troubleshooting guide

**Update `docs/engine-administrators/CONFIGURATION.md`**:

- AI configuration options
- Secrets management setup
- Environment variable configuration
- Security considerations

---

### Phase 6: Testing & Quality Assurance

**Priority**: HIGH  
**Estimated Time**: 2-3 days

#### 6.1 Unit Tests

**Test Secrets Manager** (`tests/secrets.rs`):

```rust
#[test]
fn test_secrets_get_set() { }

#[test]
fn test_secrets_exists() { }

#[test]
fn test_secrets_list() { }

#[test]
fn test_secrets_from_env() { }
```

**Test HTTP Client** (`tests/http_client.rs`):

```rust
#[tokio::test]
async fn test_fetch_get() { }

#[tokio::test]
async fn test_fetch_post_json() { }

#[tokio::test]
async fn test_fetch_timeout() { }

#[tokio::test]
async fn test_fetch_invalid_url() { }
```

**Test AI Integration** (`tests/ai_integration.rs`):

```rust
#[tokio::test]
async fn test_claude_chat() { }

#[tokio::test]
async fn test_ai_manager() { }

#[tokio::test]
async fn test_multiple_messages() { }
```

#### 6.2 Integration Tests

**Test Editor Endpoint** (`tests/api_editor.rs`):

```rust
#[tokio::test]
async fn test_ai_assistant_endpoint() {
    // Test POST /api/ai-assistant with valid prompt
    // Verify response format
    // Test error cases (no API key, invalid prompt, etc.)
}

#[tokio::test]
async fn test_ai_assistant_rate_limiting() {
    // Test rate limiting behavior
}
```

#### 6.3 JavaScript API Tests

**Test in `scripts/test_scripts/test_ai.js`**:

```javascript
async function testAIChat(req) {
  const tests = [];

  // Test 1: Simple chat
  try {
    const response = await AI.chat("Say hello");
    tests.push({
      name: "Simple chat",
      passed: response.content && response.content.length > 0,
    });
  } catch (error) {
    tests.push({ name: "Simple chat", passed: false, error: error.message });
  }

  // Test 2: Chat with options
  // Test 3: Check providers
  // Test 4: Secrets integration

  return Response.json({ tests });
}
```

#### 6.4 Manual Testing Checklist

- [ ] Configure API key via environment variable
- [ ] Start server and access `/editor`
- [ ] Open AI assistant panel
- [ ] Submit simple prompt
- [ ] Verify response displays correctly
- [ ] Test with long prompt
- [ ] Test with empty prompt (should show error)
- [ ] Test without API key configured (should show helpful error)
- [ ] Test rate limiting
- [ ] Test network error handling
- [ ] Check logs for secret redaction
- [ ] Verify response time is acceptable

---

### Phase 7: Enhancements & Future Work

**Priority**: MEDIUM  
**Estimated Time**: Ongoing

#### 7.1 Context-Aware Assistance (Future)

**Provide editor context to AI**:

- Current script being edited
- Available functions and APIs
- Recent logs
- Available scripts and assets

```javascript
async function apiAIAssistant(req) {
  // ... existing code ...

  // Add context
  const context = {
    currentScript: getCurrentScript(), // If editing a script
    availableAPIs: listAvailableAPIs(),
    recentLogs: getRecentLogs(10),
  };

  const enhancedPrompt = `
Context: ${JSON.stringify(context, null, 2)}

User Question: ${prompt}
`;

  const response = await AI.chat(enhancedPrompt, options);
}
```

#### 7.2 Conversation History (Future)

**Store conversation history**:

- Keep last N messages in browser localStorage
- Send context with new prompts
- Clear history button

#### 7.3 Code Generation Improvements (Future)

**Special handling for code requests**:

- Detect when user asks for code
- Format code snippets nicely
- Provide "Insert into editor" button
- Syntax highlighting in responses

#### 7.4 Multiple Provider Support (Future)

**Add more providers**:

- OpenAI (GPT-4, GPT-3.5-turbo)
- Google (Gemini)
- Local models (Ollama)
- Provider selection in UI

#### 7.5 Cost Tracking & Monitoring (Future)

**Track usage and costs**:

```rust
pub struct UsageStats {
    pub total_requests: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub estimated_cost_usd: f64,
}
```

- Per-user tracking (if auth is enabled)
- Budget limits
- Usage dashboard in editor

#### 7.6 Streaming Responses (Future)

**Stream AI responses**:

- Use Server-Sent Events (SSE)
- Update UI as tokens arrive
- Better UX for long responses

---

## Dependencies

### New Rust Crates

Add to `Cargo.toml`:

```toml
[dependencies]
reqwest = { version = "0.11", features = ["json", "rustls-tls"] }
serde_json = "1.0"
tokio = { version = "1.35", features = ["full"] }
async-trait = "0.1"
```

### API Key Requirements

**Anthropic Claude API**:

- Sign up at: https://console.anthropic.com/
- Create API key
- Models available:
  - `claude-3-haiku-20240307` - Fast, cost-effective (recommended for start)
  - `claude-3-sonnet-20240229` - Balanced (future)
  - `claude-3-opus-20240229` - Most capable (future)

**Pricing (as of 2024)**:

- Claude Haiku: $0.25/M input tokens, $1.25/M output tokens
- Very affordable for development and small-scale use

---

## Security Considerations

### 1. Secrets Management

- ✅ Never log API keys
- ✅ Never expose keys in error messages
- ✅ Never return keys in API responses
- ✅ Store keys in environment variables or secure config
- ✅ Use `Secrets.get()` API for access control
- ✅ Implement audit logging for secret access

### 2. Input Validation

- ✅ Validate prompt length (max 4000 chars)
- ✅ Sanitize user input
- ✅ Rate limit API calls (per user, per IP)
- ✅ Prevent prompt injection attacks

### 3. HTTP Client Security

- ✅ Validate URLs (block local addresses)
- ✅ TLS/SSL certificate validation
- ✅ Timeout enforcement
- ✅ Response size limits
- ✅ Block private IP ranges

### 4. Error Handling

- ✅ Never expose internal errors to end users
- ✅ Log detailed errors server-side
- ✅ Return generic errors to clients
- ✅ Implement retry logic with backoff

---

## Testing Strategy

### Development Testing

1. **Local Testing**: Use personal API key during development
2. **Mock Responses**: Create mock AI provider for tests without API calls
3. **Integration Tests**: Use test API key with small quotas

### Mock Provider for Tests

```rust
pub struct MockAIProvider {
    responses: Vec<String>,
    current: AtomicUsize,
}

impl MockAIProvider {
    pub fn new(responses: Vec<String>) -> Self {
        Self {
            responses,
            current: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl AIProvider for MockAIProvider {
    async fn chat(
        &self,
        _messages: Vec<Message>,
        _options: ChatOptions,
    ) -> Result<ChatResponse, AIError> {
        let idx = self.current.fetch_add(1, Ordering::SeqCst);
        let content = self.responses
            .get(idx % self.responses.len())
            .cloned()
            .unwrap_or_else(|| "Mock response".to_string());

        Ok(ChatResponse {
            content,
            model: "mock-model".to_string(),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 20,
            },
            finish_reason: "stop".to_string(),
        })
    }

    fn name(&self) -> &str { "mock" }
    fn supports_streaming(&self) -> bool { false }
}
```

---

## Migration Path for Future Providers

### Adding a New Provider (e.g., OpenAI)

1. **Create provider implementation** (`src/ai/openai.rs`):

```rust
pub struct OpenAIProvider {
    http_client: Arc<HttpClient>,
    api_key: String,
    default_model: String,
}

#[async_trait]
impl AIProvider for OpenAIProvider {
    async fn chat(...) -> Result<ChatResponse, AIError> {
        // Implement OpenAI API calls
    }
}
```

2. **Register in AIManager**:

```rust
impl AIManager {
    pub fn initialize_providers(&mut self) -> Result<(), AIError> {
        // Claude
        if let Some(key) = self.secrets.get("anthropic_api_key") {
            self.register_provider("claude", ClaudeProvider::new(key));
        }

        // OpenAI
        if let Some(key) = self.secrets.get("openai_api_key") {
            self.register_provider("openai", OpenAIProvider::new(key));
        }

        Ok(())
    }
}
```

3. **Update configuration**:

```yaml
secrets:
  openai_api_key: "${OPENAI_API_KEY}"
```

4. **No JavaScript API changes needed** - `AI.chat()` works with all providers

---

## Success Criteria

### Phase 1-4 Complete When:

- ✅ Secrets can be loaded from configuration
- ✅ `Secrets.get()` works in JavaScript
- ✅ `fetch()` works for HTTPS requests
- ✅ `AI.chat()` returns responses from Claude
- ✅ `/api/ai-assistant` endpoint works in editor
- ✅ UI displays AI responses correctly
- ✅ Error handling works for all edge cases
- ✅ Tests pass
- ✅ Documentation is complete

### Production Ready When:

- ✅ All tests passing
- ✅ Security audit complete
- ✅ Rate limiting implemented
- ✅ Cost tracking available
- ✅ Monitoring and logging in place
- ✅ Documentation complete
- ✅ Example scripts available

---

## Timeline Estimate

| Phase                       | Duration       | Dependencies                 |
| --------------------------- | -------------- | ---------------------------- |
| Phase 1: Secrets Management | 2-3 days       | None                         |
| Phase 2: HTTP Client        | 3-4 days       | None (parallel with Phase 1) |
| Phase 3: AI Integration     | 3-4 days       | Phases 1 & 2                 |
| Phase 4: Editor Endpoint    | 1-2 days       | Phase 3                      |
| Phase 5: Configuration      | 1 day          | All above                    |
| Phase 6: Testing            | 2-3 days       | All above                    |
| **Total**                   | **12-17 days** |                              |

_Note: Phases 1 and 2 can be developed in parallel_

---

## Open Questions

1. **Rate Limiting**: Should we implement per-user limits, per-IP limits, or both?
2. **Cost Control**: Should we add budget limits? Daily/monthly caps?
3. **Model Selection**: Should users be able to choose models in the UI, or is this a config-only setting?
4. **Context Size**: How much context should we include in prompts? Current script? Recent logs?
5. **Conversation Memory**: Should we maintain conversation history? If yes, for how long?
6. **Caching**: Should we cache similar prompts to reduce API calls?

---

## References

- **Anthropic API Docs**: https://docs.anthropic.com/claude/reference/
- **Claude Models**: https://docs.anthropic.com/claude/docs/models-overview
- **Web Fetch API**: https://developer.mozilla.org/en-US/docs/Web/API/Fetch_API
- **Requirements**:
  - `docs/engine-contributors/planning/REQUIREMENTS.md` (REQ-JSAPI-007, REQ-JSAPI-008, REQ-SEC-005)
  - `docs/engine-contributors/planning/USE_CASES.md` (UC-504)
