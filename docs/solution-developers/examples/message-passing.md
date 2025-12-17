# Message Passing Examples

This guide demonstrates practical patterns for using the dispatcher to enable inter-script communication in aiwebengine.

## Overview

The message dispatcher provides a publish-subscribe system that allows scripts to communicate without tight coupling. Publishers send messages without knowing who's listening, and subscribers process messages without knowing who sent them.

**Key Benefits:**

- **Decoupling**: Scripts don't depend on each other's implementation details
- **Scalability**: Add new listeners without modifying publishers
- **Maintainability**: Changes to one script don't ripple through others
- **Flexibility**: Multiple scripts can react to the same events

## Example 1: User Registration Workflow

This example shows how multiple scripts coordinate when a new user registers.

### Publisher: User Management Script

```javascript
/**
 * user-service.js - Handles user account creation
 */

function registerUser(context) {
  const req = context.request;
  const { username, email, password } = req.form;

  // Validate input
  if (!username || !email || !password) {
    return {
      status: 400,
      body: JSON.stringify({ error: "Missing required fields" }),
      contentType: "application/json",
    };
  }

  // Create user account
  const userId = generateUserId();
  const user = {
    id: userId,
    username: username,
    email: email,
    createdAt: new Date().toISOString(),
    verified: false,
  };

  // Store in database
  sharedStorage.setItem(`user:${userId}`, JSON.stringify(user));

  // Broadcast user creation event
  const eventData = JSON.stringify({
    userId: userId,
    username: username,
    email: email,
    timestamp: user.createdAt,
  });

  const result = dispatcher.sendMessage("user.registered", eventData);
  console.log(`User registration broadcast: ${result}`);

  return {
    status: 201,
    body: JSON.stringify({
      success: true,
      userId: userId,
      message: "User registered successfully",
    }),
    contentType: "application/json",
  };
}

function generateUserId() {
  const counter = parseInt(sharedStorage.getItem("userIdCounter") || "1000");
  sharedStorage.setItem("userIdCounter", String(counter + 1));
  return `user_${counter}`;
}

function init(context) {
  routeRegistry.registerRoute("/api/users/register", "registerUser", "POST");
  return { success: true };
}
```

### Subscriber: Email Notification Script

```javascript
/**
 * email-notifications.js - Sends welcome emails to new users
 */

function sendWelcomeEmail(context) {
  const userData = context.messageData;

  console.log(`Sending welcome email to ${userData.email}`);

  // Simulate email sending
  const emailContent = `
    Welcome ${userData.username}!
    
    Your account has been created successfully.
    User ID: ${userData.userId}
    Registered: ${userData.timestamp}
    
    Please verify your email address to activate your account.
  `;

  // In a real application, you would use fetch() to call an email API
  // fetch("https://api.emailservice.com/send", { ... })

  console.log(`Welcome email sent to ${userData.email}`);
}

function init(context) {
  // Listen for user registration events
  dispatcher.registerListener("user.registered", "sendWelcomeEmail");
  return { success: true };
}
```

### Subscriber: Analytics Script

```javascript
/**
 * analytics.js - Tracks user metrics and activity
 */

function trackUserRegistration(context) {
  const userData = context.messageData;

  // Update registration metrics
  const today = new Date().toISOString().split("T")[0];
  const key = `metrics:registrations:${today}`;
  const count = parseInt(sharedStorage.getItem(key) || "0");
  sharedStorage.setItem(key, String(count + 1));

  // Track total users
  const totalUsers = parseInt(
    sharedStorage.getItem("metrics:totalUsers") || "0",
  );
  sharedStorage.setItem("metrics:totalUsers", String(totalUsers + 1));

  console.log(
    `Analytics: User registration tracked. Total users: ${totalUsers + 1}`,
  );

  // Broadcast to analytics dashboard stream
  routeRegistry.sendStreamMessage("/analytics/dashboard", {
    type: "user_registered",
    userId: userData.userId,
    timestamp: userData.timestamp,
    totalUsers: totalUsers + 1,
  });
}

function init(context) {
  routeRegistry.registerStreamRoute("/analytics/dashboard");
  dispatcher.registerListener("user.registered", "trackUserRegistration");
  return { success: true };
}
```

