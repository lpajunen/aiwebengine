# Ideas for Enhancements

This document serves as a repository for ideas and suggestions to enhance the engine. Contributors are encouraged to propose new features, improvements, or changes that could benefit the engine and its users.

## Refactoring

The API should use as little namespace as possible from the actual scripts and have consistent naming conventions. The API structure should guarantee proper use of privileges and the structure is clear and easy to understand. There are system scripts that have access to all privileges, and user scripts that have limited access based on their assigned roles.

The goal is to make the API more consistent and easier to use. The following are some proposed changes:

would it make sense to rename concept script to server script or to server code? at least in the documentation?

the engine is in horizontal lower layer providing services to the upper layer applications. the engine provides services such as storage, routing, logging, events, assets, secrets, identity management, scheduling, etc. the engine also provides an api for the upper layer applications to interact with these services. the engine also provides a way to run user scripts that can use these services and api.

the vertical upper layer applications are the ones that provide the user interface and business logic. these applications can be built using various frameworks and technologies. the engine should provide a way to integrate with these applications seamlessly.

in addition to horizontal lower layer, the engine also provides higher level vertical functions like logging, auditing, monitoring, metrics, tracing, etc. these functions are essential for the proper functioning of the engine and the upper layer applications.

## Needs thinking

Transactions or transactional storage operations

- for example, a way to group multiple storage operations into a single atomic operation
- how to handle rollbacks in case of failure?
- how to combine with streams, e.g. the user interface needs an initial list and guaranteed updates?

Security enhancements

- how to improve the security model for scripts and data access?
- security for business logic
- secure storage and logging of sensitive data
- audit trails for changes made by scripts

Monitoring and analytics

- how to monitor event chains and script performance?
- if a graphql query triggers another query or mutation, how to track the full chain of events for debugging and optimization purposes?
- add support for prometheus metrics collection from scripts and from engine
- add support for open telemetry tracing from scripts and from engine

- how to visualize and monitor business logic performance and errors?

AI understanding / context of scripts

- system prompt allow script generation AI to understand envige APIs
- how AI can know about other scripts and their functionality?

## URL structure ideas

- /auth/... for authentication and authorization related endpoints
  - login, logout, token refresh
  - unauthorized
  - implement OAuth2 / OIDC protocols
  - auth status check
- /engine/... for engine management endpoints
  - /engine/status for engine status and health checks
  - /engine/metrics for metrics
  - /engine/editor for editor operations
    - script management
    - asset management
    - solution secret management (addition to engine secrets in config)
    - log management
  - /engine/graphql for graphql test console
  - /engine/admin for admin operations
    - user management
  - /engine/docs for docs and api reference
  - /engine/cli/... for external cli (deployer) tool operations
  - /engine/api/... for engine related api endpoints
- /graphql/... for GraphQL related endpoints
  - implement GraphQL queries, mutations, subscriptions
- /mcp/... for Model-Context-Protocol related endpoints
  - implement MCP interactions

- everything else is available for user scripts
  - implement HTTP endpoints
  - complete responses and streams
- if there is no script registered for /, redirect to /engine/status or show a default welcome page
