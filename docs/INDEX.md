# aiwebengine Documentation

Welcome to the aiwebengine documentation! This guide helps you find the right information based on your role and needs.

## 🚀 What is aiwebengine?

**aiwebengine** is a lightweight web application engine built in Rust that enables developers to create dynamic web content using JavaScript scripts. It provides a simple yet powerful platform for building web applications, APIs, and real-time interactive features.

### Key Features

- **JavaScript-based development**: Write handler functions in JavaScript to process HTTP requests
- **Real-time streaming**: Built-in Server-Sent Events (SSE) support for live updates
- **GraphQL support**: Query and subscription capabilities out of the box
- **Authentication & Authorization**: OAuth-based user management with role-based access control
- **Built-in editor**: Web-based script editor with syntax highlighting
- **Docker-ready**: Easy deployment with Docker and Docker Compose
- **Secrets management**: Secure handling of API keys and credentials

### Quick Example

```javascript
// Simple "Hello World" handler
function helloHandler(req) {
  return {
    status: 200,
    body: `Hello, ${req.query.name || "World"}!`,
    contentType: "text/plain",
  };
}

register("/hello", "helloHandler", "GET");
```

---

## 📖 Documentation by Role

### 👥 Solution Developers

**Building websites, APIs, web applications, and AI tools on aiwebengine.**

#### Getting Started

| Document                                                  | Description                                           |
| --------------------------------------------------------- | ----------------------------------------------------- |
| [App Development Guide](solution-developers/APP_DEVELOPMENT.md) | Complete guide to building solutions with aiwebengine |
| [JavaScript APIs](solution-developers/javascript-apis.md) | Available JavaScript APIs for solution development    |
| [Examples](solution-developers/examples.md)               | Code examples and common patterns                     |

#### Core Features

| Document                                                              | Description                                                 |
| --------------------------------------------------------------------- | ----------------------------------------------------------- |
| [Authentication API](solution-developers/AUTH_JS_API.md)              | JavaScript authentication and authorization APIs            |
| [Streaming](solution-developers/streaming.md)                         | Server-sent events and streaming responses                  |
| [GraphQL Subscriptions](solution-developers/graphql-subscriptions.md) | Real-time GraphQL subscription support                      |
| [Editor Guide](solution-developers/EDITOR_README.md)                  | Using the built-in script editor                            |
| [Example Scripts Reference](solution-developers/examples/index.md)    | Guide to all example scripts in `/scripts/example_scripts/` |
| [Deployer Tool](solution-developers/examples/deployer.md)             | Deployer tool for rapid development                         |

#### AI Integration

| Document                                                             | Description                                        |
| -------------------------------------------------------------------- | -------------------------------------------------- |
| [AI Assistant Guide](solution-developers/AI_ASSISTANT_GUIDE.md)     | Building AI-powered features into your applications |
| [AI Prompt Guide](solution-developers/AI_PROMPT_GUIDE.md)           | Best practices for AI prompt engineering           |
| [Claude Model Guide](solution-developers/CLAUDE_MODEL_GUIDE.md)     | Using Anthropic Claude models                      |

**📁 Location**: `docs/solution-developers/`

---

### 🔧 Engine Administrators

**Deploying, configuring, and managing aiwebengine instances.**

#### Step-by-Step Guides

| Document                                                            | Description                                     |
| ------------------------------------------------------------------- | ----------------------------------------------- |
| [01 - Getting Started](engine-administrators/01-GETTING-STARTED.md) | Prerequisites and first deployment              |
| [02 - Configuration](engine-administrators/02-CONFIGURATION.md)     | Configuration options and environment variables |
| [03 - Running Environments](engine-administrators/03-RUNNING-ENVIRONMENTS.md) | Local, staging, and production deployment |
| [04 - Secrets and Security](engine-administrators/04-SECRETS-AND-SECURITY.md) | OAuth setup, secrets management, bootstrap admins |
| [05 - Monitoring and Maintenance](engine-administrators/05-MONITORING-AND-MAINTENANCE.md) | Health checks, logs, backups, user management |
| [06 - Troubleshooting](engine-administrators/06-TROUBLESHOOTING.md) | Common problems and solutions                   |

#### Quick References

