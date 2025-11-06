# Deployer Tool Guide

The deployer is a command-line tool for rapid development and deployment of aiwebengine scripts. It uploads scripts to a running server and can watch for changes to automatically redeploy.

## Overview

**Key Features:**

- ✅ **One-time deployment** - Upload a script and exit
- ✅ **File watching** - Auto-redeploy when file changes
- ✅ **Fast feedback** - Instant deployment on save
- ✅ **Flexible configuration** - Custom URLs and paths
- ✅ **Multiple scripts** - Deploy many scripts simultaneously
- ✅ **CI/CD ready** - Integration with automated pipelines

## Installation

### Build the Deployer

The deployer is part of the aiwebengine project:

```bash
cd /path/to/aiwebengine
cargo build --release --bin deployer
```

Binary location:

```bash
./target/release/deployer
```

### Add to PATH (Optional)

For easier access:

```bash
# macOS/Linux
sudo cp target/release/deployer /usr/local/bin/

# Or add to PATH
export PATH="$PATH:/path/to/aiwebengine/target/release"
```

Now you can run `deployer` from anywhere.

## Basic Usage

### Command Syntax

```bash
deployer --uri <URI> --file <FILE> [OPTIONS]
```

Or with cargo:

```bash
cargo run --bin deployer -- --uri <URI> --file <FILE> [OPTIONS]
```

### Required Arguments

**`--uri` or `-u`**  
The URI/path where the script will be accessible

```bash
--uri "http://localhost:8080/api/users"
--uri "http://localhost:8080/hello"
```

**`--file` or `-f`**  
Path to the JavaScript file to deploy

```bash
--file "./scripts/api/users.js"
--file "my-script.js"
```

### Optional Arguments

**`--server` or `-s`**  
Server base URL (default: `http://localhost:3000`)

```bash
--server "http://localhost:8080"
--server "https://staging.example.com"
```

**`--watch` or `-w`**  
Watch for file changes and auto-redeploy (default: `true`)

```bash
--watch true   # Enable watching (default)
--watch false  # Deploy once and exit
```

## Examples

### Example 1: Deploy Once

Deploy a script without watching:

```bash
deployer \
  --uri "http://localhost:8080/hello" \
  --file "./scripts/hello.js" \
  --watch false
```

Output:

```
✓ Script deployed successfully: /hello
```

### Example 2: Deploy with Watching

Deploy and watch for changes:

```bash
deployer \
  --uri "http://localhost:8080/api/users" \
  --file "./scripts/api/users.js"
```

Output:

```
✓ Script deployed successfully: /api/users
👀 Watching for changes... (Press Ctrl+C to stop)
```

Now edit `./scripts/api/users.js` and save. The deployer will automatically redeploy.

### Example 3: Deploy to Remote Server

Deploy to a staging or production server:

```bash
deployer \
  --server "https://staging.example.com" \
  --uri "https://staging.example.com/api/products" \
  --file "./scripts/api/products.js" \
  --watch false
```

### Example 4: Deploy Example Scripts

```bash
# Deploy blog example
deployer \
  --uri "http://localhost:8080/blog" \
  --file "./scripts/example_scripts/blog.js"

# Deploy feedback form
deployer \
  --uri "http://localhost:8080/feedback" \
  --file "./scripts/example_scripts/feedback.js"
```

## Development Workflows

### Workflow 1: Local Development

**Terminal 1 - Start server:**

```bash
cargo run
```

**Terminal 2 - Deploy with watching:**

```bash
deployer \
  --uri "http://localhost:8080/my-feature" \
  --file "./scripts/my-feature.js"
```

**Your editor:**

1. Edit `./scripts/my-feature.js`
2. Save file (Cmd+S / Ctrl+S)
3. Deployer auto-uploads
4. Test at `http://localhost:8080/my-feature`
5. Repeat

### Workflow 2: Multiple Scripts

Run multiple deployers in separate terminals:

**Terminal 1:**

