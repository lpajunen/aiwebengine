# Documentation Index

Welcome to the aiwebengine documentation! This index helps you find the right documentation for your role.

## 📖 Quick Navigation by Role

### 👥 Solution Developers

Building websites, APIs, web applications, and AI tools on aiwebengine.

| Document | Status | Description |
|----------|--------|-------------|
| [Getting Started](solution-developers/APP_DEVELOPMENT.md) | ✅ Current | Complete guide to building solutions with aiwebengine |
| [JavaScript APIs](solution-developers/javascript-apis.md) | ✅ Current | Available JavaScript APIs for solution development |
| [Authentication API](solution-developers/AUTH_JS_API.md) | ✅ Current | JavaScript authentication and authorization APIs |
| [Examples](solution-developers/examples.md) | ✅ Current | Code examples and common patterns |
| [Example Scripts Reference](solution-developers/examples/index.md) | ✅ Current | Guide to all example scripts in `/scripts/example_scripts/` |
| [Deployer Tool](solution-developers/examples/deployer.md) | ✅ Current | Deployer tool for rapid development |
| [GraphQL Subscriptions](solution-developers/graphql-subscriptions.md) | ✅ Current | Real-time GraphQL subscription support |
| [Streaming](solution-developers/streaming.md) | ✅ Current | Server-sent events and streaming responses |
| [Editor Guide](solution-developers/EDITOR_README.md) | ✅ Current | Using the built-in script editor |

**📁 Location**: `docs/solution-developers/`

---

### 🔧 Engine Administrators

Deploying, configuring, and managing aiwebengine instances.

| Document | Status | Description |
|----------|--------|-------------|
| [Docker Deployment](engine-administrators/DOCKER.md) | ✅ Current | Complete Docker deployment guide |
| [Configuration](engine-administrators/CONFIGURATION.md) | ✅ Current | Configuration options and environment variables |
| [Local Development](engine-administrators/local-development.md) | ✅ Current | Setting up local development environment |
| [Remote Development](engine-administrators/remote-development.md) | ✅ Current | Remote development and deployment strategies |
| [Docker Quick Reference](engine-administrators/DOCKER_QUICK_REFERENCE.md) | ✅ Current | Quick Docker commands and troubleshooting |
| [Docker Publishing Status](engine-administrators/DOCKER_PUBLISHING_STATUS.md) | ⚠️ Review | Docker image publishing information |

**📁 Location**: `docs/engine-administrators/`

---

### 🛠️ Engine Contributors

Contributing to aiwebengine core development.

#### Planning & Architecture

| Document | Status | Description |
|----------|--------|-------------|
| [Requirements](engine-contributors/planning/REQUIREMENTS.md) | ✅ Current | Core requirements and feature specifications |
| [Use Cases](engine-contributors/planning/USE_CASES.md) | ✅ Current | User scenarios and use case documentation |
| [Security Analysis](engine-contributors/planning/SECURITY_ANALYSIS.md) | ✅ Current | Security architecture and analysis |
| [Rust vs JS Security](engine-contributors/planning/RUST_VS_JS_SECURITY_ANALYSIS.md) | ✅ Current | Security considerations for language boundaries |
| [Customer Survey](engine-contributors/planning/CUSTOMER_SURVEY.md) | ⚠️ Review | Customer research and feedback |

**📁 Location**: `docs/engine-contributors/planning/`

#### Implementation & Development

| Document | Status | Description |
|----------|--------|-------------|
| [TODO / Roadmap](engine-contributors/implementing/TODO.md) | ✅ Current | Development roadmap and task tracking |
| [Development Guide](engine-contributors/implementing/DEVELOPMENT.md) | ✅ Current | Core development guidelines and setup |
| [Security TODO](engine-contributors/implementing/SECURITY_TODO.md) | ✅ Current | Security-related implementation tasks |
| [Auth TODO](engine-contributors/implementing/AUTH_TODO.md) | ⚠️ Review | Authentication implementation tasks |
| [Urgent TODO](engine-contributors/implementing/URGENT_TODO.md) | ⚠️ Review | High-priority implementation tasks |
| [Docs README](README.md) | ⚠️ Review | Documentation overview (needs update) |

**📁 Location**: `docs/engine-contributors/implementing/`

---

## 🗄️ Archived Documentation

Historical implementation notes, completed phases, and deprecated analysis documents.

Located in: `/archive/`

These documents provide historical context but are no longer actively maintained:

- Phase completion documents (PHASE_*.md)
- Implementation plans (completed features)
- Test fixes and analysis (historical)
- Requirements gap analysis (completed)

---

## 📋 Documentation Status Legend

| Symbol | Meaning |
|--------|---------|
| ✅ Current | Up-to-date and actively maintained |
| ⚠️ Review | May need updates or consolidation |
| 🗄️ Archived | Historical reference only |

---

## 🎯 Recommended Next Steps

### If you're new to aiwebengine

1. **Solution Developer?** Start with [Getting Started](solution-developers/APP_DEVELOPMENT.md)
2. **Administrator?** Check [Docker Deployment](engine-administrators/DOCKER.md)
3. **Contributor?** Read [Requirements](engine-contributors/planning/REQUIREMENTS.md) and [TODO](engine-contributors/implementing/TODO.md)

### Documentation Improvement Plan

The documentation is being reorganized into role-based folders:

```text
docs/
├── INDEX.md (this file)
├── solution-developers/     # For building on aiwebengine
├── engine-administrators/   # For deploying aiwebengine
└── engine-contributors/     # For developing aiwebengine
    ├── planning/           # Requirements, use cases, architecture
    └── implementing/       # TODO, development guides, tasks
```

---

## 📝 Contributing to Documentation

When adding or updating documentation:

1. **Choose the right location** based on the target audience
2. **Add status badge** at the top of the document
3. **Update this INDEX.md** with a link and description
4. **Move outdated docs** to `/archive/` with a note about why

For questions or suggestions about documentation structure, please open an issue.

---

**Last updated:** October 16, 2025
