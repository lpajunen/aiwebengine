# aiwebengine Editor

A web-based editor for managing scripts and assets in aiwebengine.

## Features

- **Script Editor**: Create, edit, and delete JavaScript scripts with syntax highlighting
- **Asset Manager**: Upload, view, and manage static assets
- **Log Viewer**: Monitor server logs in real-time
- **Route Explorer**: View registered API endpoints
- **Modern UI**: Dark theme with responsive design

## Getting Started

1. **Start the server**:

   ```bash
   cargo run
   ```

2. **Access the editor**:
   Open your browser and navigate to: `http://localhost:8080/editor`

## Usage

### Script Management

- **View Scripts**: Click on any script in the sidebar to load it in the editor
- **Create Script**: Click the "New" button to create a new script
- **Edit Script**: Use the Monaco editor with full JavaScript syntax highlighting
- **Save Script**: Click "Save" to persist changes
- **Delete Script**: Click "Delete" to remove a script

### Asset Management

- **Upload Assets**: Click "Upload Assets" to select files from your computer
- **View Assets**: Assets are displayed in a grid with previews for images
- **Download Assets**: Click the download button to save assets locally
- **Delete Assets**: Click the delete button to remove assets

### Log Monitoring

- **View Logs**: Logs are displayed in real-time with timestamps
- **Auto-refresh**: Logs automatically refresh every 5 seconds
- **Jump to latest**: Click "Jump to latest" to scroll the view to the newest log entry

### API Testing

- **Test Endpoints**: Click "Test API" to test any endpoint
- **Route Explorer**: View all registered routes (coming soon)

## File Structure

```
assets/
├── editor.css       # Styling (public asset)
└── editor.js        # Client-side functionality (public asset)

scripts/
└── feature_scripts/
    └── editor.js    # Backend API handlers + serves editor UI
```

Note: The editor HTML is embedded in `scripts/feature_scripts/editor.js` to provide a single unified endpoint at `/editor`.

## API Endpoints

The editor provides the following REST API endpoints:

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

## Technical Details

- **Frontend**: Vanilla JavaScript with Handlebars templates
- **Editor**: Monaco Editor (VS Code's editor)
- **Styling**: Custom CSS with dark theme
- **Backend**: JavaScript handlers using aiwebengine's API
- **Templates**: Handlebars for dynamic content rendering

## Browser Support

- Chrome 70+
- Firefox 65+
- Safari 12+
- Edge 79+

## Development

The editor is built using:

- **Monaco Editor**: For code editing with syntax highlighting
- **Handlebars**: For templating dynamic content
- **Fetch API**: For making HTTP requests
- **CSS Grid/Flexbox**: For responsive layout

## Security Notes

- The editor runs on the same server as your application
- All operations are performed with the same permissions as the server
- Consider adding authentication for production use
- File uploads are limited by server configuration

## Troubleshooting

### Editor not loading

- Ensure the server is running
- Check that `editor.js` is in the `scripts/feature_scripts/` directory
- Verify that `editor.css` and `editor.js` are in the `assets/` directory

### Scripts not saving

- Check server logs for error messages
- Ensure the script name doesn't contain invalid characters
- Verify server has write permissions

### Assets not uploading

- Check file size limits
- Ensure supported file types
- Verify server has write permissions to assets directory

## Future Enhancements

- User authentication
- Script templates
- Version control integration
- Collaborative editing
- API documentation generator
- Performance monitoring
- Backup and restore functionality