### Subscriber: User Verification Script

```javascript
/**
 * verification.js - Manages email verification process
 */

function createVerificationToken(context) {
  const userData = context.messageData;

  // Generate verification token
  const token = generateToken(userData.userId);
  const expiry = new Date(Date.now() + 24 * 60 * 60 * 1000).toISOString(); // 24 hours

  // Store token
  sharedStorage.setItem(
    `verification:${token}`,
    JSON.stringify({
      userId: userData.userId,
      email: userData.email,
      createdAt: new Date().toISOString(),
      expiresAt: expiry,
    }),
  );

  console.log(`Verification token created for user ${userData.userId}`);

  // Send verification email (would integrate with email service)
  const verificationLink = `https://example.com/verify?token=${token}`;
  console.log(`Verification link: ${verificationLink}`);
}

function generateToken(userId) {
  const timestamp = Date.now();
  const random = Math.floor(Math.random() * 1000000);
  return `${userId}_${timestamp}_${random}`;
}

function init(context) {
  dispatcher.registerListener("user.registered", "createVerificationToken");
  return { success: true };
}
```

## Example 2: Real-Time Chat with Notifications

This example demonstrates event-driven chat with presence tracking and notifications.

### Chat Message Script

```javascript
/**
 * chat.js - Handles chat messages and coordination
 */

function sendChatMessage(context) {
  const req = context.request;
  const { roomId, userId, username, message } = req.form;

  if (!roomId || !userId || !message) {
    return {
      status: 400,
      body: JSON.stringify({ error: "Missing required fields" }),
      contentType: "application/json",
    };
  }

  const messageData = {
    messageId: generateMessageId(),
    roomId: roomId,
    userId: userId,
    username: username,
    message: message,
    timestamp: new Date().toISOString(),
  };

  // Store message
  sharedStorage.setItem(
    `message:${messageData.messageId}`,
    JSON.stringify(messageData),
  );

  // Broadcast to chat stream
  routeRegistry.sendStreamMessageFiltered(
    "/chat/stream",
    {
      type: "chat_message",
      ...messageData,
    },
    JSON.stringify({ roomId: roomId }),
  );

  // Dispatch internal event for other scripts
  dispatcher.sendMessage("chat.message.sent", JSON.stringify(messageData));

  return {
    status: 200,
    body: JSON.stringify({ success: true, messageId: messageData.messageId }),
    contentType: "application/json",
  };
}

function generateMessageId() {
  return `msg_${Date.now()}_${Math.floor(Math.random() * 10000)}`;
}

function init(context) {
  routeRegistry.registerStreamRoute("/chat/stream");
  routeRegistry.registerRoute("/chat/send", "sendChatMessage", "POST");
  return { success: true };
}
```

### Notification Script

```javascript
/**
 * notifications.js - Sends notifications for @mentions
 */

function handleChatMessage(context) {
  const messageData = context.messageData;

  // Check for @mentions in the message
  const mentions = extractMentions(messageData.message);

  if (mentions.length > 0) {
    console.log(
      `Found ${mentions.length} mentions in message ${messageData.messageId}`,
    );

    // Send notifications to mentioned users
    mentions.forEach((mentionedUser) => {
      const notification = {
        type: "mention",
        messageId: messageData.messageId,
        roomId: messageData.roomId,
        from: messageData.username,
        mentionedUser: mentionedUser,
        message: messageData.message,
        timestamp: new Date().toISOString(),
      };

      // Send to user's notification stream
      routeRegistry.sendStreamMessageFiltered(
        "/notifications",
        notification,
        JSON.stringify({ userId: mentionedUser }),
      );

      console.log(`Notification sent to @${mentionedUser}`);
    });
  }
}

function extractMentions(message) {
  const mentionRegex = /@(\w+)/g;
  const mentions = [];
  let match;

  while ((match = mentionRegex.exec(message)) !== null) {
    mentions.push(match[1]);
  }

  return mentions;
}