```bash
deployer --uri "http://localhost:8080/api/users" --file "./api/users.js"
```

**Terminal 2:**

```bash
deployer --uri "http://localhost:8080/api/posts" --file "./api/posts.js"
```

**Terminal 3:**

```bash
deployer --uri "http://localhost:8080/" --file "./pages/home.js"
```

All scripts watch and auto-deploy independently.

### Workflow 3: Batch Deployment Script

Create a deployment script:

**`deploy.sh`:**

```bash
#!/bin/bash

SERVER="http://localhost:8080"
DEPLOYER="./target/release/deployer"

echo "Deploying all scripts to $SERVER..."

# Deploy API scripts
$DEPLOYER --uri "$SERVER/api/users" --file "./scripts/api/users.js" --watch false
$DEPLOYER --uri "$SERVER/api/posts" --file "./scripts/api/posts.js" --watch false
$DEPLOYER --uri "$SERVER/api/auth" --file "./scripts/api/auth.js" --watch false

# Deploy pages
$DEPLOYER --uri "$SERVER/" --file "./scripts/pages/home.js" --watch false
$DEPLOYER --uri "$SERVER/about" --file "./scripts/pages/about.js" --watch false

echo "✓ All scripts deployed!"
```

Run it:

```bash
chmod +x deploy.sh
./deploy.sh
```

### Workflow 4: Environment-Based Deployment

**`deploy-local.sh`:**

```bash
#!/bin/bash
deployer \
  --server "http://localhost:8080" \
  --uri "http://localhost:8080/api/test" \
  --file "$1"
```

**`deploy-staging.sh`:**

```bash
#!/bin/bash
deployer \
  --server "https://staging.example.com" \
  --uri "https://staging.example.com/api/test" \
  --file "$1" \
  --watch false
```

**`deploy-production.sh`:**

```bash
#!/bin/bash
deployer \
  --server "https://api.example.com" \
  --uri "https://api.example.com/api/test" \
  --file "$1" \
  --watch false
```

Usage:

```bash
./deploy-local.sh scripts/api/test.js
./deploy-staging.sh scripts/api/test.js
./deploy-production.sh scripts/api/test.js
```

## IDE Integration

### VS Code Tasks

Add to `.vscode/tasks.json`:

```json
{
  "version": "2.0.0",
  "tasks": [
    {
      "label": "Deploy Current Script",
      "type": "shell",
      "command": "deployer",
      "args": [
        "--uri",
        "http://localhost:8080/${fileBasenameNoExtension}",
        "--file",
        "${file}"
      ],
      "group": "build",
      "presentation": {
        "reveal": "always",
        "panel": "dedicated"
      }
    },
    {
      "label": "Deploy and Watch",
      "type": "shell",
      "command": "deployer",
      "args": [
        "--uri",
        "http://localhost:8080/${fileBasenameNoExtension}",
        "--file",
        "${file}",
        "--watch",
        "true"
      ],
      "isBackground": true,
      "presentation": {
        "reveal": "always",
        "panel": "dedicated"
      }
    }
  ]
}
```

**Usage:**

1. Open a script file
2. Run task: Cmd+Shift+P → "Tasks: Run Task"
3. Select "Deploy Current Script" or "Deploy and Watch"

### Makefile

Add to `Makefile`:

```makefile
.PHONY: deploy watch deploy-all deploy-prod

# Deploy a single script
deploy:
	@deployer --uri "http://localhost:8080/$(PATH)" --file "$(FILE)" --watch false

# Deploy and watch
watch:
	@deployer --uri "http://localhost:8080/$(PATH)" --file "$(FILE)"

# Deploy all scripts
deploy-all:
	@./deploy.sh

# Deploy to production
deploy-prod:
	@deployer --server "$(PROD_SERVER)" --uri "$(URI)" --file "$(FILE)" --watch false
```

**Usage:**

