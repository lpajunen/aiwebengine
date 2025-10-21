// User Management Interface for Administrators
// Provides UI and API for managing user roles
//
// AUTHENTICATION USAGE:
// - 'auth' is a GLOBAL object (not part of request)
// - Available properties:
//   * auth.isAuthenticated (boolean) - whether user is logged in
//   * auth.isAdmin (boolean) - whether user has admin privileges
//   * auth.userId (string|null) - user's unique ID
//   * auth.userEmail (string|null) - user's email address
//   * auth.userName (string|null) - user's display name
//   * auth.provider (string|null) - OAuth provider (google, microsoft, apple)
// - Available methods:
//   * auth.currentUser() - returns user object or null
//   * auth.requireAuth() - throws error if not authenticated
//
// REQUEST OBJECT:
// - 'request' parameter passed to handlers contains:
//   * request.path (string) - the URL path
//   * request.method (string) - HTTP method (GET, POST, etc.)
//   * request.query (object) - query string parameters
//   * request.form (object) - form data (for POST requests)
//   * request.body (string) - raw request body

function init(context) {
    // Register routes for user management
    register('/manager', 'handleManagerUI', 'GET');
    register('/api/manager/users', 'handleListUsers', 'GET');
    register('/api/manager/users/*', 'handleUpdateUserRole', 'POST');
    
    return { success: true };
}

