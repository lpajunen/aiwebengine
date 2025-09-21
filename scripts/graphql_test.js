// Test GraphQL registrations
registerGraphQLQuery("hello", "type Query { hello: String }", "helloResolver");
registerGraphQLMutation("createUser", "type Mutation { createUser(name: String!): String }", "createUserResolver");
registerGraphQLSubscription("userUpdates", "type Subscription { userUpdates: String }", "userUpdatesResolver");

// Simple resolvers (for testing)
function helloResolver() {
    return "Hello from JavaScript!";
}

function createUserResolver(args) {
    return "Created user: " + args.name;
}

function userUpdatesResolver() {
    return "User update notification";
}