function init(context) {
  routeRegistry.registerStreamRoute("/notifications");
  dispatcher.registerListener("chat.message.sent", "handleChatMessage");
  return { success: true };
}
```

### Activity Tracking Script

```javascript
/**
 * activity-tracker.js - Tracks user activity and presence
 */

function trackChatActivity(context) {
  const messageData = context.messageData;

  // Update last activity timestamp
  sharedStorage.setItem(
    `activity:user:${messageData.userId}`,
    new Date().toISOString(),
  );

  // Update room activity count
  const roomKey = `activity:room:${messageData.roomId}:count`;
  const count = parseInt(sharedStorage.getItem(roomKey) || "0");
  sharedStorage.setItem(roomKey, String(count + 1));

  // Broadcast presence update
  dispatcher.sendMessage(
    "user.active",
    JSON.stringify({
      userId: messageData.userId,
      username: messageData.username,
      roomId: messageData.roomId,
      timestamp: new Date().toISOString(),
    }),
  );

  console.log(
    `Activity tracked for user ${messageData.username} in room ${messageData.roomId}`,
  );
}

function init(context) {
  dispatcher.registerListener("chat.message.sent", "trackChatActivity");
  return { success: true };
}
```

## Example 3: Content Publishing Workflow

This example shows how content creation triggers a multi-stage publishing pipeline.

### Content Creation Script

```javascript
/**
 * content-creator.js - Handles content creation
 */

function createPost(context) {
  const req = context.request;
  const { title, content, author, category } = req.form;

  const post = {
    id: generatePostId(),
    title: title,
    content: content,
    author: author,
    category: category,
    status: "draft",
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
  };

  // Store post
  sharedStorage.setItem(`post:${post.id}`, JSON.stringify(post));

  // Dispatch creation event
  dispatcher.sendMessage("content.created", JSON.stringify(post));

  return {
    status: 201,
    body: JSON.stringify({ success: true, postId: post.id }),
    contentType: "application/json",
  };
}

function publishPost(context) {
  const req = context.request;
  const postId = req.query.id;

  const postData = sharedStorage.getItem(`post:${postId}`);
  if (!postData) {
    return { status: 404, body: "Post not found" };
  }

  const post = JSON.parse(postData);
  post.status = "published";
  post.publishedAt = new Date().toISOString();

  sharedStorage.setItem(`post:${postId}`, JSON.stringify(post));

  // Dispatch publish event
  dispatcher.sendMessage("content.published", JSON.stringify(post));

  return {
    status: 200,
    body: JSON.stringify({ success: true, postId: postId }),
    contentType: "application/json",
  };
}

function generatePostId() {
  return `post_${Date.now()}_${Math.floor(Math.random() * 1000)}`;
}

function init(context) {
  routeRegistry.registerRoute("/content/create", "createPost", "POST");
  routeRegistry.registerRoute("/content/publish", "publishPost", "POST");
  return { success: true };
}
```

### Search Indexer Script

```javascript
/**
 * search-indexer.js - Indexes content for search
 */

function indexContent(context) {
  const post = context.messageData;

  // Create search index entry
  const indexEntry = {
    id: post.id,
    title: post.title,
    content: post.content,
    author: post.author,
    category: post.category,
    publishedAt: post.publishedAt,
    keywords: extractKeywords(post.title + " " + post.content),
  };

  sharedStorage.setItem(`search:index:${post.id}`, JSON.stringify(indexEntry));

  console.log(`Indexed content: ${post.title} (${post.id})`);
}

function extractKeywords(text) {
  // Simple keyword extraction (in reality, you'd use a more sophisticated algorithm)
  return text
    .toLowerCase()
    .split(/\s+/)
    .filter((word) => word.length > 3)
    .slice(0, 10);
}

function init(context) {
  dispatcher.registerListener("content.published", "indexContent");
  return { success: true };
}
```

### Subscriber Notification Script

```javascript
/**
 * subscriber-notifier.js - Notifies subscribers of new content
 */

