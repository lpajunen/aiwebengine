# Deployment Workflow

This guide covers different ways to deploy and manage scripts in aiwebengine, from local development to production deployment.

## Overview

aiwebengine supports multiple deployment workflows:

1. **Web Editor** - Browser-based development and deployment
2. **Deployer Tool** - CLI tool for rapid deployment
3. **Direct File Placement** - Copy files to scripts directory
4. **API-based Deployment** - Programmatic deployment via REST API
5. **Git-based Workflow** - Version control integration

## Understanding Script Lifecycle

### Script States

```text
1. Created/Edited → 2. Saved → 3. Loaded → 4. Initialized → 5. Active
```

1. **Created/Edited** - Script exists as code
2. **Saved** - Script is persisted to storage
3. **Loaded** - Script is read by the engine
4. **Initialized** - `init()` function runs, routes registered
5. **Active** - Script responds to requests

### Hot Reloading

aiwebengine supports hot reloading:

- Save a script
- Engine automatically reloads it
- Routes are re-registered
- No server restart needed

## Workflow 1: Web Editor (Recommended for Beginners)

Best for: Quick prototyping, learning, small projects

### Advantages

✅ No local development environment needed
✅ Built-in AI assistant
✅ Instant preview and testing
✅ Visual asset management
✅ Integrated log viewer

### Workflow Steps

**Development:**

1. Open `/editor` in browser
2. Create or edit scripts
3. Save changes (auto-reloads)
4. Test immediately
5. Check logs for errors

**Example:**

```javascript
// Create api/hello.js in the editor
function helloHandler(req) {
  return {
    status: 200,
    body: "Hello from editor!",
    contentType: "text/plain; charset=UTF-8",
  };
}

function init() {
  routeRegistry.registerRoute("/api/hello", "helloHandler", "GET");
}

init();
```

Click Save → Test at `/api/hello` → View logs

### When to Use

- 🎯 Learning aiwebengine
- 🎯 Quick experiments
- 🎯 Small scripts
- 🎯 Demo applications
- 🎯 Remote server without local access

## Workflow 2: Deployer Tool (Recommended for Developers)

Best for: Local development, version control, team collaboration

### Advantages

✅ Use your favorite code editor (VS Code, etc.)
✅ Local version control with Git
✅ Test locally before deploying
✅ Batch deployment of multiple scripts
✅ CI/CD integration

### Setup

The deployer tool is built with your aiwebengine installation:

```bash
cargo build --release
```

Binary location:

```bash
target/release/deployer
```

### Basic Usage

**Deploy a single script:**

```bash
./target/release/deployer \
  --uri "http://localhost:8080/api/hello" \
  --file "./scripts/api/hello.js"
```

**Parameters:**

- `--uri` - Full URL where script will be accessible
- `--file` - Local path to your script file

### Development Workflow

**Step 1: Create script locally**

```bash
# Create directory
mkdir -p my-scripts/api

# Create script
cat > my-scripts/api/users.js << 'EOF'
function usersHandler(req) {
  return {
    status: 200,
    body: JSON.stringify({ users: [] }),
    contentType: "application/json"
  };
}

function init() {
  routeRegistry.registerRoute("/api/users", "usersHandler", "GET");
}

init();
EOF
```

**Step 2: Test locally**

```bash
# Start aiwebengine locally
cargo run

# In another terminal, deploy
./target/release/deployer \
  --uri "http://localhost:8080/api/users" \
  --file "./my-scripts/api/users.js"
```

**Step 3: Test the endpoint**

```bash
curl http://localhost:8080/api/users
```

**Step 4: Iterate**

1. Edit `my-scripts/api/users.js` in your editor
2. Re-run deployer command
3. Test again
4. Repeat until satisfied

**Step 5: Deploy to production**

```bash
./target/release/deployer \
  --uri "https://production.example.com/api/users" \
  --file "./my-scripts/api/users.js"
```

### Deploying Multiple Scripts

**Create a deployment script:**

```bash
#!/bin/bash
# deploy.sh

SERVER="http://localhost:8080"
DEPLOYER="./target/release/deployer"

# Deploy all API scripts
for script in ./scripts/api/*.js; do
  filename=$(basename "$script")
  path="/api/${filename%.js}"

  echo "Deploying $filename to $path..."
  $DEPLOYER --uri "$SERVER$path" --file "$script"
done

echo "Deployment complete!"
```

Make it executable:

```bash
chmod +x deploy.sh
./deploy.sh
```

### Using with Git

**Example Git workflow:**

```bash
# Initialize repo
git init
git add my-scripts/
git commit -m "Initial commit"

# Create a branch for new feature
git checkout -b feature/user-management

# Edit scripts
vim my-scripts/api/users.js

# Test locally
./deploy.sh

# Commit changes
git add my-scripts/api/users.js
git commit -m "Add user management API"

# Merge to main
git checkout main
git merge feature/user-management

# Deploy to production
./deploy-production.sh
```

### When to Use

