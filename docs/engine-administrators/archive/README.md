# Archived Documentation

This directory contains previous versions of engine administrator documentation that have been replaced by the new task-based structure.

## What's Here

These documents have been consolidated and reorganized into the new documentation structure:

### Replaced by New Docs

| Old Document                 | Content Now In                                                                                                  |
| ---------------------------- | --------------------------------------------------------------------------------------------------------------- |
| `CONFIGURATION.md`           | [02-CONFIGURATION.md](../02-CONFIGURATION.md)                                                                   |
| `DOCKER.md`                  | [03-RUNNING-ENVIRONMENTS.md](../03-RUNNING-ENVIRONMENTS.md)                                                     |
| `DOCKER_QUICK_REFERENCE.md`  | [QUICK-REFERENCE.md](../QUICK-REFERENCE.md)                                                                     |
| `SECRETS_QUICK_REFERENCE.md` | [04-SECRETS-AND-SECURITY.md](../04-SECRETS-AND-SECURITY.md)                                                     |
| `PRODUCTION_CHECKLIST.md`    | [05-MONITORING-AND-MAINTENANCE.md](../05-MONITORING-AND-MAINTENANCE.md)                                         |
| `local-development.md`       | [01-GETTING-STARTED.md](../01-GETTING-STARTED.md) + [03-RUNNING-ENVIRONMENTS.md](../03-RUNNING-ENVIRONMENTS.md) |
| `HTTPS_SETUP.md`             | [03-RUNNING-ENVIRONMENTS.md](../03-RUNNING-ENVIRONMENTS.md)                                                     |
| `HTTPS_QUICK_START.md`       | [03-RUNNING-ENVIRONMENTS.md](../03-RUNNING-ENVIRONMENTS.md)                                                     |

### Internal/Outdated Docs

These documents are outdated or were internal development notes:

- `DOCKER_PUBLISHING_STATUS.md` - Internal status tracking
- `DOCUMENTATION_UPDATES_SECRETS.md` - Internal update notes
- `HTTPS_ARCHITECTURE.md` - Too detailed for admin docs (moved to contributors if needed)
- `JWT_SESSION_IMPLEMENTATION.md` - Implementation details (contributor doc)
- `remote-development.md` - Solution developer guide (wrong audience)

## New Documentation Structure

The new engine administrator documentation is organized by task:

1. **[README.md](../README.md)** - Overview and navigation
2. **[QUICK-REFERENCE.md](../QUICK-REFERENCE.md)** - Fast command lookup
3. **[01-GETTING-STARTED.md](../01-GETTING-STARTED.md)** - First-time setup
4. **[02-CONFIGURATION.md](../02-CONFIGURATION.md)** - Configuration reference
5. **[03-RUNNING-ENVIRONMENTS.md](../03-RUNNING-ENVIRONMENTS.md)** - Local/staging/production
6. **[04-SECRETS-AND-SECURITY.md](../04-SECRETS-AND-SECURITY.md)** - Secrets and OAuth
7. **[05-MONITORING-AND-MAINTENANCE.md](../05-MONITORING-AND-MAINTENANCE.md)** - Operations
8. **[06-TROUBLESHOOTING.md](../06-TROUBLESHOOTING.md)** - Problem solving

## Why the Change?

The previous documentation had several issues:

- **Too many files** (13 documents) with overlapping content
- **Unclear organization** - no obvious reading order
- **Outdated information** - references to YAML configs when using TOML
- **Mixed audiences** - admin and developer docs mixed together
- **Duplication** - same information in multiple places

The new structure:

- ✅ Clear, task-oriented organization
- ✅ Progressive learning path (01 → 06)
- ✅ Quick reference for experienced admins
- ✅ Updated and accurate information
- ✅ No duplication - each topic has one home
- ✅ Focused on administrator tasks

## Need Old Content?

If you need something specific from the old docs:

1. Check the mapping table above to find where it moved
2. If content is missing, these archived files are still here
3. Open an issue on GitHub if you think something important was lost

---

**Archived:** October 2025  
**Reason:** Documentation restructure for clarity and usability