// Serve the management UI (HTML page)
function handleManagerUI(request) {
    // Check if user is authenticated and is an administrator
    // Note: 'auth' is a global object, not part of request
    if (!auth || !auth.isAuthenticated) {
        return {
            status: 302,
            headers: { 'Location': '/auth/login?redirect=/manager' },
            body: ''
        };
    }
    
    if (!auth.isAdmin) {
        return {
            status: 403,
            body: JSON.stringify({ error: 'Access denied. Administrator privileges required.' }),
            contentType: 'application/json'
        };
    }
    
    const html = `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>User Management - AIWebEngine</title>
    <style>
        * {
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }
        
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            padding: 20px;
        }
        
        .container {
            max-width: 1200px;
            margin: 0 auto;
        }
        
        .header {
            background: white;
            border-radius: 12px;
            padding: 24px;
            margin-bottom: 20px;
            box-shadow: 0 4px 6px rgba(0, 0, 0, 0.1);
        }
        
        .header h1 {
            color: #333;
            font-size: 28px;
            margin-bottom: 8px;
        }
        
        .header p {
            color: #666;
            font-size: 14px;
        }
        
        .stats {
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
            gap: 16px;
            margin-bottom: 20px;
        }
        
        .stat-card {
            background: white;
            border-radius: 12px;
            padding: 20px;
            box-shadow: 0 4px 6px rgba(0, 0, 0, 0.1);
        }
        
        .stat-card h3 {
            color: #666;
            font-size: 14px;
            font-weight: 500;
            margin-bottom: 8px;
        }
        
        .stat-card .value {
            color: #333;
            font-size: 32px;
            font-weight: bold;
        }
        
        .users-section {
            background: white;
            border-radius: 12px;
            padding: 24px;
            box-shadow: 0 4px 6px rgba(0, 0, 0, 0.1);
        }
        
        .users-section h2 {
            color: #333;
            font-size: 20px;
            margin-bottom: 20px;
        }
        
        .loading {
            text-align: center;
            padding: 40px;
            color: #666;
        }
        
        .error {
            background: #fee;
            border: 1px solid #fcc;
            border-radius: 8px;
            padding: 16px;
            color: #c33;
            margin-bottom: 16px;
        }
        
        .user-table {
            width: 100%;
            border-collapse: collapse;
        }
        
        .user-table thead {
            background: #f8f9fa;
        }
        
        .user-table th {
            text-align: left;
            padding: 12px;
            font-weight: 600;
            color: #333;
            border-bottom: 2px solid #e9ecef;
        }
        
        .user-table td {
            padding: 12px;
            border-bottom: 1px solid #e9ecef;
            color: #555;
        }
        
        .user-table tr:hover {
            background: #f8f9fa;
        }
        
        .role-badge {
            display: inline-block;
            padding: 4px 8px;
            border-radius: 4px;
            font-size: 12px;
            font-weight: 500;
            margin-right: 4px;
            margin-bottom: 4px;
        }
        
        .role-badge.authenticated {
            background: #e3f2fd;
            color: #1976d2;
        }
        
        .role-badge.editor {
            background: #fff3e0;
            color: #f57c00;
        }
        
        .role-badge.administrator {
            background: #fce4ec;
            color: #c2185b;
        }
        
        .role-actions {
            display: flex;
            gap: 8px;
            flex-wrap: wrap;
        }
        
        .btn {
            padding: 6px 12px;
            border: none;
            border-radius: 6px;
            font-size: 13px;
            cursor: pointer;
            font-weight: 500;
            transition: all 0.2s;
        }
        
        .btn:hover {
            transform: translateY(-1px);
            box-shadow: 0 2px 4px rgba(0, 0, 0, 0.2);
        }
        
        .btn:disabled {
            opacity: 0.5;
            cursor: not-allowed;
            transform: none;
        }
        
        .btn-add-editor {
            background: #ff9800;
            color: white;
        }
        
        .btn-remove-editor {
            background: #e0e0e0;
            color: #666;
        }
        
        .btn-add-admin {
            background: #e91e63;
            color: white;
        }
        
        .btn-remove-admin {
            background: #e0e0e0;
            color: #666;
        }
        
        .timestamp {
            font-size: 12px;
            color: #999;
        }
        
        .providers {
            display: flex;
            gap: 4px;
            flex-wrap: wrap;
        }
        
        .provider-tag {
            background: #e8eaf6;
            color: #3f51b5;
            padding: 2px 8px;
            border-radius: 4px;
            font-size: 11px;
            font-weight: 500;
        }
        
        .nav-links {
            display: flex;
            gap: 12px;
            margin-top: 16px;
        }
        
        .nav-link {
            color: #667eea;
            text-decoration: none;
            font-size: 14px;
            font-weight: 500;
        }
        
        .nav-link:hover {
            text-decoration: underline;
        }
        
        @media (max-width: 768px) {
            .user-table {
                font-size: 14px;
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
        <div class="header">
            <h1>User Management</h1>
            <p>Manage user roles and permissions</p>
            <div class="nav-links">
                <a href="/" class="nav-link">← Back to Home</a>
                <a href="/editor" class="nav-link">Editor</a>
                <a href="/graphql" class="nav-link">GraphQL</a>
            </div>
        </div>
        
        <div class="stats" id="stats">
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
        
        <div class="users-section">
            <h2>Users</h2>
            <div id="error-container"></div>
            <div id="loading" class="loading">Loading users...</div>
            <div id="users-container" style="display: none;"></div>
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
                const response = await fetch('/api/manager/users');
                
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
                    <div class="error">
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
                const response = await fetch(\`/api/manager/users/\${userId}/roles\`, {
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
                errorContainer.innerHTML = \`
                    <div style="background: #e8f5e9; border: 1px solid #c8e6c9; border-radius: 8px; padding: 16px; color: #2e7d32; margin-bottom: 16px;">
                        <strong>Success:</strong> Role \${action === 'add' ? 'added' : 'removed'} successfully
                    </div>
                \`;
                
                setTimeout(() => {
                    errorContainer.innerHTML = '';
                }, 3000);
                
            } catch (error) {
                errorContainer.innerHTML = \`
                    <div class="error">
                        <strong>Error:</strong> \${error.message}
                    </div>
                \`;
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
    
    return {
        status: 200,
        body: html,
        contentType: 'text/html'
    };
}

// API endpoint to list all users
function handleListUsers(request) {
    // Check if user is authenticated and is an administrator
    // Note: 'auth' is a global object, not part of request
    if (!auth || !auth.isAuthenticated) {
        return {
            status: 401,
            body: JSON.stringify({ error: 'Authentication required' }),
            contentType: 'application/json'
        };
    }
    
    if (!auth.isAdmin) {
        return {
            status: 403,
            body: JSON.stringify({ error: 'Access denied. Administrator privileges required.' }),
            contentType: 'application/json'
        };
    }
    
    try {
        // Call Rust function to list users
        const users = listUsers();
        
        return {
            status: 200,
            body: JSON.stringify({
                users: users,
                total: users.length
            }),
            contentType: 'application/json'
        };
    } catch (error) {
        return {
            status: 500,
            body: JSON.stringify({ 
                error: 'Failed to list users',
                details: error.toString()
            }),
            contentType: 'application/json'
        };
    }
}

// API endpoint to update user role
function handleUpdateUserRole(request) {
    // Check if user is authenticated and is an administrator
    // Note: 'auth' is a global object, not part of request
    if (!auth || !auth.isAuthenticated) {
        return {
            status: 401,
            body: JSON.stringify({ error: 'Authentication required' }),
            contentType: 'application/json'
        };
    }
    
    if (!auth.isAdmin) {
        return {
            status: 403,
            body: JSON.stringify({ error: 'Access denied. Administrator privileges required.' }),
            contentType: 'application/json'
        };
    }
    
    try {
        // Parse request body
        let body;
        try {
            body = JSON.parse(request.body);
        } catch (e) {
            return {
                status: 400,
                body: JSON.stringify({ error: 'Invalid JSON in request body' }),
                contentType: 'application/json'
            };
        }
        
        const { role, action } = body;
        
        // Validate parameters
        if (!role || !action) {
            return {
                status: 400,
                body: JSON.stringify({ error: 'Missing required fields: role, action' }),
                contentType: 'application/json'
            };
        }
        
        if (!['add', 'remove'].includes(action)) {
            return {
                status: 400,
                body: JSON.stringify({ error: 'Invalid action. Must be "add" or "remove"' }),
                contentType: 'application/json'
            };
        }
        
        if (!['Editor', 'Administrator'].includes(role)) {
            return {
                status: 400,
                body: JSON.stringify({ error: 'Invalid role. Must be "Editor" or "Administrator"' }),
                contentType: 'application/json'
            };
        }
        
        // Extract userId from path
        const pathParts = request.path.split('/');
        const userId = pathParts[pathParts.indexOf('users') + 1];
        
        if (!userId) {
            return {
                status: 400,
                body: JSON.stringify({ error: 'User ID is required' }),
                contentType: 'application/json'
            };
        }
        
        // Call Rust function to update role
        if (action === 'add') {
            addUserRole(userId, role);
        } else {
            removeUserRole(userId, role);
        }
        
        return {
            status: 200,
            body: JSON.stringify({
                success: true,
                userId: userId,
                role: role,
                action: action
            }),
            contentType: 'application/json'
        };
    } catch (error) {
        return {
            status: 500,
            body: JSON.stringify({ 
                error: 'Failed to update user role',
                details: error.toString()
            }),
            contentType: 'application/json'
        };
    }
}
