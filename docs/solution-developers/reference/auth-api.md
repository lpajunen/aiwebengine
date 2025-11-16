# JavaScript Authentication API

## Overview

The JavaScript Authentication API exposes user authentication information and functions to JavaScript handlers running in the QuickJS runtime. This allows your JavaScript code to check authentication status, require authentication, and access user information.

## Request Authentication Object

When a JavaScript handler is executed, authentication information is available via the `req.auth` property (or `request.auth` depending on your parameter name). This object contains the following properties and methods:

### Properties

#### `req.auth.isAuthenticated` (boolean)

Indicates whether the current request is from an authenticated user.

```javascript
function myHandler(req) {
  if (req.auth.isAuthenticated) {
    console.log("User is logged in");
  } else {
    console.log("Anonymous user");
  }
}
```

#### `req.auth.userId` (string | null)

The unique identifier of the authenticated user, or `null` if not authenticated.

```javascript
function myHandler(req) {
  if (req.auth.userId) {
    console.log(`User ID: ${req.auth.userId}`);
  }
}
```

#### `req.auth.userEmail` (string | null)

The email address of the authenticated user, or `null` if not available.

```javascript
function myHandler(req) {
  if (req.auth.userEmail) {
    console.log(`Email: ${req.auth.userEmail}`);
  }
}
```

#### `req.auth.userName` (string | null)

The display name of the authenticated user, or `null` if not available.

```javascript
function myHandler(req) {
  if (req.auth.userName) {
    console.log(`Welcome, ${req.auth.userName}!`);
  }
}
```

#### `req.auth.provider` (string | null)

The OAuth2 provider used for authentication (`"google"`, `"microsoft"`, or `"apple"`), or `null` if not authenticated.

```javascript
function myHandler(req) {
  if (req.auth.provider === "google") {
    console.log("Authenticated via Google");
  }
}
```

#### `req.auth.isAdmin` (boolean)

Indicates whether the current user has administrator privileges.

```javascript
function myHandler(req) {
  if (req.auth.isAdmin) {
    console.log("User is an administrator");
  }
}
```

#### `req.auth.isEditor` (boolean)

Indicates whether the current user has editor privileges.

```javascript
function myHandler(req) {
  if (req.auth.isEditor) {
    console.log("User is an editor");
  }
}
```

### Methods

#### `req.auth.currentUser()` → object | null

Returns an object with complete user information if authenticated, or `null` if not authenticated.

**Returns:**

```typescript
{
    id: string,
    email?: string,
    name?: string,
    provider: string,
    isAuthenticated: true
} | null
```

**Example:**

```javascript
function myHandler(req) {
  const user = req.auth.currentUser();
  if (user) {
    console.log(`User ${user.id} logged in via ${user.provider}`);
    if (user.email) {
      console.log(`Email: ${user.email}`);
    }
  } else {
    console.log("No user logged in");
  }
}
```

#### `req.auth.requireAuth()` → object

Returns the current user object if authenticated, or **throws an error** if not authenticated.

Use this in handlers that require authentication - it will automatically reject anonymous requests.

**Returns:**

```typescript
{
    id: string,
    email?: string,
    name?: string,
    provider: string,
    isAuthenticated: true
}
```

**Throws:** `Error` with message `"Authentication required. Please login to access this resource."`

**Example:**

```javascript
// Protected endpoint - only accessible to authenticated users
function protectedHandler(req) {
  // This will throw an error if not authenticated
  const user = req.auth.requireAuth();

  return {
    status: 200,
    body: JSON.stringify({
      message: `Hello ${user.name || user.id}!`,
      data: {
        userId: user.id,
        provider: user.provider,
      },
    }),
    contentType: "application/json",
  };
}
```

## Usage Examples

### Public Endpoint (Optional Authentication)

```javascript
function greetingHandler(req) {
  if (req.auth.isAuthenticated) {
    return {
      status: 200,
      body: JSON.stringify({
        message: `Hello, ${req.auth.userName || req.auth.userId}!`,
        personalized: true,
      }),
      contentType: "application/json",
    };
  } else {
    return {
      status: 200,
      body: JSON.stringify({
        message: "Hello, Guest!",
        personalized: false,
      }),
      contentType: "application/json",
    };
  }
}
```

### Protected Endpoint (Required Authentication)

```javascript
function profileHandler(req) {
  const user = req.auth.requireAuth(); // Throws if not authenticated

  return {
    status: 200,
    body: JSON.stringify({
      profile: {
        id: user.id,
        email: user.email,
        name: user.name,
        provider: user.provider,
      },
    }),
    contentType: "application/json",
  };
}
```

### Conditional Logic Based on Provider

```javascript
function dataHandler(req) {
  const user = req.auth.currentUser();

  if (!user) {
    return {
      status: 401,
      body: JSON.stringify({ error: "Authentication required" }),
      contentType: "application/json",
    };
  }

  // Different behavior based on OAuth provider
  let dataSource;
  switch (user.provider) {
    case "google":
      dataSource = "Google Workspace";
      break;
    case "microsoft":
      dataSource = "Microsoft 365";
      break;
    case "apple":
      dataSource = "iCloud";
      break;
    default:
      dataSource = "Unknown";
  }

  return {
    status: 200,
    body: JSON.stringify({
      message: `Data from ${dataSource}`,
      user: user.id,
    }),
    contentType: "application/json",
  };
}
```

