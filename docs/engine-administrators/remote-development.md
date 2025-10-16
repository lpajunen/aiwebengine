# Remote Development with the Editor

aiwebengine includes a built-in web-based editor that allows you to manage scripts and assets remotely without needing local file access. This is perfect for quick prototyping, testing, and managing your application from any device with a web browser.

## Features

- **Script Editor**: Create, edit, and delete JavaScript scripts with syntax highlighting
- **Asset Manager**: Upload, view, and manage static assets (images, CSS, files)
- **Log Viewer**: Monitor server logs in real-time
- **Route Explorer**: View registered API endpoints
- **Modern UI**: Dark theme with responsive design

## Getting Started

1. **Start the server**:

   ```bash
   cargo run
   ```

2. **Access the editor**:
   Open your browser and navigate to: `http://localhost:3000/editor`

## Script Management

### Creating Scripts

- Click the **"New"** button to create a new script
- Enter a script name (this becomes the URI)
- Write your JavaScript code in the Monaco editor
- Click **"Save"** to persist changes

### Editing Scripts

- Click on any script in the sidebar to load it in the editor
- Use the Monaco editor with full JavaScript syntax highlighting
- Changes are auto-saved as you type
- Click **"Save"** to manually save

### Deleting Scripts

- Select a script from the sidebar
- Click the **"Delete"** button
- Confirm deletion when prompted

## Asset Management

### Uploading Assets

- Click **"Upload Assets"** to select files from your computer
- Supported formats: images, CSS, JavaScript, documents, etc.
- Files are uploaded via HTTP POST

### Viewing Assets

- Assets are displayed in a grid with previews for images
- Click on an asset to view/download it
- File information includes size and upload date

### Managing Assets

- Click the **download** button to save assets locally
- Click the **delete** button to remove assets
- Assets are served at `/assets/{filename}`

## Log Monitoring

### Viewing Logs

- Logs are displayed in real-time with timestamps
- Auto-refresh every 5 seconds
- Shows all server activity including script executions

### Manual Refresh

- Click **"Refresh"** to manually update logs
- Useful when auto-refresh is disabled

## API Testing

### Testing Endpoints

- Click **"Test API"** to test any registered endpoint
- Enter parameters and view responses
- Useful for debugging your scripts

### Route Explorer

- View all registered routes and their handlers
- See which scripts are active
- Monitor endpoint availability

## API Endpoints

The editor provides REST API endpoints for programmatic access:

### Scripts

- `GET /api/scripts` - List all scripts
- `GET /api/scripts/:name` - Get script content
- `POST /api/scripts/:name` - Save/update script
- `DELETE /api/scripts/:name` - Delete script

### Assets

- `GET /api/assets` - List all assets
- `GET /api/assets/:path` - Get asset data
- `POST /api/assets` - Upload asset
- `DELETE /api/assets/:path` - Delete asset

### Logs

- `GET /api/logs` - Get recent logs
- `GET /script_logs?uri=<script-uri>` - Get logs for a specific script

## Browser Support

- Chrome 70+
- Firefox 65+
- Safari 12+
- Edge 79+

## Development Tips

- Use the editor for rapid prototyping and testing
- Combine with local development for full workflow
- Scripts created in the editor are immediately active
- Use logs to debug script execution
- Assets uploaded here are served statically

## Security Notes

- The editor is intended for development use
- In production deployments, consider restricting access
- No authentication is implemented by default
- Be cautious with sensitive data in logs

## Next Steps


- Learn about [local development](local-development.md) for file-based workflows
- Check [examples](../solution-developers/examples.md) for script patterns
- Review [JavaScript APIs](../solution-developers/javascript-apis.md) for available functions

