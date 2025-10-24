# User Management UI Documentation

## Overview

The Manager UI (`/manager`) provides administrators with a web interface to view and manage user roles in the AIWebEngine system.

## Access Control

**Administrator Access Only**: The manager UI and its API endpoints are restricted to users with administrator privileges. Non-admin users will receive a 403 Forbidden error.

### Getting Administrator Access

To use the Manager UI, you need Administrator privileges. There are two ways to get this:

1. **Bootstrap Admin (Recommended for Initial Setup)**: Configure your email as a bootstrap admin in the configuration file. When you first sign in, you'll automatically receive Administrator role. See [Bootstrap Admin Configuration](./BOOTSTRAP_ADMIN.md) for details.

   ```toml
   [auth]
   bootstrap_admins = ["your.email@company.com"]
   ```

2. **Granted by Another Admin**: An existing administrator can grant you the Administrator role through the Manager UI.

### Endpoints

- **UI Endpoint**: `/manager` (GET) - Serves the management interface
- **API Endpoints**:
  - `/api/manager/users` (GET) - List all users
  - `/api/manager/users/:userId/roles` (POST) - Add or remove user roles

## Features

### 1. User Dashboard

The main dashboard displays:

- **Statistics Cards**:
  - Total number of users
  - Number of administrators
  - Number of editors

- **User Table**: Shows all users with:
  - Email address
  - Display name
  - Current roles (with color-coded badges)
  - OAuth providers used
  - Account creation date
  - Role management actions

### 2. Role Management

Administrators can:

- **Add Editor Role**: Grant editor privileges to a user
- **Remove Editor Role**: Revoke editor privileges from a user
- **Add Administrator Role**: Grant administrator privileges to a user
- **Remove Administrator Role**: Revoke administrator privileges from a user

**Note**: The `Authenticated` role cannot be removed as it's the base role for all users.

### 3. Visual Design

The UI features:

- Modern gradient background (purple theme)
- Card-based layout
- Responsive design (mobile-friendly)
- Color-coded role badges:
  - Blue for Authenticated
  - Orange for Editor
  - Pink for Administrator
- Hover effects and smooth transitions
- Real-time updates after role changes

## API Endpoints

### List Users

**Endpoint**: `GET /api/manager/users`

**Authentication**: Required (Admin only)

**Response**:

```json
{
  "users": [
    {
      "id": "uuid-here",
      "email": "user@example.com",
      "name": "John Doe",
      "roles": ["Authenticated", "Editor"],
      "created_at": "SystemTime { tv_sec: 1234567890, tv_nsec: 0 }",
      "providers": ["google", "github"]
    }
  ],
  "total": 1
}
```

### Update User Role

**Endpoint**: `POST /api/manager/users/:userId/roles`

**Authentication**: Required (Admin only)

**Request Body**:

```json
{
  "role": "Editor", // or "Administrator"
  "action": "add" // or "remove"
}
```

**Response**:

```json
{
  "success": true,
  "userId": "uuid-here",
  "role": "Editor",
  "action": "add"
}
```

**Error Responses**:

- `401 Unauthorized`: User not authenticated
- `403 Forbidden`: User is not an administrator
- `400 Bad Request`: Invalid parameters
- `500 Internal Server Error`: Server-side error

## Usage Examples

### Accessing the Manager UI

1. Ensure you're logged in as an administrator
2. Navigate to `/manager` in your browser
3. View the user list and statistics

### Adding Editor Role to a User

1. Locate the user in the table
2. Click "Add Editor" button
3. The role is added immediately
4. The button changes to "Remove Editor"

### Removing Administrator Role

1. Find the user with Administrator role
2. Click "Remove Admin" button
3. Confirmation message appears
4. Role is removed from the user

## JavaScript API Functions

The manager script exposes these functions to the JavaScript runtime (admin-only):

### `listUsers()`

Returns a JSON string containing all users.

**Usage**:

```javascript
const usersJson = listUsers();
const users = JSON.parse(usersJson);
```

### `addUserRole(userId, role)`

Adds a role to a user.

**Parameters**:

- `userId` (string): The user's internal ID
- `role` (string): "Editor", "Administrator", or "Authenticated"

**Usage**:

```javascript
addUserRole("user-id-123", "Editor");
```