| Document                                                        | Description                           |
| --------------------------------------------------------------- | ------------------------------------- |
| [Quick Reference](engine-administrators/QUICK-REFERENCE.md)     | Fast lookup for commands and variables |
| [Administrator README](engine-administrators/README.md)         | Overview of administrator documentation |

**📁 Location**: `docs/engine-administrators/`

---

### 🛠️ Engine Contributors

**Contributing to aiwebengine core development.**

#### Planning & Architecture

| Document                                                     | Description                              |
| ------------------------------------------------------------ | ---------------------------------------- |
| [Requirements](engine-contributors/planning/REQUIREMENTS.md) | Core requirements and feature specifications |
| [Use Cases](engine-contributors/planning/USE_CASES.md)       | User scenarios and use case documentation |
| [Customer Survey](engine-contributors/planning/CUSTOMER_SURVEY.md) | Customer research and feedback    |

**📁 Location**: `docs/engine-contributors/planning/`

#### Implementation & Development

| Document                                                                  | Description                                |
| ------------------------------------------------------------------------- | ------------------------------------------ |
| [Development Guide](engine-contributors/implementing/DEVELOPMENT.md)      | Core development guidelines and setup      |
| [TODO / Roadmap](engine-contributors/implementing/TODO.md)                | Development roadmap and task tracking      |
| [Security TODO](engine-contributors/implementing/SECURITY_TODO.md)        | Security-related implementation tasks      |
| [Auth TODO](engine-contributors/implementing/AUTH_TODO.md)                | Authentication implementation tasks        |
| [Urgent TODO](engine-contributors/implementing/URGENT_TODO.md)            | High-priority implementation tasks         |
| [User Repository Implementation](engine-contributors/implementing/USER_REPOSITORY_IMPLEMENTATION.md) | User management system details |
| [User Repository Integration](engine-contributors/implementing/USER_REPOSITORY_INTEGRATION.md) | Integration guide for user management |
| [Auth Debugging Guide](engine-contributors/implementing/AUTH_DEBUGGING_GUIDE.md) | Debugging authentication issues |
| [Secret Management Summary](engine-contributors/implementing/SECRET_MANAGEMENT_SUMMARY.md) | Secrets management architecture |

**📁 Location**: `docs/engine-contributors/implementing/`

---

## 🎯 Quick Start by Goal

### I want to...

**Build a web application**
→ Start with [App Development Guide](solution-developers/APP_DEVELOPMENT.md)

**Create real-time features**
→ Read [Streaming Guide](solution-developers/streaming.md)

**Deploy to production**
→ Follow [Running Environments](engine-administrators/03-RUNNING-ENVIRONMENTS.md)

**Set up OAuth and user management**
→ See [Secrets and Security](engine-administrators/04-SECRETS-AND-SECURITY.md)

**Understand available APIs**
→ Check [JavaScript APIs](solution-developers/javascript-apis.md)

**See code examples**
→ Browse [Examples](solution-developers/examples.md)

**Troubleshoot issues**
→ Visit [Troubleshooting Guide](engine-administrators/06-TROUBLESHOOTING.md)

**Contribute to the project**
→ Read [Development Guide](engine-contributors/implementing/DEVELOPMENT.md) and [TODO](engine-contributors/implementing/TODO.md)

---

## 🗂️ Documentation Structure

```text
docs/
├── INDEX.md (this file)              # Main documentation index
│
├── solution-developers/              # For building ON aiwebengine
│   ├── APP_DEVELOPMENT.md            # Getting started building apps
│   ├── javascript-apis.md            # JavaScript API reference
│   ├── AUTH_JS_API.md                # Authentication APIs
│   ├── streaming.md                  # Real-time streaming
│   ├── graphql-subscriptions.md      # GraphQL subscriptions
│   ├── examples.md                   # Code examples
│   ├── EDITOR_README.md              # Built-in editor guide
│   ├── AI_ASSISTANT_GUIDE.md         # AI integration guide
│   └── examples/                     # Example scripts and tools
│       ├── index.md                  # Example scripts index
│       └── deployer.md               # Deployer tool guide
│
├── engine-administrators/            # For deploying aiwebengine
│   ├── README.md                     # Administrator overview
│   ├── 01-GETTING-STARTED.md         # Initial setup
│   ├── 02-CONFIGURATION.md           # Configuration guide
│   ├── 03-RUNNING-ENVIRONMENTS.md    # Deployment environments
│   ├── 04-SECRETS-AND-SECURITY.md    # Security and OAuth
│   ├── 05-MONITORING-AND-MAINTENANCE.md # Operations and user mgmt
│   ├── 06-TROUBLESHOOTING.md         # Problem solving
│   └── QUICK-REFERENCE.md            # Command cheat sheet
│
└── engine-contributors/              # For developing aiwebengine
    ├── planning/                     # Requirements and architecture
    │   ├── REQUIREMENTS.md           # Feature specifications
    │   ├── USE_CASES.md              # Use case documentation
    │   └── CUSTOMER_SURVEY.md        # Customer research
    │
    └── implementing/                 # Development guides
        ├── DEVELOPMENT.md            # Development setup
        ├── TODO.md                   # Roadmap and tasks
        ├── SECURITY_TODO.md          # Security tasks
        ├── AUTH_TODO.md              # Auth tasks
        └── [implementation docs]     # Various implementation guides
```