- 🎯 Professional development
- 🎯 Team collaboration
- 🎯 Version control needed
- 🎯 Multiple environments (dev, staging, prod)
- 🎯 CI/CD pipelines

## Workflow 3: Direct File Placement

Best for: Server administrators, automated deployments

### How It Works

Place JavaScript files directly in the `scripts/` directory:

```text
aiwebengine/
├── scripts/
│   ├── api/
│   │   ├── users.js
│   │   └── posts.js
│   ├── pages/
│   │   └── home.js
│   └── features/
│       └── chat.js
```

The engine automatically:

1. Scans the `scripts/` directory
2. Loads all `.js` files
3. Executes `init()` function in each
4. Registers routes

### Development Workflow

**Local development:**

```bash
# Edit files directly
vim scripts/api/users.js

# Restart server (or wait for auto-reload if enabled)
cargo run
```

**Docker deployment:**

```dockerfile
# Dockerfile
FROM aiwebengine:latest

# Copy scripts
COPY ./scripts /app/scripts

# Copy assets
COPY ./assets /app/assets
```

Build and run:

```bash
docker build -t my-app .
docker run -p 8080:8080 my-app
```

### When to Use

- 🎯 Docker deployments
- 🎯 Server with file system access
- 🎯 Automated deployment scripts
- 🎯 Simple, straightforward workflow

## Workflow 4: API-based Deployment

Best for: Integration with other tools, programmatic deployment

### Using the Scripts API

aiwebengine exposes REST APIs for script management (if editor is enabled):

**Create/Update a script:**

```bash
curl -X POST "http://localhost:8080/api/scripts/api/hello.js" \
  -H "Content-Type: application/json" \
  -d '{
    "content": "function helloHandler(req) { return { status: 200, body: \"Hello\" }; } function init() { routeRegistry.registerRoute(\"/api/hello\", \"helloHandler\", \"GET\"); } init();"
  }'
```

**Get script content:**

```bash
curl http://localhost:8080/api/scripts/api/hello.js
```

**Delete a script:**

```bash
curl -X DELETE http://localhost:8080/api/scripts/api/hello.js
```

**List all scripts:**

```bash
curl http://localhost:8080/api/scripts
```

### Custom Deployment Tool

Create your own deployment tool:

```javascript
// deploy.mjs
import fs from "fs";
import fetch from "node-fetch";

async function deployScript(serverUrl, scriptPath, localFile) {
  const content = fs.readFileSync(localFile, "utf8");

  const response = await fetch(`${serverUrl}/api/scripts/${scriptPath}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ content }),
  });

  if (response.ok) {
    console.log(`✓ Deployed ${scriptPath}`);
  } else {
    console.error(`✗ Failed to deploy ${scriptPath}:`, await response.text());
  }
}

// Usage
await deployScript(
  "http://localhost:8080",
  "api/users.js",
  "./scripts/api/users.js",
);
```

### When to Use

- 🎯 Integration with existing tools
- 🎯 Custom deployment pipelines
- 🎯 Automated testing workflows
- 🎯 Third-party IDE integration

## Workflow 5: Git-based Workflow

Best for: Professional teams, version control requirements

### Repository Structure

```text
my-aiwebengine-app/
├── .git/
├── scripts/
│   ├── api/
│   ├── pages/
│   └── features/
├── assets/
│   ├── css/
│   ├── js/
│   └── images/
├── tests/
├── .gitignore
├── README.md
└── deploy.sh
```

### Gitignore Example

```gitignore
# .gitignore
target/
logs/
*.log
.env
.DS_Store
```

### Development Workflow

**1. Clone repository:**

```bash
git clone https://github.com/your-org/your-app.git
cd your-app
```

**2. Create feature branch:**

```bash
git checkout -b feature/new-api
```

**3. Develop locally:**

```bash
# Edit scripts
vim scripts/api/newfeature.js

# Test locally
./deploy-local.sh
curl http://localhost:8080/api/newfeature
```

**4. Commit changes:**

```bash
git add scripts/api/newfeature.js
git commit -m "Add new feature API"
git push origin feature/new-api
```

**5. Code review:**

- Create pull request
- Team reviews changes
- CI runs automated tests

**6. Merge and deploy:**

```bash
git checkout main
git merge feature/new-api
./deploy-production.sh
```

### CI/CD Integration

**GitHub Actions example:**

```yaml
# .github/workflows/deploy.yml
name: Deploy to aiwebengine

on:
  push:
    branches: [main]

jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v2

      - name: Deploy scripts
        run: |
          for script in scripts/**/*.js; do
            ./deployer \
              --uri "${{ secrets.SERVER_URL }}/$(basename $script .js)" \
              --file "$script"
          done