### `removeUserRole(userId, role)`

Removes a role from a user.

**Parameters**:

- `userId` (string): The user's internal ID
- `role` (string): "Editor" or "Administrator" (cannot remove "Authenticated")

**Usage**:

```javascript
removeUserRole("user-id-123", "Editor");
```

## Implementation Details

### File: `/scripts/feature_scripts/manager.js`

The manager script is automatically loaded at server startup and registers three routes:

1. **UI Route**: Serves the HTML/CSS/JavaScript for the management interface
2. **List API**: Returns user data as JSON
3. **Update API**: Processes role changes

### Security Features

- **Authentication Check**: All endpoints verify the user is authenticated
- **Authorization Check**: All endpoints require administrator capabilities
- **Input Validation**: Role and action parameters are validated
- **Capability-Based Access**: Uses Rust-side capability checking (DeleteScripts capability indicates admin)
- **Audit Logging**: All role changes are logged with admin ID, target user, and action

### Integration with User Repository

The manager UI integrates directly with the `user_repository` module:

- `user_repository::list_users()` - Fetch all users
- `user_repository::add_user_role()` - Add role to user
- `user_repository::remove_user_role()` - Remove role from user

## Testing

Run the manager tests:

```bash
cargo test --test manager
```

Tests verify:

- ✅ Manager script loads correctly
- ✅ Manager script executes successfully
- ✅ Init function registers all routes
- ✅ Routes are accessible

## Navigation

The manager UI includes quick navigation links to:

- **Home** (`/`) - Main application
- **Editor** (`/editor`) - Script editor
- **GraphQL** (`/graphql`) - GraphQL playground

## Error Handling

The UI displays errors in red alert boxes for:

- Failed API requests
- Network errors
- Permission denied errors

Success messages are shown in green for:

- Successful role additions
- Successful role removals

## Browser Compatibility

The UI uses modern web standards and is compatible with:

- Chrome/Edge (latest)
- Firefox (latest)
- Safari (latest)
- Mobile browsers (iOS Safari, Chrome Mobile)

## Responsive Design

The UI adapts to different screen sizes:

- **Desktop**: Full table layout with all columns
- **Tablet**: Compact layout with smaller fonts
- **Mobile**: Stacked layout with full-width buttons

## Future Enhancements

Potential improvements:

1. **Search/Filter**: Search users by email or name
2. **Sorting**: Sort table by any column
3. **Pagination**: Support for large user lists
4. **Bulk Operations**: Select multiple users for batch role changes
5. **Role History**: View when roles were added/removed
6. **User Details Modal**: Click user to see full details and history
7. **Export**: Export user list to CSV
8. **Custom Roles**: Support for custom role definitions

## Troubleshooting

### Cannot Access Manager UI

**Problem**: Receiving 403 Forbidden error

**Solution**: Ensure you're logged in as an administrator. Check user roles in the database.

### Role Changes Not Persisting

**Problem**: Roles revert after page refresh

**Solution**: Check server logs for errors. Ensure user repository is functioning correctly.

### UI Not Loading

**Problem**: Blank page or JavaScript errors

**Solution**:

1. Check browser console for errors
2. Verify manager.js is loaded (`fetch_scripts()` includes it)
3. Ensure server is running with script initialization enabled

## Security Considerations

1. **Admin-Only Access**: All endpoints check for administrator capabilities
2. **No Client-Side Bypass**: Authorization is enforced server-side
3. **Audit Trail**: All role changes are logged
4. **No Password Exposure**: User passwords are never transmitted or displayed
5. **HTTPS Required**: Use HTTPS in production to protect session cookies

## Configuration

The manager UI uses the default server configuration. No additional configuration is required.

To disable the manager UI, you would need to:

1. Remove manager.js from the repository
2. Or implement a configuration flag to skip loading it

## Related Documentation

- [User Repository Implementation](./USER_REPOSITORY_IMPLEMENTATION.md)
- [User Repository Integration Guide](./USER_REPOSITORY_INTEGRATION.md)
- [Authentication System](./AUTH_DEBUGGING_GUIDE.md)

## Support

For issues or questions about the Manager UI:

1. Check server logs for errors
2. Review user repository tests
3. Verify authentication is working
4. Check administrator role assignment
