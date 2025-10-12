# JavaScript Authentication API

## Overview

The JavaScript Authentication API exposes user authentication information and functions to JavaScript handlers running in the QuickJS runtime. This allows your JavaScript code to check authentication status, require authentication, and access user information.

## Global `auth` Object

When a JavaScript handler is executed, a global `auth` object is available with the following properties and methods:

### Properties

#### `auth.isAuthenticated` (boolean)

Indicates whether the current request is from an authenticated user.

```javascript
if (auth.isAuthenticated) {
    console.log("User is logged in");
} else {
    console.log("Anonymous user");
}
```

#### `auth.userId` (string | null)

The unique identifier of the authenticated user, or `null` if not authenticated.

```javascript
if (auth.userId) {
    console.log(`User ID: ${auth.userId}`);
}
```

#### `auth.userEmail` (string | null)

The email address of the authenticated user, or `null` if not available.

```javascript
if (auth.userEmail) {
    console.log(`Email: ${auth.userEmail}`);
}
```

#### `auth.userName` (string | null)

The display name of the authenticated user, or `null` if not available.

```javascript
if (auth.userName) {
    console.log(`Welcome, ${auth.userName}!`);
}
```

#### `auth.provider` (string | null)

The OAuth2 provider used for authentication (`"google"`, `"microsoft"`, or `"apple"`), or `null` if not authenticated.

```javascript
if (auth.provider === "google") {
    console.log("Authenticated via Google");
}
```

### Methods

#### `auth.currentUser()` → object | null

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
const user = auth.currentUser();
if (user) {
    console.log(`User ${user.id} logged in via ${user.provider}`);
    if (user.email) {
        console.log(`Email: ${user.email}`);
    }
} else {
    console.log("No user logged in");
}
```

#### `auth.requireAuth()` → object

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
register("/api/protected", function(request) {
    // This will throw an error if not authenticated
    const user = auth.requireAuth();
    
    return {
        message: `Hello ${user.name || user.id}!`,
        data: {
            userId: user.id,
            provider: user.provider
        }
    };
});
```

## Usage Examples

### Public Endpoint (Optional Authentication)

```javascript
register("/api/greeting", function(request) {
    if (auth.isAuthenticated) {
        return {
            message: `Hello, ${auth.userName || auth.userId}!`,
            personalized: true
        };
    } else {
        return {
            message: "Hello, Guest!",
            personalized: false
        };
    }
});
```

### Protected Endpoint (Required Authentication)

```javascript
register("/api/profile", function(request) {
    const user = auth.requireAuth(); // Throws if not authenticated
    
    return {
        profile: {
            id: user.id,
            email: user.email,
            name: user.name,
            provider: user.provider
        }
    };
});
```

### Conditional Logic Based on Provider

```javascript
register("/api/data", function(request) {
    const user = auth.currentUser();
    
    if (!user) {
        return { error: "Authentication required" };
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
        message: `Data from ${dataSource}`,
        user: user.id
    };
});
```

### User-Specific Resources

```javascript
register("/api/user-data", function(request) {
    if (!auth.isAuthenticated) {
        return {
            status: 401,
            body: { error: "Unauthorized" }
        };
    }
    
    // Use user ID to fetch user-specific data
    const userData = getUserData(auth.userId);
    
    return {
        userId: auth.userId,
        data: userData
    };
});
```

### Graceful Degradation

```javascript
register("/api/content", function(request) {
    const user = auth.currentUser();
    
    // Public content available to everyone
    const publicContent = getPublicContent();
    
    if (user) {
        // Additional private content for authenticated users
        const privateContent = getPrivateContent(user.id);
        
        return {
            public: publicContent,
            private: privateContent,
            user: {
                id: user.id,
                name: user.name
            }
        };
    } else {
        return {
            public: publicContent,
            message: "Login to see more content"
        };
    }
});
```

## Error Handling

### Handling `requireAuth()` Errors

```javascript
register("/api/secure", function(request) {
    try {
        const user = auth.requireAuth();
        
        return {
            message: "Access granted",
            userId: user.id
        };
    } catch (error) {
        // This will catch authentication errors
        return {
            status: 401,
            body: {
                error: error.message,
                loginUrl: "/auth/login"
            }
        };
    }
});
```

### Custom Authentication Check

```javascript
function requireUser() {
    if (!auth.isAuthenticated) {
        throw new Error("Please login to access this resource");
    }
    return auth.currentUser();
}

register("/api/custom-protected", function(request) {
    const user = requireUser();
    
    return {
        message: "Authenticated!",
        user: user.id
    };
});
```

## Integration with Request Context

The authentication context is automatically extracted from:
1. `Authorization: Bearer <token>` header
2. `session` cookie

The middleware handles authentication before your JavaScript handler runs, so the `auth` object is always available and up-to-date.

## Security Considerations

### Never Trust Client Data for Authentication

```javascript
// ❌ BAD - Don't trust user-provided data
register("/api/bad-example", function(request) {
    const userId = request.query.userId; // DON'T DO THIS
    // Attacker could impersonate any user
});

// ✅ GOOD - Use authenticated user ID
register("/api/good-example", function(request) {
    const user = auth.requireAuth();
    const userId = user.id; // This is verified by the server
    // Safe to use for authorization
});
```

### Check Authentication, Not Just Presence

```javascript
// ❌ RISKY - Checking if userId exists
register("/api/risky", function(request) {
    if (auth.userId) {
        // This is okay but requireAuth() is clearer
    }
});

// ✅ BETTER - Use requireAuth() for clarity
register("/api/better", function(request) {
    const user = auth.requireAuth();
    // Intent is clear - authentication required
});
```

### Separate Public and Private Endpoints

```javascript
// Public endpoint
register("/api/public/status", function(request) {
    return { status: "online" };
});

// Private endpoint
register("/api/private/admin", function(request) {
    const user = auth.requireAuth();
    
    // Add additional authorization checks
    if (!isAdmin(user.id)) {
        throw new Error("Admin access required");
    }
    
    return { admin: true };
});
```

## Implementation Details

### Context Extraction

The `auth` object is populated from the request's session token, which is validated by the authentication middleware before the JavaScript handler runs.

### Performance

Authentication context is extracted once per request and cached, so there's no performance penalty for accessing `auth` properties multiple times in your handler.

### Null Safety

All user information properties (`userId`, `userEmail`, `userName`, `provider`) are `null` when not authenticated or not available, making them safe to check with standard JavaScript truthiness checks:

```javascript
if (auth.userName) {
    // userName is available and not null
}
```

## See Also

- [Authentication Setup Guide](./AUTH_SETUP.md) - How to configure OAuth2 providers
- [Authentication Routes](./AUTH_API.md) - HTTP endpoints for login/logout
- [Middleware Documentation](./AUTH_MIDDLEWARE.md) - Server-side authentication

---

**Version:** 1.0  
**Last Updated:** January 2025
