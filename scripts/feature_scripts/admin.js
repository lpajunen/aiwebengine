// User Management Interface for Administrators
// Provides UI and API for managing user roles
//
// AUTHENTICATION USAGE:
// - 'auth' is part of the request object (req.auth)
// - Available properties:
//   * req.auth.isAuthenticated (boolean) - whether user is logged in
//   * req.auth.isAdmin (boolean) - whether user has admin privileges
//   * req.auth.userId (string|null) - user's unique ID
//   * req.auth.userEmail (string|null) - user's email address
//   * req.auth.userName (string|null) - user's display name
//   * req.auth.provider (string|null) - OAuth provider (google, microsoft, apple)
// - Available methods:
//   * req.auth.user (object|null) - complete user object with id, email, name, provider, isAuthenticated
//   * req.auth.requireAuth() - throws error if not authenticated
//
// REQUEST OBJECT:
// - 'request' parameter passed to handlers contains:
//   * request.path (string) - the URL path
//   * request.method (string) - HTTP method (GET, POST, etc.)
//   * request.query (object) - query string parameters
//   * request.form (object) - form data (for POST requests)
//   * request.body (string) - raw request body
//   * request.auth (object) - authentication context (see above)

function getRequest(context) {
  return (context && context.request) || {};
}

function init(context) {
  // Register routes for user management
  routeRegistry.registerRoute("/engine/admin", "handleManagerUI", "GET");
  routeRegistry.registerRoute(
    "/api/engine/admin/users",
    "handleListUsers",
    "GET",
  );
  routeRegistry.registerRoute(
    "/api/engine/admin/users/*",
    "handleUpdateUserRole",
    "POST",
  );

  return { success: true };
}

