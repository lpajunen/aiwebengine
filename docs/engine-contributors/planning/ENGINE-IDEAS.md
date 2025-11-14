# Ideas for Enhancements

This document serves as a repository for ideas and suggestions to enhance the engine. Contributors are encouraged to propose new features, improvements, or changes that could benefit the engine and its users.

## Refactoring

The API should use as little namespace as possible from the actual scripts and have consistent naming conventions. The API structure should guarantee proper use of privileges and the structure is clear and easy to understand. There are system scripts that have access to all privileges, and user scripts that have limited access based on their assigned roles.

The goal is to make the API more consistent and easier to use. The following are some proposed changes:

would it make sense to rename concept script to server script or to server code? at least in the documentation?

the engine is in horizontal lower layer providing services to the upper layer applications. the engine provides services such as storage, routing, logging, events, assets, secrets, identity management, scheduling, etc. the engine also provides an api for the upper layer applications to interact with these services. the engine also provides a way to run user scripts that can use these services and api.

the vertical upper layer applications are the ones that provide the user interface and business logic. these applications can be built using various frameworks and technologies. the engine should provide a way to integrate with these applications seamlessly.

in addition to horizontal lower layer, the engine also provides higher level vertical functions like logging, auditing, monitoring, metrics, tracing, etc. these functions are essential for the proper functioning of the engine and the upper layer applications.

### Engine API Changes

codeStorage object

- getScript -> value | null getItem(key) (use similar api structure as localStorage)
- upsertScript -> bool setItem(key, value)
- deleteScript -> bool removeItem(key)
- listScripts -> [key] listKeys()
- getScriptInitStatus -> {} | null getMetadata(key)

- if editor role, owned scripts only
- if admin role, all scripts

routeRegistry object

- register -> registerRoute(path, method, handler)
- None -> unregisterRoute(path, method)
- registerWebStream -> registerStreamRoute(path, connectionHandler | null)
- None -> unregisterStreamRoute(path)
- sendStreamMessageToPath -> sendStreamMessage(path, message, filterCriteria | null)
- sendStreamMessageToConnection -> None
- listRoutes -> [{path, method, handler | asset name}] listRoutes()
- registerPublicAsset -> registerAssetRoute(path, assetName)
- None -> unregisterAssetRoute(path)

- connectionHandler gets req object as parameter and returns filterCriteria for that connection

- all scripts owned by editors and admins can list all routes and register new routes
- unregisterRoute, unregisterStreamRoute, sendStreamMessage only for scripts where they were registered or owned by admin

console object (implemented)

- writeLog -> log

logStorage object

- listLogs -> [logDetails] listLogs(key)

- editors can list logs for their own scripts
- admins can list logs for all scripts

eventRegistry object

- None -> registerEvent(eventType, handler)
- None -> unregisterEvent(eventType)
- None -> dispatchEvent(eventType, eventData)

assetStorage / assetRegistry object

- upsertAsset -> setAsset

secretStorage object

- upsertSecret -> setSecret

engine object

- checkDatabaseHealth -> isDatabaseHealthy

sharedStorage object

userStorage object (personalStorage)

graphQLRegistry object

- registerGraphQLQuery -> registerQuery(name, sdl, handler)
- registerGraphQLMutation -> registerMutation(name, sdl, handler)
- registerGraphQLSubscription -> registerSubscription(name, sdl, handler, connectionHandler | null)
- None -> unregisterQuery(name)
- None -> unregisterMutation(name)
- None -> unregisterSubscription(name)
- sendSubscriptionMessageToConnections -> sendSubscriptionMessage(name, message, filterCriteria | null)
- executeGraphQL -> executeGraphQL(query, variables | null)

identityStorage object

- addUserRole -> assignUserRole

timerEventService object (scheduler)

- None -> registerSingleShotTimer(name, delayMs, handler)
- None -> registerRecurringTimer(name, intervalMs, handler)
- None -> unregisterTimer(name)

### Script API Changes

init function

- returns nothing

handler function

- req.auth
- req.params
- return value or {statusCode, headers, body}

connectionHandler function

- req.auth
- req.params
- return filterCriteria | null

event function

- eventType
- eventData

## Needs thinking

Streams

- how to provide different values for different streams and users?
- now filtercriteria is specified in http url when connecting the stream. it should be fully customizable in script side in connectionHandler function. this allows hiding this logic from the clients.

Change events

- streams and GraphQL subscriptions are for external clients to get real-time updates. however, there could be a need for internal event system for scripts to communicate with each other based on certain events happening in the system.

- for example new script created, script updated, script deleted
- for example new asset created, asset updated, asset deleted
- for example new user created, user updated, user deleted

- for example, i create a script for handling chat groups. there could be a separate script for handling user presence. when a user joins or leaves a chat group, the chat group script needs to be notified about this event. how to implement this event system? also when a new message is created, the chat group script needs to be notified about this event. e.g. multiple scripts need to communicate with each other based on events.

- could there be a "event" function that is called with the event details as parameters?
- should there be a way to subscribe to specific events?
- multiple external clients can subscribe to the same stream. however, if multiple scripts wants to subscribe to the same stream, how to handle that? is the separate event system needed?

Transactions or transactional storage operations

- for example, a way to group multiple storage operations into a single atomic operation
- how to handle rollbacks in case of failure?
- how to combine with streams, e.g. the user interface needs an initial list and guaranteed updates?

Assets

- what is their role and best way to manage them?
- favicons, css files, images, fonts, etc.

Composition of scripts

- for example, a way to include or import functionality from one script into another
- how to make a generic top bar or bottom bar for all user interfaces? how to provide custom components that can be reused across different scripts?

Ticker service / background jobs

- how to implement a ticker service that can trigger events at regular intervals?
- how to implement long running background jobs that do not have direct user interaction?

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
