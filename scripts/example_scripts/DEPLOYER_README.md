# Deployer Tool

The `deployer` is a command-line tool that helps you develop JavaScript scripts for aiwebengine by automatically uploading them to a running server and watching for changes.

## Features

- 🚀 **One-time deployment**: Upload a script and exit
- 👀 **File watching**: Automatically redeploy when the file changes
- ⚡ **Fast feedback**: Instant deployment on save
- 🛠️ **Flexible configuration**: Custom server URLs and script URIs

## Installation

The deployer is built as part of the aiwebengine project:

```bash
cargo build --release --bin deployer
```

## Usage

### Basic Usage

```bash
# Deploy a script once
cargo run --bin deployer --uri "https://example.com/my-script" --file "my-script.js"

# Deploy and watch for changes
cargo run --bin deployer --uri "https://example.com/my-script" --file "my-script.js" --watch true
```

### Command Line Options

- `-u, --uri <URI>`: URI for the script (required)
  - Example: `https://example.com/blog`, `https://myapp.com/api-handler`
- `-f, --file <FILE>`: Path to the JavaScript file (required)
- `-s, --server <SERVER>`: Server URL (default: `http://localhost:3000`)
- `-w, --watch`: Watch for file changes (default: `true`)

### Examples

#### Deploying Example Scripts

```bash
# Deploy the blog example
cargo run --bin deployer \
  --uri "https://example.com/blog" \
  --file "scripts/example_scripts/blog.js"

# Deploy the feedback form
cargo run --bin deployer \
  --uri "https://example.com/feedback" \
  --file "scripts/example_scripts/feedback.js"
```

#### Development Workflow

1. **Start the server**:

   ```bash
   cargo run --bin server
   ```

2. **Start the deployer** in another terminal:

   ```bash
   cargo run --bin deployer \
     --uri "https://example.com/my-feature" \
     --file "src/my-feature.js"
   ```

3. **Edit your script** - changes are automatically deployed!

4. **Test your endpoint** at `http://localhost:3000/my-feature`

#### Custom Server

If your server is running on a different port or host:

```bash
cargo run --bin deployer \
  --server "http://localhost:8080" \
  --uri "https://example.com/test" \
  --file "test.js"
```

## How It Works

1. **Reads the file**: The deployer reads your JavaScript file
2. **Uploads via HTTP**: Sends a POST request to `/api/scripts/{uri}` with the file content
3. **Watches for changes**: Uses file system events to detect modifications
4. **Auto-redeploys**: When changes are detected, re-uploads the script

## Error Handling

The deployer provides clear feedback:

- ✅ **Success**: Script deployed successfully
- ❌ **File not found**: Specified file doesn't exist
- ❌ **Server unreachable**: Cannot connect to the server
- ❌ **Deployment failed**: Server returned an error

## Integration with Development

### VS Code Integration

You can add a task to your VS Code workspace for easy deployment:

```json
{
  "version": "2.0.0",
  "tasks": [
    {
      "label": "Deploy Script",
      "type": "shell",
      "command": "cargo",
      "args": [
        "run",
        "--bin",
        "deployer",
        "--uri",
        "https://example.com/my-script",
        "--file",
        "${file}"
      ],
      "group": "build",
      "presentation": {
        "echo": true,
        "reveal": "always",
        "focus": false,
        "panel": "shared"
      }
    }
  ]
}
```

### Makefile Integration

```makefile
.PHONY: deploy-blog
deploy-blog:
	cargo run --bin deployer --uri "https://example.com/blog" --file "scripts/example_scripts/blog.js"

.PHONY: watch-script
watch-script:
	cargo run --bin deployer --uri "$(URI)" --file "$(FILE)"
```

## Troubleshooting

### Server Not Running

```
❌ Failed to deploy script: https://example.com/test (Status: 000)
```

**Solution**: Make sure the aiwebengine server is running:

```bash
cargo run --bin server
```

### File Not Found

```
❌ Error: File 'my-script.js' does not exist
```

**Solution**: Check the file path and ensure the file exists.

### Permission Issues

If you get permission errors, make sure the server has write access to its data directory.

## Advanced Usage

### Multiple Deployers

You can run multiple deployers for different scripts:

```bash
# Terminal 1: Deploy blog
cargo run --bin deployer --uri "https://example.com/blog" --file "blog.js"

# Terminal 2: Deploy API
cargo run --bin deployer --uri "https://example.com/api" --file "api.js"

# Terminal 3: Deploy auth
cargo run --bin deployer --uri "https://example.com/auth" --file "auth.js"
```

### CI/CD Integration

The deployer can be used in CI/CD pipelines to automatically deploy scripts:

```yaml
# .github/workflows/deploy.yml
name: Deploy Scripts
on: [push]
jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v2
      - name: Deploy to staging
        run: |
          cargo run --bin deployer \
            --server "https://staging.myapp.com" \
            --uri "https://myapp.com/api" \
            --file "api.js" \
            --watch false
```