// Serve the management UI (HTML page)
function handleManagerUI(context) {
  const request = getRequest(context);
  // Check if user is authenticated and is an administrator
  if (!request.auth || !request.auth.isAuthenticated) {
    return Response.redirect("/auth/login?redirect=/engine/admin");
  }

  if (!request.auth.isAdmin) {
    return Response.error(
      403,
      "Access denied. Administrator privileges required.",
    );
  }

  const html = `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>User Management - AIWebEngine</title>
    <link rel="stylesheet" href="/engine.css">
    <link rel="icon" type="image/x-icon" href="/favicon.ico">
    <style>
        /* Manager-specific overrides */
        body {
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            padding: 2rem 0;
        }

        .page-header {
            background: rgba(255, 255, 255, 0.95);
            backdrop-filter: blur(10px);
            border: none;
            margin-bottom: 2rem;
        }

        .stats-grid {
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(250px, 1fr));
            gap: 1.5rem;
            margin-bottom: 2rem;
        }

        .stat-card {
            background: rgba(255, 255, 255, 0.95);
            backdrop-filter: blur(10px);
            border: none;
            padding: 1.5rem;
            text-align: center;
            box-shadow: 0 8px 32px rgba(0, 0, 0, 0.1);
        }

        .stat-card h3 {
            color: var(--text-muted);
            font-size: 0.875rem;
            font-weight: 600;
            text-transform: uppercase;
            letter-spacing: 0.5px;
            margin-bottom: 0.5rem;
        }

        .stat-card .value {
            color: var(--text-color);
            font-size: 2.5rem;
            font-weight: bold;
            margin: 0;
        }

        .users-section {
            background: rgba(255, 255, 255, 0.95);
            backdrop-filter: blur(10px);
            border: none;
            box-shadow: 0 8px 32px rgba(0, 0, 0, 0.1);
        }

        .role-badge {
            display: inline-block;
            padding: 0.25rem 0.75rem;
            border-radius: 1rem;
            font-size: 0.75rem;
            font-weight: 600;
            text-transform: uppercase;
            letter-spacing: 0.5px;
            margin-right: 0.25rem;
            margin-bottom: 0.25rem;
        }

        .role-badge.authenticated {
            background: rgba(25, 118, 210, 0.1);
            color: #1976d2;
        }

        .role-badge.editor {
            background: rgba(245, 124, 0, 0.1);
            color: #f57c00;
        }

        .role-badge.administrator {
            background: rgba(194, 24, 91, 0.1);
            color: #c2185b;
        }

        .provider-tag {
            background: rgba(63, 81, 181, 0.1);
            color: #3f51b5;
            padding: 0.125rem 0.5rem;
            border-radius: 0.5rem;
            font-size: 0.75rem;
            font-weight: 500;
        }

        .nav-links {
            display: flex;
            gap: 1rem;
            margin-top: 1rem;
            flex-wrap: wrap;
        }

        .nav-link {
            color: var(--primary-color);
            text-decoration: none;
            font-weight: 500;
            transition: var(--transition);
        }

        .nav-link:hover {
            color: var(--primary-color);
            text-decoration: underline;
        }

        .btn-add-editor {
            background: #ff9800;
            color: white;
        }

        .btn-add-editor:hover:not(:disabled) {
            background: #e68900;
        }

        .btn-add-admin {
            background: #e91e63;
            color: white;
        }

        .btn-add-admin:hover:not(:disabled) {
            background: #d81b60;
        }

        .btn-remove-editor,
        .btn-remove-admin {
            background: var(--bg-secondary);
            color: var(--text-muted);
            border: 1px solid var(--border-color);
        }

        .btn-remove-editor:hover:not(:disabled),
        .btn-remove-admin:hover:not(:disabled) {
            background: var(--bg-tertiary);
        }

        .alert-success {
            background: rgba(40, 167, 69, 0.1);
            border-color: rgba(40, 167, 69, 0.2);
            color: #155724;
        }

        @media (max-width: 768px) {
            .stats-grid {
                grid-template-columns: 1fr;
            }

            .role-actions {
                flex-direction: column;
            }

            .btn {
                width: 100%;
            }
        }
    </style>
</head>
<body>
    <div class="container">
        <header class="page-header">
            <div class="text-center">
                <h1>User Management</h1>
                <p class="text-muted">Manage user roles and permissions</p>
                <div class="nav-links">
                    <a href="/" class="nav-link">← Back to Home</a>
                    <a href="/engine/editor" class="nav-link">Editor</a>
                    <a href="/engine/graphql" class="nav-link">GraphQL</a>
                </div>
            </div>
        </header>

        <div class="stats-grid" id="stats">
            <div class="stat-card">
                <h3>Total Users</h3>
                <div class="value" id="total-users">-</div>
            </div>
            <div class="stat-card">
                <h3>Administrators</h3>
                <div class="value" id="total-admins">-</div>
            </div>
            <div class="stat-card">
                <h3>Editors</h3>
                <div class="value" id="total-editors">-</div>
            </div>
        </div>

        <div class="card users-section">
            <div class="card-header">
                <h2 class="mb-0">Users</h2>
            </div>
            <div class="card-body">
                <div id="error-container"></div>
                <div id="loading" class="loading">Loading users...</div>
                <div id="users-container" style="display: none;"></div>
            </div>
        </div>
    </div>
    
    <script>
        let users = [];
        
        // Load users on page load
        async function loadUsers() {
            const errorContainer = document.getElementById('error-container');
            const loading = document.getElementById('loading');
            const usersContainer = document.getElementById('users-container');
            
            try {
                const response = await fetch('/api/engine/admin/users');
                
                if (!response.ok) {
                    const error = await response.json();
                    throw new Error(error.error || 'Failed to load users');
                }
                
                const data = await response.json();
                users = data.users;
                
                // Update stats
                document.getElementById('total-users').textContent = data.total;
                
                const adminCount = users.filter(u => 
                    u.roles.some(r => r.toLowerCase() === 'administrator')
                ).length;
                document.getElementById('total-admins').textContent = adminCount;
                
                const editorCount = users.filter(u => 
                    u.roles.some(r => r.toLowerCase() === 'editor')
                ).length;
                document.getElementById('total-editors').textContent = editorCount;
                
                // Render users table
                renderUsers();
                
                loading.style.display = 'none';
                usersContainer.style.display = 'block';
            } catch (error) {
                loading.style.display = 'none';
                errorContainer.innerHTML = \`
                    <div class="alert alert-danger">
                        <strong>Error:</strong> \${error.message}
                    </div>
                \`;
            }
        }
        
        // Render users table
        function renderUsers() {
            const container = document.getElementById('users-container');
            
            if (users.length === 0) {
                container.innerHTML = '<p>No users found.</p>';
                return;
            }
            
            const html = \`
                <table class="user-table">
                    <thead>
                        <tr>
                            <th>Email</th>
                            <th>Name</th>
                            <th>Roles</th>
                            <th>Providers</th>
                            <th>Created</th>
                            <th>Actions</th>
                        </tr>
                    </thead>
                    <tbody>
                        \${users.map(user => renderUserRow(user)).join('')}
                    </tbody>
                </table>
            \`;
            
            container.innerHTML = html;
        }
        
        // Render a single user row
        function renderUserRow(user) {
            const hasEditor = user.roles.some(r => r.toLowerCase() === 'editor');
            const hasAdmin = user.roles.some(r => r.toLowerCase() === 'administrator');
            
            return \`
                <tr>
                    <td><strong>\${user.email}</strong></td>
                    <td>\${user.name || '-'}</td>
                    <td>
                        \${user.roles.map(role => {
                            const roleClass = role.toLowerCase();
                            return \`<span class="role-badge \${roleClass}">\${role}</span>\`;
                        }).join('')}
                    </td>
                    <td>
                        <div class="providers">
                            \${user.providers.map(p => 
                                \`<span class="provider-tag">\${p}</span>\`
                            ).join('')}
                        </div>
                    </td>
                    <td>
                        <div class="timestamp">\${formatDate(user.created_at)}</div>
                    </td>
                    <td>
                        <div class="role-actions">
                            \${hasEditor 
                                ? \`<button class="btn btn-remove-editor" onclick="updateRole('\${user.id}', 'Editor', 'remove')">Remove Editor</button>\`
                                : \`<button class="btn btn-add-editor" onclick="updateRole('\${user.id}', 'Editor', 'add')">Add Editor</button>\`
                            }
                            \${hasAdmin 
                                ? \`<button class="btn btn-remove-admin" onclick="updateRole('\${user.id}', 'Administrator', 'remove')">Remove Admin</button>\`
                                : \`<button class="btn btn-add-admin" onclick="updateRole('\${user.id}', 'Administrator', 'add')">Add Admin</button>\`
                            }
                        </div>
                    </td>
                </tr>
            \`;
        }
        
        // Update user role
        async function updateRole(userId, role, action) {
            const errorContainer = document.getElementById('error-container');
            errorContainer.innerHTML = '';
            
            try {
                const response = await fetch("/api/engine/admin/users/" + userId + "/roles", {
                    method: 'POST',
                    headers: {
                        'Content-Type': 'application/json'
                    },
                    body: JSON.stringify({ role, action })
                });
                
                if (!response.ok) {
                    const error = await response.json();
                    throw new Error(error.error || 'Failed to update role');
                }
                
                const result = await response.json();
                
                // Reload users to reflect changes
                await loadUsers();
                
                // Show success message briefly
                errorContainer.innerHTML = '<div class="alert alert-success"><strong>Success:</strong> Role ' + (action === 'add' ? 'added' : 'removed') + ' successfully</div>';
                
                setTimeout(() => {
                    errorContainer.innerHTML = '';
                }, 3000);
                
            } catch (error) {
                errorContainer.innerHTML = '<div class="alert alert-danger"><strong>Error:</strong> ' + error.message + '</div>';
            }
        }
        
        // Format date for display
        function formatDate(dateStr) {
            // Parse the SystemTime debug format
            const match = dateStr.match(/secs: (\\d+)/);
            if (match) {
                const secs = parseInt(match[1]);
                const date = new Date(secs * 1000);
                return date.toLocaleDateString() + ' ' + date.toLocaleTimeString();
            }
            return dateStr;
        }
        
        // Load users when page loads
        loadUsers();
    </script>
</body>
</html>`;

  return Response.html(html);
}

