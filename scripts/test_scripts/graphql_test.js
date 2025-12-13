// Test GraphQL registrations
graphQLRegistry.registerQuery(
  "hello",
  "type Query { hello: String }",
  "helloResolver",
  "external",
);
graphQLRegistry.registerMutation(
  "createUser",
  "type Mutation { createUser(name: String!): String }",
  "createUserResolver",
  "external",
);
graphQLRegistry.registerSubscription(
  "userUpdates",
  "type Subscription { userUpdates: String }",
  "userUpdatesResolver",
  "external",
);

// Simple resolvers (for testing)
function helloResolver() {
  return "Hello from JavaScript!";
}

function createUserResolver(args) {
  return "Created user: " + args.name;
}

function userUpdatesResolver() {
  console.log("User subscribed to userUpdates");
  return "User updates subscription initialized";
}

// Add a mutation to trigger user updates
graphQLRegistry.registerMutation(
  "triggerUserUpdate",
  "type Mutation { triggerUserUpdate(userId: String!): String }",
  "triggerUserUpdateResolver",
  "external",
);

function triggerUserUpdateResolver(args) {
  const updateData = {
    userId: args.userId,
    action: "profile_updated",
    timestamp: new Date().toISOString(),
  };

  console.log(`Triggering user update for: ${args.userId}`);
  graphQLRegistry.sendSubscriptionMessage(
    "userUpdates",
    JSON.stringify(updateData),
  );

  return `User update triggered for ${args.userId}`;
}