```bash
# Deploy once
make deploy FILE=scripts/api/users.js PATH=api/users

# Deploy and watch
make watch FILE=scripts/api/users.js PATH=api/users

# Deploy all
make deploy-all

# Deploy to production
make deploy-prod FILE=scripts/api/users.js URI=https://api.example.com/api/users PROD_SERVER=https://api.example.com
```

### npm Scripts

If using `package.json` in your project:

```json
{
  "scripts": {
    "deploy": "deployer --uri http://localhost:8080/api/test --file scripts/api/test.js --watch false",
    "dev": "deployer --uri http://localhost:8080/api/test --file scripts/api/test.js",
    "deploy:staging": "deployer --server https://staging.example.com --uri https://staging.example.com/api/test --file scripts/api/test.js --watch false",
    "deploy:prod": "deployer --server https://api.example.com --uri https://api.example.com/api/test --file scripts/api/test.js --watch false"
  }
}
```

**Usage:**

```bash
npm run deploy      # Deploy once
npm run dev         # Deploy and watch
npm run deploy:staging
npm run deploy:prod
```

## CI/CD Integration

### GitHub Actions

`.github/workflows/deploy.yml`:

```yaml
name: Deploy Scripts

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

jobs:
  deploy:
    runs-on: ubuntu-latest

    steps:
      - uses: actions/checkout@v3

      - name: Install Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable

      - name: Build deployer
        run: cargo build --release --bin deployer

      - name: Deploy to staging
        if: github.event_name == 'pull_request'
        run: |
          ./target/release/deployer \
            --server "${{ secrets.STAGING_SERVER }}" \
            --uri "${{ secrets.STAGING_SERVER }}/api/test" \
            --file "scripts/api/test.js" \
            --watch false

      - name: Deploy to production
        if: github.event_name == 'push' && github.ref == 'refs/heads/main'
        run: |
          ./target/release/deployer \
            --server "${{ secrets.PROD_SERVER }}" \
            --uri "${{ secrets.PROD_SERVER }}/api/test" \
            --file "scripts/api/test.js" \
            --watch false
```

### GitLab CI

`.gitlab-ci.yml`:

```yaml
stages:
  - build
  - deploy

build:
  stage: build
  script:
    - cargo build --release --bin deployer
  artifacts:
    paths:
      - target/release/deployer

deploy_staging:
  stage: deploy
  only:
    - develop
  script:
    - ./target/release/deployer
      --server "$STAGING_SERVER"
      --uri "$STAGING_SERVER/api/test"
      --file "scripts/api/test.js"
      --watch false

deploy_production:
  stage: deploy
  only:
    - main
  script:
    - ./target/release/deployer
      --server "$PROD_SERVER"
      --uri "$PROD_SERVER/api/test"
      --file "scripts/api/test.js"
      --watch false
  when: manual
```

## How It Works

### Deployment Process

1. **Read file** - Deployer reads your JavaScript file
2. **HTTP POST** - Sends POST request to `/api/scripts/{uri}` with content
3. **Server processes** - aiwebengine saves and loads the script
4. **Routes register** - Script's `init()` function runs, routes registered
5. **Ready** - Script is immediately available at the specified URI

### Watch Mode

When watching (`--watch true`):

1. Deploys initially
2. Monitors file for changes using file system events
3. On change detected:
   - Waits for file to stabilize (handles editors that save in chunks)
   - Rereads file
   - Redeploys automatically
4. Continues until Ctrl+C

## Troubleshooting

### Server Not Reachable

**Error:**

```
❌ Failed to deploy: Connection refused
```

**Solutions:**

- Ensure server is running: `cargo run`
- Check server URL is correct
- Verify firewall allows connection
- Check network connectivity

### File Not Found

**Error:**

```
❌ Error: File 'script.js' does not exist
```

**Solutions:**

- Verify file path is correct
- Use relative or absolute paths
- Check current working directory

### Permission Denied

**Error:**

```
❌ Failed to deploy: Forbidden (403)
```

**Solutions:**