// API endpoint to list all users
function handleListUsers(context) {
  const request = getRequest(context);
  // Check if user is authenticated and is an administrator
  if (!request.auth || !request.auth.isAuthenticated) {
    return Response.error(401, "Authentication required");
  }

  if (!request.auth.isAdmin) {
    return Response.error(
      403,
      "Access denied. Administrator privileges required.",
    );
  }

  try {
    // Call Rust function to list users (returns JSON string)
    const usersJson = userStorage.listUsers();
    const users = JSON.parse(usersJson);

    return Response.json({
      users: users,
      total: users.length,
    });
  } catch (error) {
    return Response.error(500, "Failed to list users");
  }
}

// API endpoint to update user role
function handleUpdateUserRole(context) {
  const request = getRequest(context);
  // Check if user is authenticated and is an administrator
  if (!request.auth || !request.auth.isAuthenticated) {
    return Response.error(401, "Authentication required");
  }

  if (!request.auth.isAdmin) {
    return Response.error(
      403,
      "Access denied. Administrator privileges required.",
    );
  }

  try {
    // Parse request body
    let body;
    try {
      body = JSON.parse(request.body);
    } catch (e) {
      return Response.error(400, "Invalid JSON in request body");
    }

    const { role, action } = body;

    // Validate parameters
    if (!role || !action) {
      return Response.error(400, "Missing required fields: role, action");
    }

    if (!["add", "remove"].includes(action)) {
      return Response.error(400, 'Invalid action. Must be "add" or "remove"');
    }

    if (!["Editor", "Administrator"].includes(role)) {
      return Response.error(
        400,
        'Invalid role. Must be "Editor" or "Administrator"',
      );
    }

    // Extract userId from path
    const pathParts = request.path.split("/");
    const userId = pathParts[pathParts.indexOf("users") + 1];

    if (!userId) {
      return Response.error(400, "User ID is required");
    }

    // Call Rust function to update role
    if (action === "add") {
      userStorage.addUserRole(userId, role);
    } else {
      userStorage.removeUserRole(userId, role);
    }

    return Response.json({
      success: true,
      userId: userId,
      role: role,
      action: action,
    });
  } catch (error) {
    return Response.error(500, "Failed to update user role");
  }
}