---

## 🗄️ Archived Documentation

Historical implementation notes, completed phases, and deprecated analysis documents are located in `/archive/`.

These documents provide historical context but are no longer actively maintained:

- Phase completion documents (PHASE_*.md)
- Implementation plans for completed features
- Test fixes and analysis (historical)
- Requirements gap analysis (completed)
- Bootstrap admin and manager UI docs (now integrated into admin docs)

---

## 💡 Common Scenarios

### Scenario 1: First-Time Local Development

1. Read [Getting Started](engine-administrators/01-GETTING-STARTED.md) to set up aiwebengine
2. Follow [App Development Guide](solution-developers/APP_DEVELOPMENT.md) to create your first script
3. Check [Examples](solution-developers/examples.md) for code patterns
4. Use [JavaScript APIs](solution-developers/javascript-apis.md) as reference

### Scenario 2: Production Deployment

1. Review [Configuration](engine-administrators/02-CONFIGURATION.md) for environment setup
2. Set up OAuth following [Secrets and Security](engine-administrators/04-SECRETS-AND-SECURITY.md)
3. Deploy using [Running Environments](engine-administrators/03-RUNNING-ENVIRONMENTS.md)
4. Set up monitoring per [Monitoring and Maintenance](engine-administrators/05-MONITORING-AND-MAINTENANCE.md)
5. Keep [Quick Reference](engine-administrators/QUICK-REFERENCE.md) handy for commands

### Scenario 3: Building a Real-Time Application

1. Start with [App Development Guide](solution-developers/APP_DEVELOPMENT.md)
2. Learn [Streaming](solution-developers/streaming.md) for SSE features
3. Optionally add [GraphQL Subscriptions](solution-developers/graphql-subscriptions.md)
4. Check [Example Scripts](solution-developers/examples/index.md) for real-time examples

### Scenario 4: Adding AI Features

1. Review [AI Assistant Guide](solution-developers/AI_ASSISTANT_GUIDE.md)
2. Follow [AI Prompt Guide](solution-developers/AI_PROMPT_GUIDE.md) for best practices
3. Check [Claude Model Guide](solution-developers/CLAUDE_MODEL_GUIDE.md) for specific models
4. Set up API keys in [Secrets and Security](engine-administrators/04-SECRETS-AND-SECURITY.md)

---

## 📝 Contributing to Documentation

When adding or updating documentation:

1. **Choose the right location** based on the target audience:
   - Solution developers: Building apps with aiwebengine
   - Engine administrators: Deploying and managing aiwebengine
   - Engine contributors: Developing aiwebengine core

2. **Follow naming conventions**:
   - Use descriptive names (e.g., `APP_DEVELOPMENT.md`, `CONFIGURATION.md`)
   - Administrator guides use numbered prefixes (01-, 02-, etc.) for sequential reading

3. **Update this INDEX.md** with a link and description

4. **Move outdated docs** to `/archive/` with a note about why

For questions or suggestions about documentation structure, please open an issue on GitHub.

---

## 🔗 External Resources

- **GitHub Repository**: [github.com/lpajunen/aiwebengine](https://github.com/lpajunen/aiwebengine)
- **Issues & Bug Reports**: [GitHub Issues](https://github.com/lpajunen/aiwebengine/issues)
- **License**: See [LICENSE](../LICENSE) file

---

**Last updated:** October 24, 2025
