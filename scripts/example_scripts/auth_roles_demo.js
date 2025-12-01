/**
 * Authentication Roles Demo
 *
 * Demonstrates how to use the req.auth.isAdmin, req.auth.isEditor, and req.auth.isAuthenticated
 * properties in JavaScript handlers.
 */

export async function handleRequest(context) {
  const request = context.request || {};
  if (!request.auth) {
    throw new Error("Authentication context unavailable in handler");
  }
  // Check if user is authenticated
  if (!request.auth.isAuthenticated) {
    return {
      status: 401,
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        error: "Authentication required",
        message: "Please login to access this resource",
      }),
    };
  }

  // Get current user information
  const user = request.auth.user;

  // Build response based on user roles
  const roles = [];
  if (request.auth.isAdmin) {
    roles.push("Administrator");
  }
  if (request.auth.isEditor) {
    roles.push("Editor");
  }
  if (roles.length === 0) {
    roles.push("Viewer");
  }

  // Example: Restrict certain actions to editors or admins
  if (
    request.method === "POST" ||
    request.method === "PUT" ||
    request.method === "DELETE"
  ) {
    if (!request.auth.isEditor && !request.auth.isAdmin) {
      return {
        status: 403,
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          error: "Insufficient permissions",
          message: "Editor or Administrator role required for this action",
        }),
      };
    }
  }

  // Example: Restrict admin-only actions
  if (request.path === "/admin/settings" && !request.auth.isAdmin) {
    return {
      status: 403,
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        error: "Insufficient permissions",
        message: "Administrator role required for this action",
      }),
    };
  }

  // Return user info and capabilities
  return {
    status: 200,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      user: {
        id: user.id,
        email: user.email,
        name: user.name,
        provider: user.provider,
      },
      roles: roles,
      capabilities: {
        canView: true,
        canEdit: request.auth.isEditor || request.auth.isAdmin,
        canAdminister: request.auth.isAdmin,
      },
      message: `Welcome ${user.name || user.email}! You have ${roles.join(", ")} access.`,
    }),
  };
}

/**
 * Example: Editor-only endpoint
 */
export async function editorOnly(context) {
  const request = context.request || {};
  if (!request.auth) {
    throw new Error("Authentication context unavailable in handler");
  }
  // Simple check using requireAuth
  const user = request.auth.requireAuth(); // Throws if not authenticated

  if (!request.auth.isEditor && !request.auth.isAdmin) {
    return {
      status: 403,
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        error: "Editor access required",
      }),
    };
  }

  return {
    status: 200,
    headers: { "Content-Type": "text/plain" },
    body: `Hello ${user.name}, you have editor access!`,
  };
}

/**
 * Example: Admin-only endpoint
 */
export async function adminOnly(context) {
  const request = context.request || {};
  if (!request.auth) {
    throw new Error("Authentication context unavailable in handler");
  }
  const user = request.auth.requireAuth();

  if (!request.auth.isAdmin) {
    return {
      status: 403,
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        error: "Administrator access required",
      }),
    };
  }

  return {
    status: 200,
    headers: { "Content-Type": "text/plain" },
    body: `Hello ${user.name}, you have administrator access!`,
  };
}
