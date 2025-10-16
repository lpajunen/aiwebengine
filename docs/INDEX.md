# Documentation Index

Welcome to the aiwebengine documentation! This index helps you find the right documentation for your role.

## 📖 Quick Navigation by Role

### 👥 Solution Developers

Building websites, APIs, web applications, and AI tools on aiwebengine.

| Document | Status | Description |
|----------|--------|-------------|
| [Getting Started](APP_DEVELOPMENT.md) | ✅ Current | Complete guide to building solutions with aiwebengine |
| [JavaScript APIs](javascript-apis.md) | ✅ Current | Available JavaScript APIs for solution development |
| [Authentication API](AUTH_JS_API.md) | ✅ Current | JavaScript authentication and authorization APIs |
| [Examples](examples.md) | ✅ Current | Code examples and common patterns |
| [GraphQL Subscriptions](graphql-subscriptions.md) | ✅ Current | Real-time GraphQL subscription support |
| [Streaming](streaming.md) | ✅ Current | Server-sent events and streaming responses |
| [Editor Guide](EDITOR_README.md) | ✅ Current | Using the built-in script editor |

**📁 Suggested location**: `docs/solution-developers/`

---

### 🔧 Engine Administrators

Deploying, configuring, and managing aiwebengine instances.

| Document | Status | Description |
|----------|--------|-------------|
| [Docker Deployment](DOCKER.md) | ✅ Current | Complete Docker deployment guide |
| [Configuration](CONFIGURATION.md) | ✅ Current | Configuration options and environment variables |
| [Local Development](local-development.md) | ✅ Current | Setting up local development environment |
| [Remote Development](remote-development.md) | ✅ Current | Remote development and deployment strategies |
| [Docker Quick Reference](../DOCKER_QUICK_REFERENCE.md) | ✅ Current | Quick Docker commands and troubleshooting |
| [Docker Publishing Status](../DOCKER_PUBLISHING_STATUS.md) | ⚠️ Review | Docker image publishing information |

**📁 Suggested location**: `docs/engine-administrators/`

---

### 🛠️ Engine Contributors

Contributing to aiwebengine core development.

#### Planning & Architecture

| Document | Status | Description |
|----------|--------|-------------|
| [Requirements](../REQUIREMENTS.md) | ✅ Current | Core requirements and feature specifications |
| [Use Cases](../USE_CASES.md) | ✅ Current | User scenarios and use case documentation |
| [Security Analysis](../SECURITY_ANALYSIS.md) | ✅ Current | Security architecture and analysis |
| [Rust vs JS Security](../RUST_VS_JS_SECURITY_ANALYSIS.md) | ✅ Current | Security considerations for language boundaries |
| [Customer Survey](../CUSTOMER_SURVEY.md) | ⚠️ Review | Customer research and feedback |

**📁 Suggested location**: `docs/engine-contributors/planning/`

#### Implementation & Development

| Document | Status | Description |
|----------|--------|-------------|
| [TODO / Roadmap](../TODO.md) | ✅ Current | Development roadmap and task tracking |
| [Development Guide](../DEVELOPMENT.md) | ✅ Current | Core development guidelines and setup |
| [Security TODO](../SECURITY_TODO.md) | ✅ Current | Security-related implementation tasks |
| [Auth TODO](../AUTH_TODO.md) | ⚠️ Review | Authentication implementation tasks |
| [Urgent TODO](../URGENT_TODO.md) | ⚠️ Review | High-priority implementation tasks |
| [Docs README](README.md) | ⚠️ Review | Documentation overview (needs update) |

**📁 Suggested location**: `docs/engine-contributors/implementing/`

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

1. **Solution Developer?** Start with [Getting Started](APP_DEVELOPMENT.md)
2. **Administrator?** Check [Docker Deployment](DOCKER.md)
3. **Contributor?** Read [Requirements](../REQUIREMENTS.md) and [TODO](../TODO.md)

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