function notifySubscribers(context) {
  const post = context.messageData;

  // Get subscribers for this category
  const subscribersKey = `subscribers:category:${post.category}`;
  const subscribersData = sharedStorage.getItem(subscribersKey) || "[]";
  const subscribers = JSON.parse(subscribersData);

  console.log(
    `Notifying ${subscribers.length} subscribers of new post in ${post.category}`,
  );

  // Send notifications to each subscriber
  subscribers.forEach((userId) => {
    routeRegistry.sendStreamMessageFiltered(
      "/notifications",
      {
        type: "new_content",
        postId: post.id,
        title: post.title,
        author: post.author,
        category: post.category,
        publishedAt: post.publishedAt,
      },
      JSON.stringify({ userId: userId }),
    );
  });
}

function init(context) {
  dispatcher.registerListener("content.published", "notifySubscribers");
  return { success: true };
}
```

## Best Practices

### 1. Message Type Naming Convention

Use a hierarchical naming scheme with dots:

```javascript
// Good
"user.registered";
"user.verified";
"user.deleted";
"content.created";
"content.published";
"order.placed";
"order.shipped";

// Bad
"newUser";
"UserRegistered";
"CONTENT_PUBLISHED";
```

### 2. Message Data Structure

Always use JSON and include contextual metadata:

```javascript
const eventData = JSON.stringify({
  // Entity identifiers
  userId: "user_123",

  // Relevant data
  username: "alice",
  email: "alice@example.com",

  // Metadata
  timestamp: new Date().toISOString(),
  source: "registration_form",

  // Optional context
  ipAddress: "192.168.1.1",
  userAgent: "Mozilla/5.0...",
});

dispatcher.sendMessage("user.registered", eventData);
```

### 3. Error Handling

Handlers should be defensive:

```javascript
function handleUserEvent(context) {
  try {
    const userData = context.messageData;

    // Validate required fields
    if (!userData || !userData.userId) {
      console.log("Invalid user data in event");
      return;
    }

    // Process event
    processUser(userData);
  } catch (error) {
    console.log(`Error handling user event: ${error.message}`);
    // Don't throw - let other handlers continue
  }
}
```

### 4. Registration in init()

Always register listeners in the `init()` function:

```javascript
function init(context) {
  // Clear any existing state if needed
  // ...

  // Register all listeners
  dispatcher.registerListener("user.registered", "handleUserRegistered");
  dispatcher.registerListener("user.verified", "handleUserVerified");
  dispatcher.registerListener("user.deleted", "handleUserDeleted");

  return { success: true };
}
```

### 5. Event Documentation

Document your events in comments:

```javascript
/**
 * Events Published:
 * - user.registered: {userId, username, email, timestamp}
 * - user.verified: {userId, verifiedAt}
 *
 * Events Consumed:
 * - content.published: {postId, title, author, category}
 */

function init(context) {
  // ...
}
```

## Common Patterns

### Fire-and-Forget Notifications

```javascript
// Publisher doesn't care if anyone is listening
dispatcher.sendMessage(
  "system.started",
  JSON.stringify({
    timestamp: new Date().toISOString(),
    version: "1.0.0",
  }),
);
```

### Event Chaining

```javascript
// Handler publishes new events
function handleOrderPlaced(context) {
  const order = context.messageData;

  // Process order
  processOrder(order);

  // Trigger next stage
  dispatcher.sendMessage(
    "order.processed",
    JSON.stringify({
      orderId: order.orderId,
      status: "processing",
      timestamp: new Date().toISOString(),
    }),
  );
}
```

### Conditional Broadcasting

```javascript
function handleUserAction(context) {
  const action = context.messageData;

  // Only broadcast significant actions
  if (action.importance === "high") {
    dispatcher.sendMessage("user.significant_action", JSON.stringify(action));
  }
}
```

## Next Steps

- Review the [JavaScript APIs Reference](../reference/javascript-apis.md#message-dispatcher) for complete API documentation
- See [Script Development Guide](../guides/scripts.md) for script architecture patterns
- Explore the [Examples Index](./index.md) for more use cases