### User-Specific Resources

```javascript
function userDataHandler(req) {
  if (!req.auth.isAuthenticated) {
    return {
      status: 401,
      body: JSON.stringify({ error: "Unauthorized" }),
      contentType: "application/json",
    };
  }

  // Use user ID to fetch user-specific data
  const userData = getUserData(req.auth.userId);

  return {
    status: 200,
    body: JSON.stringify({
      userId: req.auth.userId,
      data: userData,
    }),
    contentType: "application/json",
  };
}
```

### Graceful Degradation

```javascript
function contentHandler(req) {
  const user = req.auth.currentUser();

  // Public content available to everyone
  const publicContent = getPublicContent();

  if (user) {
    // Additional private content for authenticated users
    const privateContent = getPrivateContent(user.id);

    return {
      status: 200,
      body: JSON.stringify({
        public: publicContent,
        private: privateContent,
        user: {
          id: user.id,
          name: user.name,
        },
      }),
      contentType: "application/json",
    };
  } else {
    return {
      status: 200,
      body: JSON.stringify({
        public: publicContent,
        message: "Login to see more content",
      }),
      contentType: "application/json",
    };
  }
}
```

## Error Handling

### Handling `requireAuth()` Errors

```javascript
function secureHandler(req) {
  try {
    const user = req.auth.requireAuth();

    return {
      status: 200,
      body: JSON.stringify({
        message: "Access granted",
        userId: user.id,
      }),
      contentType: "application/json",
    };
  } catch (error) {
    // This will catch authentication errors
    return {
      status: 401,
      body: JSON.stringify({
        error: error.message,
        loginUrl: "/auth/login",
      }),
      contentType: "application/json",
    };
  }
}
```

### Custom Authentication Check

```javascript
function requireUser(req) {
  if (!req.auth.isAuthenticated) {
    throw new Error("Please login to access this resource");
  }
  return req.auth.currentUser();
}

function customProtectedHandler(req) {
  const user = requireUser(req);

  return {
    status: 200,
    body: JSON.stringify({
      message: "Authenticated!",
      user: user.id,
    }),
    contentType: "application/json",
  };
}
```

## Integration with Request Context

The authentication context is automatically extracted from:

1. `Authorization: Bearer <token>` header
2. `session` cookie

The middleware handles authentication before your JavaScript handler runs, so the `req.auth` object is always available and up-to-date.

## Security Considerations

### Never Trust Client Data for Authentication

```javascript
// ❌ BAD - Don't trust user-provided data
function badExampleHandler(req) {
  const userId = req.query.userId; // DON'T DO THIS
  // Attacker could impersonate any user
}

// ✅ GOOD - Use authenticated user ID
function goodExampleHandler(req) {
  const user = req.auth.requireAuth();
  const userId = user.id; // This is verified by the server
  // Safe to use for authorization
}
```

### Check Authentication, Not Just Presence

```javascript
// ❌ RISKY - Checking if userId exists
function riskyHandler(req) {
  if (req.auth.userId) {
    // This is okay but requireAuth() is clearer
  }
}

// ✅ BETTER - Use requireAuth() for clarity
function betterHandler(req) {
  const user = req.auth.requireAuth();
  // Intent is clear - authentication required
}
```

### Separate Public and Private Endpoints

```javascript
// Public endpoint
function publicStatusHandler(req) {
  return {
    status: 200,
    body: JSON.stringify({ status: "online" }),
    contentType: "application/json",
  };
}

// Private endpoint
function adminHandler(req) {
  const user = req.auth.requireAuth();

  // Add additional authorization checks
  if (!req.auth.isAdmin) {
    return {
      status: 403,
      body: JSON.stringify({ error: "Admin access required" }),
      contentType: "application/json",
    };
  }

  return {
    status: 200,
    body: JSON.stringify({ admin: true }),
    contentType: "application/json",
  };
}
```

## Implementation Details

### Context Extraction

The `req.auth` object is populated from the request's session token, which is validated by the authentication middleware before the JavaScript handler runs.

### Performance

Authentication context is extracted once per request and cached, so there's no performance penalty for accessing `req.auth` properties multiple times in your handler.

### Null Safety

All user information properties (`userId`, `userEmail`, `userName`, `provider`) are `null` when not authenticated or not available, making them safe to check with standard JavaScript truthiness checks:

```javascript
function myHandler(req) {
  if (req.auth.userName) {
    // userName is available and not null
  }
}
```

## See Also

- [Authentication Setup Guide](./AUTH_SETUP.md) - How to configure OAuth2 providers
- [Authentication Routes](./AUTH_API.md) - HTTP endpoints for login/logout
- [Middleware Documentation](./AUTH_MIDDLEWARE.md) - Server-side authentication

---

**Version:** 2.0  
**Last Updated:** November 2025
