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

| Document                                                          | Description                                           |
| ----------------------------------------------------------------- | ----------------------------------------------------- |
| [Solution Developer Guide](solution-developers/README.md)         | Main navigation hub and quick start guide             |
| [Your First Script](solution-developers/getting-started/01-first-script.md) | Create your first "Hello World" script      |
| [Working with Editor](solution-developers/getting-started/02-working-with-editor.md) | Use the web-based editor              |
| [Deployment Workflow](solution-developers/getting-started/03-deployment-workflow.md) | Deploy scripts to aiwebengine          |

#### Development Guides

| Document                                                              | Description                                                 |
| --------------------------------------------------------------------- | ----------------------------------------------------------- |
| [Script Development](solution-developers/guides/scripts.md)           | Complete guide to writing scripts                           |
| [Asset Management](solution-developers/guides/assets.md)              | Manage CSS, JS, images, and other assets                    |
| [Logging & Debugging](solution-developers/guides/logging.md)          | Write logs and debug your scripts                           |
| [AI-Assisted Development](solution-developers/guides/ai-development.md) | Use AI to generate and improve scripts                    |
| [Streaming](solution-developers/guides/streaming.md)                  | Server-sent events and streaming responses                  |
| [GraphQL Subscriptions](solution-developers/guides/graphql-subscriptions.md) | Real-time GraphQL subscription support           |

#### Tools

| Document                                                              | Description                                                 |
| --------------------------------------------------------------------- | ----------------------------------------------------------- |
| [Web Editor](solution-developers/tools/editor.md)                     | Comprehensive web editor guide                              |
| [Deployer CLI](solution-developers/tools/deployer.md)                 | Command-line deployment tool                                |
| [External Tools](solution-developers/tools/external-tools.md)         | VS Code, Git, CI/CD integration                             |

#### API Reference

| Document                                                              | Description                                                 |
| --------------------------------------------------------------------- | ----------------------------------------------------------- |
| [JavaScript APIs](solution-developers/reference/javascript-apis.md)   | Complete API reference for scripts                          |
| [Authentication API](solution-developers/reference/auth-api.md)        | User authentication and authorization APIs                  |

#### Examples

| Document                                                              | Description                                                 |
| --------------------------------------------------------------------- | ----------------------------------------------------------- |
| [Basic API Examples](solution-developers/examples/basic-api.md)       | RESTful API patterns and CRUD operations                    |
| [Forms & Data Handling](solution-developers/examples/forms-and-data.md) | Form submissions, validation, file uploads               |
| [Real-Time Features](solution-developers/examples/real-time-features.md) | Chat, notifications, live updates                        |
| [AI Integration](solution-developers/examples/ai-integration.md)      | AI-powered content generation and chatbots                  |
| [Example Scripts Index](solution-developers/examples/index.md)        | Guide to all example scripts                                |

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
| [Implementation Overview](engine-contributors/implementing/README.md)     | Navigation hub for implementation work     |
| [Development Roadmap](engine-contributors/implementing/ROADMAP.md)        | Prioritized development roadmap            |
| [Contributing Guide](engine-contributors/implementing/CONTRIBUTING.md)    | How to contribute features and improvements |
| [Development Guidelines](engine-contributors/implementing/DEVELOPMENT.md) | Core development guidelines and standards   |

**Implementation by Type:**

- **Features:** [features/](engine-contributors/implementing/features/) - New functional capabilities to build
- **Improvements:** [improvements/](engine-contributors/implementing/improvements/) - Quality, performance, and security enhancements
- **Guides:** [guides/](engine-contributors/implementing/guides/) - Generic implementation best practices

**📁 Location**: `docs/engine-contributors/implementing/`

---

## 🎯 Quick Start by Goal

### I want to...

**Build my first script**
→ Start with [Your First Script](solution-developers/getting-started/01-first-script.md)

**Build a web application**
→ Follow [Solution Developer Guide](solution-developers/README.md) and [Script Development](solution-developers/guides/scripts.md)

**Create real-time features**
→ Read [Real-Time Features Examples](solution-developers/examples/real-time-features.md) and [Streaming Guide](solution-developers/guides/streaming.md)