```

### When to Use

- 🎯 Team collaboration
- 🎯 Code review process
- 🎯 Version history important
- 🎯 Multiple developers
- 🎯 Professional projects

## Comparing Workflows

| Workflow      | Ease of Use | Version Control | Team Collaboration | CI/CD Ready | Best For              |
| ------------- | ----------- | --------------- | ------------------ | ----------- | --------------------- |
| Web Editor    | ⭐⭐⭐⭐⭐  | ❌              | ⭐                 | ❌          | Learning, prototyping |
| Deployer Tool | ⭐⭐⭐⭐    | ✅              | ⭐⭐⭐⭐           | ✅          | Professional dev      |
| Direct File   | ⭐⭐⭐      | ✅              | ⭐⭐               | ⭐⭐⭐      | Server admins         |
| API-based     | ⭐⭐        | ✅              | ⭐⭐⭐             | ✅          | Tool integration      |
| Git-based     | ⭐⭐⭐      | ✅              | ⭐⭐⭐⭐⭐         | ✅          | Teams                 |

## Environment-Specific Deployment

### Local Development

```bash
# config.local.toml
[server]
host = "localhost"
port = 8080

[scripts]
auto_reload = true
```

Deploy locally:

```bash
./target/release/deployer \
  --uri "http://localhost:8080/api/test" \
  --file "./scripts/api/test.js"
```

### Staging Environment

```bash
# config.staging.toml
[server]
host = "0.0.0.0"
port = 8080

[scripts]
auto_reload = true  # Still enable for testing
```

Deploy to staging:

```bash
./target/release/deployer \
  --uri "https://staging.example.com/api/test" \
  --file "./scripts/api/test.js"
```

### Production Environment

```bash
# config.production.toml
[server]
host = "0.0.0.0"
port = 8080

[scripts]
auto_reload = false  # Disable for stability
```

Deploy to production:

```bash
./target/release/deployer \
  --uri "https://api.example.com/api/test" \
  --file "./scripts/api/test.js"
```

## Best Practices

### 1. Test Before Deploying

Always test scripts in a dev/staging environment:

```bash
# Test locally first
./deploy-local.sh
./run-tests.sh

# Then deploy to staging
./deploy-staging.sh
./run-tests.sh

# Finally deploy to production
./deploy-production.sh
```

### 2. Use Version Control

Even with web editor, export and commit:

```bash
# Export scripts from editor
curl http://localhost:8080/api/scripts > scripts-backup.json

# Commit to git
git add scripts-backup.json
git commit -m "Backup scripts $(date +%Y-%m-%d)"
```

### 3. Automate Deployment

Create deployment scripts:

```bash
#!/bin/bash
# deploy-all.sh

set -e  # Exit on error

echo "Deploying to $1..."

case $1 in
  local)
    SERVER="http://localhost:8080"
    ;;
  staging)
    SERVER="https://staging.example.com"
    ;;
  production)
    SERVER="https://api.example.com"
    ;;
  *)
    echo "Usage: $0 {local|staging|production}"
    exit 1
    ;;
esac

# Deploy all scripts
find scripts -name "*.js" -type f | while read script; do
  path="${script#scripts/}"
  path="/${path%.js}"

  echo "Deploying $script to $path..."
  ./deployer --uri "$SERVER$path" --file "$script"
done

echo "✓ Deployment complete!"
```

### 4. Monitor Deployments

After deploying, check:

```bash
# Check logs
curl http://localhost:8080/api/logs

# Test endpoints
curl http://localhost:8080/api/test

# Check registered routes
curl http://localhost:8080/api/scripts
```

### 5. Rollback Plan

Keep previous versions:

```bash
# Before deploying
cp scripts/api/users.js scripts/api/users.js.backup

# If deployment fails
mv scripts/api/users.js.backup scripts/api/users.js
./deploy.sh
```

## Troubleshooting

### Script Not Loading

**Check:**

- File has `.js` extension
- Valid JavaScript syntax
- `init()` function exists and is called
- File permissions are correct

### Routes Not Registering

**Check:**

- `routeRegistry.registerRoute()` called in `init()`
- Path starts with `/`
- Handler name matches function name
- No duplicate routes

### Deployer Tool Fails

**Check:**

- Server is running
- URL is correct
- File path exists
- Network connectivity

### Changes Not Appearing

**Check:**

- Script was saved
- Auto-reload is enabled (or restart server)
- Browser cache cleared
- Correct environment targeted

## Next Steps

Now that you understand deployment workflows:

1. **[Learn Script Development](../guides/scripts.md)** - Master script features
2. **[Explore Asset Management](../guides/assets.md)** - Work with static files
3. **[Study Logging](../guides/logging.md)** - Debug effectively
4. **[Use AI Development](../guides/ai-development.md)** - Accelerate with AI

## Quick Reference

### Deployer Command

```bash
./target/release/deployer --uri "URL" --file "PATH"
```

### Deployment Checklist

- [ ] Test locally
- [ ] Review code changes
- [ ] Check logs for errors
- [ ] Deploy to staging
- [ ] Test in staging
- [ ] Deploy to production
- [ ] Monitor production logs
- [ ] Test production endpoints

### Common Commands

```bash
# Deploy single script
./deployer --uri "http://localhost:8080/api/test" --file "./test.js"

# Test endpoint
curl http://localhost:8080/api/test

# View logs
curl http://localhost:8080/api/logs

# List scripts
curl http://localhost:8080/api/scripts
```

Happy deploying! 🚀
