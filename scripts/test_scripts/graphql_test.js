// Test GraphQL registrations
registerGraphQLQuery("hello", "type Query { hello: String }", "helloResolver");
registerGraphQLMutation(
  "createUser",
  "type Mutation { createUser(name: String!): String }",
  "createUserResolver",
);
registerGraphQLSubscription(
  "userUpdates",
  "type Subscription { userUpdates: String }",
  "userUpdatesResolver",
);

// Simple resolvers (for testing)
function helloResolver() {
  return "Hello from JavaScript!";
}

function createUserResolver(args) {
  return "Created user: " + args.name;
}

function userUpdatesResolver() {
  writeLog("User subscribed to userUpdates");
  return "User updates subscription initialized";
}

// Add a mutation to trigger user updates
registerGraphQLMutation(
  "triggerUserUpdate",
  "type Mutation { triggerUserUpdate(userId: String!): String }",
  "triggerUserUpdateResolver",
);

function triggerUserUpdateResolver(args) {
  const updateData = {
    userId: args.userId,
    action: "profile_updated",
    timestamp: new Date().toISOString(),
  };

  writeLog(`Triggering user update for: ${args.userId}`);
  sendSubscriptionMessage("userUpdates", JSON.stringify(updateData));

  return `User update triggered for ${args.userId}`;
}