**Build APIs**
→ Check [Basic API Examples](solution-developers/examples/basic-api.md)

**Handle forms and data**
→ See [Forms & Data Handling](solution-developers/examples/forms-and-data.md)

**Integrate AI features**
→ Follow [AI Integration Examples](solution-developers/examples/ai-integration.md) and [AI-Assisted Development](solution-developers/guides/ai-development.md)

**Deploy to production**
→ Follow [Running Environments](engine-administrators/03-RUNNING-ENVIRONMENTS.md)

**Set up OAuth and user management**
→ See [Secrets and Security](engine-administrators/04-SECRETS-AND-SECURITY.md)

**Understand available APIs**
→ Check [JavaScript APIs](solution-developers/reference/javascript-apis.md)

**Use the web editor**
→ Read [Web Editor Guide](solution-developers/tools/editor.md)

**Deploy with CLI**
→ Use [Deployer CLI](solution-developers/tools/deployer.md)

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
│   ├── README.md                     # Main navigation hub and quick start
│   │
│   ├── getting-started/              # Tutorials for beginners
│   │   ├── 01-first-script.md        # Your first "Hello World" script
│   │   ├── 02-working-with-editor.md # Using the web editor
│   │   └── 03-deployment-workflow.md # Deploying your scripts
│   │
│   ├── guides/                       # Comprehensive topic guides
│   │   ├── scripts.md                # Script development guide
│   │   ├── assets.md                 # Asset management
│   │   ├── logging.md                # Logging and debugging
│   │   ├── ai-development.md         # AI-assisted development
│   │   ├── streaming.md              # Real-time streaming
│   │   └── graphql-subscriptions.md  # GraphQL subscriptions
│   │
│   ├── tools/                        # Development tools
│   │   ├── editor.md                 # Web editor comprehensive guide
│   │   ├── deployer.md               # Deployer CLI tool
│   │   └── external-tools.md         # VS Code, Git, CI/CD
│   │
│   ├── reference/                    # API reference documentation
│   │   ├── javascript-apis.md        # Complete JavaScript API reference
│   │   └── auth-api.md               # Authentication APIs
│   │
│   └── examples/                     # Practical examples
│       ├── index.md                  # Example scripts index
│       ├── deployer.md               # Deployer tool usage
│       ├── basic-api.md              # REST API patterns
│       ├── forms-and-data.md         # Form handling examples
│       ├── real-time-features.md     # Chat, notifications, streaming
│       └── ai-integration.md         # AI-powered features
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
2. Follow [Your First Script](solution-developers/getting-started/01-first-script.md) to create your first script
3. Learn [Working with Editor](solution-developers/getting-started/02-working-with-editor.md) for browser-based development
4. Check [Basic API Examples](solution-developers/examples/basic-api.md) for code patterns
5. Use [JavaScript APIs](solution-developers/reference/javascript-apis.md) as reference

### Scenario 2: Production Deployment

1. Review [Configuration](engine-administrators/02-CONFIGURATION.md) for environment setup
2. Set up OAuth following [Secrets and Security](engine-administrators/04-SECRETS-AND-SECURITY.md)
3. Deploy using [Running Environments](engine-administrators/03-RUNNING-ENVIRONMENTS.md)
4. Set up monitoring per [Monitoring and Maintenance](engine-administrators/05-MONITORING-AND-MAINTENANCE.md)
5. Keep [Quick Reference](engine-administrators/QUICK-REFERENCE.md) handy for commands

### Scenario 3: Building a Real-Time Application

1. Start with [Your First Script](solution-developers/getting-started/01-first-script.md)
2. Learn [Streaming](solution-developers/guides/streaming.md) for SSE features
3. Explore [Real-Time Features Examples](solution-developers/examples/real-time-features.md) for chat, notifications, dashboards
4. Optionally add [GraphQL Subscriptions](solution-developers/guides/graphql-subscriptions.md)

### Scenario 4: Adding AI Features

1. Review [AI Integration Examples](solution-developers/examples/ai-integration.md)
2. Follow [AI-Assisted Development](solution-developers/guides/ai-development.md) for using the built-in AI assistant
3. Set up API keys in [Secrets and Security](engine-administrators/04-SECRETS-AND-SECURITY.md)

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
