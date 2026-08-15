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
// Simple "Hello World" handler using the unified context object
function helloHandler(context) {
  const req = context.request;

  return {
    status: 200,
    body: `Hello, ${req.query.name || "World"}!`,
    contentType: "text/plain",
  };
}

routeRegistry.registerRoute("/hello", "helloHandler", "GET");
```

---

## 📖 Documentation by Role

### Engine Administrators

**Deploying, configuring, and managing aiwebengine instances.**

#### Step-by-Step Guides

| Document                                                                                  | Description                                       |
| ----------------------------------------------------------------------------------------- | ------------------------------------------------- |
| [01 - Getting Started](engine-administrators/01-GETTING-STARTED.md)                       | Prerequisites and first deployment                |
| [02 - Configuration](engine-administrators/02-CONFIGURATION.md)                           | Configuration options and environment variables   |
| [03 - Running Environments](engine-administrators/03-RUNNING-ENVIRONMENTS.md)             | Local, staging, and production deployment         |
| [04 - Secrets and Security](engine-administrators/04-SECRETS-AND-SECURITY.md)             | OAuth setup, secrets management, bootstrap admins |
| [05 - Monitoring and Maintenance](engine-administrators/05-MONITORING-AND-MAINTENANCE.md) | Health checks, logs, backups, user management     |
| [06 - Troubleshooting](engine-administrators/06-TROUBLESHOOTING.md)                       | Common problems and solutions                     |

#### Quick References

| Document                                                    | Description                             |
| ----------------------------------------------------------- | --------------------------------------- |
| [Quick Reference](engine-administrators/QUICK-REFERENCE.md) | Fast lookup for commands and variables  |
| [Administrator README](engine-administrators/README.md)     | Overview of administrator documentation |

**📁 Location**: `docs/engine-administrators/`

---

### 🛠️ Engine Contributors

**Contributing to aiwebengine core development.**

#### Planning & Architecture

| Document                                                           | Description                                  |
| ------------------------------------------------------------------ | -------------------------------------------- |
| [Requirements](engine-contributors/planning/REQUIREMENTS.md)       | Core requirements and feature specifications |
| [Use Cases](engine-contributors/planning/USE_CASES.md)             | User scenarios and use case documentation    |
| [Customer Survey](engine-contributors/planning/CUSTOMER_SURVEY.md) | Customer research and feedback               |

**📁 Location**: `docs/engine-contributors/planning/`

#### Implementation & Development

| Document                                                                  | Description                                 |
| ------------------------------------------------------------------------- | ------------------------------------------- |
| [Implementation Overview](engine-contributors/implementing/README.md)     | Navigation hub for implementation work      |
| [Development Roadmap](engine-contributors/implementing/ROADMAP.md)        | Prioritized development roadmap             |
| [Contributing Guide](engine-contributors/implementing/CONTRIBUTING.md)    | How to contribute features and improvements |
| [Development Guidelines](engine-contributors/implementing/DEVELOPMENT.md) | Core development guidelines and standards   |

**Implementation by Type:**

- **Features:** [features/](engine-contributors/implementing/features/) - New functional capabilities to build
- **Improvements:** [improvements/](engine-contributors/implementing/improvements/) - Quality, performance, and security enhancements
- **Guides:** [guides/](engine-contributors/implementing/guides/) - Generic implementation best practices

**📁 Location**: `docs/engine-contributors/implementing/`

---

## 🎯 Quick Start by Goal

### I Want To

**Deploy to production**
→ Follow [Running Environments](engine-administrators/03-RUNNING-ENVIRONMENTS.md)

**Set up OAuth and user management**
→ See [Secrets and Security](engine-administrators/04-SECRETS-AND-SECURITY.md)

**Troubleshoot issues**
→ Visit [Troubleshooting Guide](engine-administrators/06-TROUBLESHOOTING.md)

**Contribute to the project**
→ Read [Development Guide](engine-contributors/implementing/DEVELOPMENT.md) and [Roadmap](engine-contributors/implementing/ROADMAP.md)

---

## 🗂️ Documentation Structure

```text
docs/
├── INDEX.md (this file)              # Main documentation index
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
        ├── README.md                 # Implementation overview
        ├── DEVELOPMENT.md            # Development setup
        ├── CONTRIBUTING.md           # Contribution guide
        ├── ROADMAP.md                # Roadmap and tasks
        ├── AUTH_DEBUGGING_GUIDE.md   # Auth debugging
        ├── features/                 # New capabilities to build
        ├── improvements/             # Quality/perf/security work
        └── guides/                   # Implementation best practices
```

---

## 💡 Common Scenarios

### Scenario 1: First-Time Local Development

1. Read [Getting Started](engine-administrators/01-GETTING-STARTED.md) to set up aiwebengine
2. Install a script with `POST /engine/upsert_script` (see [Script Management](engine-administrators/QUICK-REFERENCE.md))
3. Review the documentation for available JavaScript APIs

### Scenario 2: Production Deployment

1. Review [Configuration](engine-administrators/02-CONFIGURATION.md) for environment setup
2. Set up OAuth following [Secrets and Security](engine-administrators/04-SECRETS-AND-SECURITY.md)
3. Deploy using [Running Environments](engine-administrators/03-RUNNING-ENVIRONMENTS.md)
4. Set up monitoring per [Monitoring and Maintenance](engine-administrators/05-MONITORING-AND-MAINTENANCE.md)
5. Keep [Quick Reference](engine-administrators/QUICK-REFERENCE.md) handy for commands

### Scenario 3: Building a Real-Time Application

1. Start by creating a basic script in the web editor
2. Explore the example scripts for streaming and real-time features
3. Use Server-Sent Events (SSE) for live updates
4. Optionally add GraphQL subscriptions for bidirectional communication

### Scenario 4: Adding AI Features

1. Use the built-in AI assistant in the web editor
2. Set up API keys in [Secrets and Security](engine-administrators/04-SECRETS-AND-SECURITY.md)
3. Explore AI integration examples in the example scripts

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

**Last updated:** July 29, 2026
