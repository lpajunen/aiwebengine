# Example Scripts

This folder contains working example JavaScript scripts for aiwebengine.

## Quick Start

Deploy scripts using the deployer tool:

```bash
cargo run --bin deployer --uri "https://example.com/blog" --file "scripts/example_scripts/blog.js"
```

Or upload via the built-in editor at http://localhost:3000/editor

## Available Scripts

- **blog.js** - Sample blog with modern styling
- **feedback.js** - Interactive feedback form with GET/POST handling
- **graphql_subscription_demo.js** - GraphQL subscription example using Server-Sent Events (SSE)
- **graphql_ws_demo.js** - GraphQL subscription example using WebSocket (graphql-transport-ws protocol)
- **script_updates_demo.js** - Script update demonstration

## Documentation

For complete documentation, see:

- [Example Scripts Reference](../../docs/solution-developers/examples/index.md)
- [Deployer Tool Guide](../../docs/solution-developers/examples/deployer.md)

All documentation has been moved to `/docs/solution-developers/examples/` for better organization.