- Check server authentication requirements
- Verify server write permissions
- Ensure correct credentials if auth is enabled

### Script Syntax Errors

**Error:**

```
✓ Script deployed successfully
(But script doesn't work)
```

**Solutions:**

- Check script syntax in editor
- View server logs for JavaScript errors
- Test script in `/editor` for error messages
- Add `console.log()` for debugging

### Watch Not Working

**Problem:** File changes not detected

**Solutions:**

- Try saving file again
- Check file system events are working on your OS
- Restart deployer
- Use `--watch false` and deploy manually

## Best Practices

### 1. Use Watch Mode During Development

```bash
# Development
deployer --uri "http://localhost:8080/test" --file "test.js"

# Edit → Save → Auto-deploy → Test → Repeat
```

### 2. Disable Watch for Production

```bash
# Production
deployer \
  --server "https://api.example.com" \
  --uri "https://api.example.com/test" \
  --file "test.js" \
  --watch false
```

### 3. Create Deployment Scripts

Don't type long commands repeatedly. Create scripts:

```bash
# dev.sh
deployer --uri "http://localhost:8080/api/test" --file "api/test.js"

# prod.sh
deployer --server "$PROD" --uri "$PROD/api/test" --file "api/test.js" --watch false
```

### 4. Use Absolute Paths for Clarity

```bash
# Relative (can be confusing)
deployer --file "../scripts/api/test.js"

# Absolute (clear)
deployer --file "/Users/you/project/scripts/api/test.js"
```

### 5. Test Locally Before Production

```bash
# 1. Test locally
deployer --uri "http://localhost:8080/test" --file "test.js" --watch false
curl http://localhost:8080/test

# 2. Deploy to staging
deployer --server "https://staging.example.com" --uri "https://staging.example.com/test" --file "test.js" --watch false
curl https://staging.example.com/test

# 3. Deploy to production
deployer --server "https://api.example.com" --uri "https://api.example.com/test" --file "test.js" --watch false
```

### 6. Version Your Deployment Scripts

Commit deployment scripts to Git:

```bash
git add deploy.sh deploy-staging.sh deploy-prod.sh
git commit -m "Add deployment scripts"
```

## Advanced Usage

### Deploy with Environment Variables

```bash
# Set environment
export DEPLOY_SERVER="http://localhost:8080"
export DEPLOY_FILE="scripts/api/users.js"
export DEPLOY_URI="$DEPLOY_SERVER/api/users"

# Deploy
deployer --server "$DEPLOY_SERVER" --uri "$DEPLOY_URI" --file "$DEPLOY_FILE"
```

### Deploy All Files in Directory

```bash
#!/bin/bash
for file in scripts/api/*.js; do
  name=$(basename "$file" .js)
  deployer \
    --uri "http://localhost:8080/api/$name" \
    --file "$file" \
    --watch false
done
```

### Conditional Deployment

```bash
#!/bin/bash
if [ "$1" == "prod" ]; then
  SERVER="https://api.example.com"
else
  SERVER="http://localhost:8080"
fi

deployer --server "$SERVER" --uri "$SERVER/api/test" --file "api/test.js" --watch false
```

## Next Steps

- **[Web Editor](editor.md)** - Browser-based development
- **[External Tools](external-tools.md)** - VS Code, Git workflows
- **[Deployment Workflow](../getting-started/03-deployment-workflow.md)** - All deployment methods
- **[Script Development](../guides/scripts.md)** - Build better scripts

## Quick Reference

```bash
# Basic deploy
deployer --uri "http://localhost:8080/test" --file "test.js"

# Deploy and watch
deployer --uri "http://localhost:8080/test" --file "test.js" --watch true

# Deploy once
deployer --uri "http://localhost:8080/test" --file "test.js" --watch false

# Deploy to remote
deployer --server "https://api.example.com" --uri "https://api.example.com/test" --file "test.js"
```

The deployer tool is your command-line companion for rapid aiwebengine development! 🚀
