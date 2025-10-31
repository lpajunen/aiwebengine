# Ideas for Enhancements

This document serves as a repository for ideas and suggestions to enhance the engine. Contributors are encouraged to propose new features, improvements, or changes that could benefit the engine and its users.

## Refactoring

The API should use as little namespace as possible from the actual scripts and have consistent naming conventions. The API structure should guarantee proper use of privileges and the structure is clear and easy to understand. There are system scripts that have access to all privileges, and user scripts that have limited access based on their assigned roles.

The goal is to make the API more consistent and easier to use. The following are some proposed changes:

codeStorage object

- getScript -> getItem (use similar api structure as localStorage)
- getScriptInitStatus -> getItemMetadata

routeRegistry object

- register -> addRoute

console object

- writeLog -> log

logHistory object

- getLogs
- getOtherLogs

assetStorage / assetRegistry object

- upsertAsset -> setAsset / publishAsset

secretStorage object

- upsertSecret -> setSecret

engine object

- checkDatabaseHealth -> isDatabaseHealthy

scriptStorage object

userStorage object

graphQLRegistry object

- registerGraphQLQuery -> addGraphQLQuery

identityStorage object

- addUserRole -> assignUserRole

init function

handler function

- req.auth

event function

## Needs thinking

Streams

- how to provide different values for different streams and users?

Change events

- for example new script created, script updated, script deleted
- for example new asset created, asset updated, asset deleted
- for example new user created, user updated, user deleted

- could there be a "event" function that is called with the event details as parameters?
- should there be a way to subscribe to specific events?

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

Security enhancements

- how to improve the security model for scripts and data access?
- security for business logic
- secure storage and logging of sensitive data
- audit trails for changes made by scripts
