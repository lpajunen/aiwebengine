# Archived Solution Developer Documentation

This directory contains historical solution developer documentation that has been superseded by the new restructured documentation.

## Archived on: October 24, 2025

## What Happened

The solution developer documentation was completely restructured to provide better organization and easier navigation. The new structure includes:

- **getting-started/** - Progressive tutorials for beginners
- **guides/** - Comprehensive topic guides
- **tools/** - Development tools documentation
- **reference/** - API reference documentation
- **examples/** - Practical code examples

## Archived Files

### Old Tutorial and Overview Files

- **APP_DEVELOPMENT.md** → Replaced by:
  - `solution-developers/README.md` (navigation hub)
  - `solution-developers/getting-started/01-first-script.md`
  - `solution-developers/guides/scripts.md`

- **examples.md** → Replaced by:
  - `solution-developers/examples/basic-api.md`
  - `solution-developers/examples/forms-and-data.md`
  - `solution-developers/examples/real-time-features.md`
  - `solution-developers/examples/ai-integration.md`

### Old Editor Documentation

- **EDITOR_README.md** → Replaced by:
  - `solution-developers/tools/editor.md` (comprehensive guide)
  - `solution-developers/getting-started/02-working-with-editor.md`

### Old AI Documentation

These individual AI guides were consolidated into a single comprehensive guide:

- **AI_ASSISTANT_GUIDE.md**
- **AI_PROMPT_GUIDE.md**
- **CLAUDE_MODEL_GUIDE.md**

→ All replaced by:
- `solution-developers/guides/ai-development.md` (comprehensive AI guide)
- `solution-developers/examples/ai-integration.md` (practical examples)

### Implementation/Fix Files

These were internal implementation notes and fixes:

- **AI_ASSISTANT_FIXES.md** - Historical bug fixes
- **AI_ASSISTANT_IMPLEMENTATION.md** - Implementation details
- **AI_BUTTON_FIX.md** - UI fix documentation
- **AI_MODEL_FIX.md** - Model fix documentation

These are preserved for historical reference but are no longer maintained.

## New Documentation Structure

To find what you're looking for in the new structure:

```
solution-developers/
├── README.md                         # Start here! Navigation hub
│
├── getting-started/                  # For beginners
│   ├── 01-first-script.md
│   ├── 02-working-with-editor.md
│   └── 03-deployment-workflow.md
│
├── guides/                           # Deep dives
│   ├── scripts.md
│   ├── assets.md
│   ├── logging.md
│   ├── ai-development.md
│   ├── streaming.md
│   └── graphql-subscriptions.md
│
├── tools/                            # Tool guides
│   ├── editor.md
│   ├── deployer.md
│   └── external-tools.md
│
├── reference/                        # API docs
│   ├── javascript-apis.md
│   └── auth-api.md
│
└── examples/                         # Code examples
    ├── basic-api.md
    ├── forms-and-data.md
    ├── real-time-features.md
    └── ai-integration.md
```

## Why Were These Archived?

The old documentation structure had several issues:

1. **No clear learning path** - Users didn't know where to start
2. **Information scattered** - Related content was in different files
3. **Redundancy** - Multiple files covering similar topics
4. **Inconsistent depth** - Some files too basic, others too advanced
5. **Poor navigation** - No central hub or clear structure

The new structure addresses all of these issues with:

- Progressive learning path (getting-started → guides → examples)
- Topic-based organization
- Comprehensive guides consolidating related information
- Clear separation of tutorials, guides, tools, reference, and examples
- Central navigation hub (README.md)

## Need the Old Content?

All files are preserved here exactly as they were. If you need information from the old documentation:

1. Check the new documentation first - it's more comprehensive
2. Use the mapping above to find where content moved
3. Only refer to these archived files if you can't find what you need in the new structure

## Reporting Issues

If you find information in these archived files that's missing from the new documentation:

1. Open an issue on GitHub
2. Reference both the old file and where it should be in the new structure
3. We'll add the missing information to the appropriate new location

---

**Note**: These files are preserved for historical reference only and are not maintained. Please use the current documentation in `/docs/solution-developers/`.
