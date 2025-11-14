# External Tools Guide

Learn how to integrate aiwebengine development with external tools like VS Code, Git, and CI/CD systems for a professional development workflow.

## Overview

While the web editor is great for quick edits, many developers prefer using their familiar tools and workflows. This guide covers integrating aiwebengine with:

- **VS Code** - Full-featured code editor with extensions
- **Git** - Version control for your scripts and assets
- **CI/CD** - Automated testing and deployment pipelines
- **Testing Tools** - Unit testing and integration testing
- **Other Editors** - Vim, Emacs, Sublime Text, etc.

## VS Code Integration

### Project Setup

**1. Create a workspace:**

```bash
mkdir my-aiwebengine-project
cd my-aiwebengine-project
```

**2. Initialize project structure:**

```bash
mkdir -p scripts/{api,pages,services}
mkdir -p assets/{css,js,images}
mkdir -p tests
```

**3. Open in VS Code:**

```bash
code .
```

### Recommended Extensions

Install these VS Code extensions for better aiwebengine development:

#### JavaScript Development

**ESLint** (`dbaeumer.vscode-eslint`)

- Lints your JavaScript code
- Catches errors before deployment

**Prettier** (`esbenp.prettier-vscode`)

- Auto-formats code consistently
- Configurable style rules

**JavaScript (ES6) Code Snippets** (`xabikos.JavaScriptSnippets`)

- Quick code generation
- Common patterns ready to use

#### API Development

**REST Client** (`humao.rest-client`)

- Test HTTP endpoints directly in VS Code
- No need for Postman/Insomnia

**Thunder Client** (`rangav.vscode-thunder-client`)

- Alternative REST client with GUI
- Collections for organizing requests

#### Productivity

**Path Intellisense** (`christian-kohler.path-intellisense`)

- Auto-complete file paths
- Reduces typos in asset references

**GitLens** (`eamonn.gitlens`)

- Enhanced Git integration
- See file history and blame

**Todo Tree** (`Gruntfuggly.todo-tree`)

- Track TODO comments
- Navigate pending tasks

### VS Code Tasks

Create `.vscode/tasks.json` for automated workflows:

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
        "${file}",
        "--watch",
        "false"
      ],
      "group": {
        "kind": "build",
        "isDefault": true
      },
      "presentation": {
        "reveal": "always",
        "panel": "dedicated"
      },
      "problemMatcher": []
    },
    {
      "label": "Deploy and Watch",
      "type": "shell",
      "command": "deployer",
      "args": [
        "--uri",
        "http://localhost:8080/${fileBasenameNoExtension}",
        "--file",
        "${file}"
      ],
      "isBackground": true,
      "presentation": {
        "reveal": "always",
        "panel": "dedicated"
      },
      "problemMatcher": []
    },
    {
      "label": "Run Tests",
      "type": "shell",
      "command": "npm",
      "args": ["test"],
      "group": {
        "kind": "test",
        "isDefault": true
      },
      "presentation": {
        "reveal": "always"
      }
    },
    {
      "label": "Deploy All Scripts",
      "type": "shell",
      "command": "./deploy-all.sh",
      "presentation": {
        "reveal": "always"
      }
    }
  ]
}
```

**Usage:**

- **Cmd+Shift+B** (Mac) or **Ctrl+Shift+B** (Windows/Linux) - Deploy current script
- **Cmd+Shift+P** → "Tasks: Run Task" - Choose any task

### VS Code Snippets

Create `.vscode/aiwebengine.code-snippets`:

```json
{
  "aiwebengine Basic Script": {
    "prefix": "awe-script",
    "body": [
      "function init() {",
      "  routeRegistry.registerRoute('${2:/}', '${3:handler}', '${1:GET}');",
      "}",
      "",
      "function ${3:handler}(request) {",
      "  return {",
      "    status: ${4:200},",
      "    headers: { 'Content-Type': '${5:application/json}' },",
      "    body: JSON.stringify({ ${6:message: 'Hello'} })",
      "  };",
      "}",
      "",
      "init();"
    ],
    "description": "Create a basic aiwebengine script"
  },
  "aiwebengine API Handler": {
    "prefix": "awe-api",
    "body": [
      "function ${1:handler}(request) {",
      "  const data = ${2:{ success: true \\}};",
      "  ",
      "  return {",
      "    status: 200,",
      "    headers: { 'Content-Type': 'application/json' },",
      "    body: JSON.stringify(data)",
      "  };",
      "}"
    ],
    "description": "Create an API handler function"
  },
  "aiwebengine HTML Response": {
    "prefix": "awe-html",
    "body": [
      "function ${1:handler}(request) {",
      "  const html = `",
      "    <!DOCTYPE html>",
      "    <html>",
      "    <head>",
      "      <title>${2:Page Title}</title>",
      "    </head>",
      "    <body>",
      "      <h1>${3:Hello World}</h1>",
      "    </body>",
      "    </html>",
      "  `;",
      "  ",
      "  return {",
      "    status: 200,",
      "    headers: { 'Content-Type': 'text/html' },",
      "    body: html",
      "  };",
      "}"
    ],
    "description": "Create an HTML response handler"
  },
  "aiwebengine Log": {
    "prefix": "awe-log",
    "body": ["console.log('${1:message}', { ${2:data: value} });"],
    "description": "Write a log message"
  }
}
```

**Usage:** Type `awe-` and press Tab to see snippet suggestions.

### Settings

Add to `.vscode/settings.json`:

```json
{
  "files.associations": {
    "*.js": "javascript"
  },
  "editor.formatOnSave": true,
  "editor.defaultFormatter": "esbenp.prettier-vscode",
  "eslint.validate": ["javascript"],
  "javascript.suggest.paths": true,
  "files.exclude": {
    "**/node_modules": true,
    "**/target": true,
    "**/.git": true
  },
  "search.exclude": {
    "**/node_modules": true,
    "**/target": true
  }
}
```

### Launch Configurations

Add to `.vscode/launch.json` for debugging:

```json
{
  "version": "0.2.0",
  "configurations": [
    {
      "type": "node",
      "request": "launch",
      "name": "Test Script Locally",
      "program": "${workspaceFolder}/tests/run-test.js",
      "args": ["${file}"]
    }
  ]
}
```

## Git Workflows

### Repository Structure

Recommended structure:

```
my-aiwebengine-project/
├── .git/
├── .gitignore
├── README.md
├── scripts/
│   ├── api/
│   │   ├── users.js
│   │   ├── posts.js
│   │   └── auth.js
│   ├── pages/
│   │   ├── home.js
│   │   ├── about.js
│   │   └── contact.js
│   └── services/
│       └── email.js
├── assets/
│   ├── css/
│   │   └── styles.css
│   ├── js/
│   │   └── app.js
│   └── images/
│       └── logo.png
├── tests/
│   └── api/
│       └── users.test.js
├── deploy.sh
└── package.json
```

### .gitignore

Create `.gitignore`:

```gitignore
# Dependencies
node_modules/
target/

# Environment files
.env
.env.local
.env.*.local
config.local.toml

# Secrets
secrets/
*.key
*.pem

# Logs
logs/
*.log

# OS files
.DS_Store
Thumbs.db

# Editor
.vscode/
.idea/
*.swp
*.swo

# Build artifacts
dist/
build/
```

### Git Workflow

#### Feature Branch Workflow

**1. Create feature branch:**

```bash
git checkout -b feature/user-api
```

**2. Develop and test:**

```bash
# Edit scripts/api/users.js
deployer --uri "http://localhost:8080/api/users" --file "scripts/api/users.js"

# Test endpoint
curl http://localhost:8080/api/users
```

**3. Commit changes:**

```bash
git add scripts/api/users.js
git commit -m "Add user API endpoint

- Implements GET /api/users
- Returns list of all users
- Includes pagination support"
```

**4. Push and create PR:**

```bash
git push origin feature/user-api
# Create pull request on GitHub/GitLab
```

**5. Deploy after merge:**

```bash
git checkout main
git pull
./deploy.sh
```

#### Commit Message Convention

Use conventional commits:

```bash
# Feature
git commit -m "feat: add user registration endpoint"

# Bug fix
git commit -m "fix: correct validation in login handler"

# Documentation
git commit -m "docs: update API documentation"

# Refactor
git commit -m "refactor: extract authentication logic"

# Performance
git commit -m "perf: optimize database queries"
```

### Git Hooks

Create `.git/hooks/pre-commit`:

```bash
#!/bin/bash

# Run linter
npm run lint
if [ $? -ne 0 ]; then
  echo "❌ Linting failed. Fix errors before committing."
  exit 1
fi

# Run tests
npm test
if [ $? -ne 0 ]; then
  echo "❌ Tests failed. Fix tests before committing."
  exit 1
fi

echo "✓ Pre-commit checks passed"
```

Make it executable:

```bash
chmod +x .git/hooks/pre-commit
```

## CI/CD Pipelines

### GitHub Actions

Create `.github/workflows/deploy.yml`:

```yaml
name: Deploy to aiwebengine

on:
  push:
    branches: [main, develop]
  pull_request:
    branches: [main]

env:
  CARGO_TERM_COLOR: always

jobs:
  test:
    runs-on: ubuntu-latest

    steps:
      - uses: actions/checkout@v3

      - name: Setup Node.js
        uses: actions/setup-node@v3
        with:
          node-version: "18"

      - name: Install dependencies
        run: npm ci

      - name: Run linter
        run: npm run lint

      - name: Run tests
        run: npm test

  deploy-staging:
    needs: test
    if: github.ref == 'refs/heads/develop'
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
        run: |
          for file in scripts/**/*.js; do
            path="${file#scripts/}"
            uri="${{ secrets.STAGING_URL }}/${path%.js}"
            ./target/release/deployer \
              --server "${{ secrets.STAGING_URL }}" \
              --uri "$uri" \
              --file "$file" \
              --watch false
          done

  deploy-production:
    needs: test
    if: github.ref == 'refs/heads/main'
    runs-on: ubuntu-latest
    environment: production

    steps:
      - uses: actions/checkout@v3

      - name: Install Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable

      - name: Build deployer
        run: cargo build --release --bin deployer

      - name: Deploy to production
        run: ./deploy-production.sh
        env:
          PROD_SERVER: ${{ secrets.PROD_SERVER }}
```

### GitLab CI/CD

Create `.gitlab-ci.yml`:

```yaml
stages:
  - test
  - deploy

variables:
  CARGO_HOME: ${CI_PROJECT_DIR}/.cargo

cache:
  paths:
    - .cargo/
    - target/
    - node_modules/

test:
  stage: test
  image: node:18
  script:
    - npm ci
    - npm run lint
    - npm test

deploy_staging:
  stage: deploy
  image: rust:latest
  only:
    - develop
  script:
    - cargo build --release --bin deployer
    - ./deploy-staging.sh
  environment:
    name: staging
    url: $STAGING_URL

deploy_production:
  stage: deploy
  image: rust:latest
  only:
    - main
  when: manual
  script:
    - cargo build --release --bin deployer
    - ./deploy-production.sh
  environment:
    name: production
    url: $PROD_URL
```

### Jenkins Pipeline

Create `Jenkinsfile`:

```groovy
pipeline {
    agent any

    stages {
        stage('Test') {
            steps {
                sh 'npm ci'
                sh 'npm run lint'
                sh 'npm test'
            }
        }

        stage('Build') {
            steps {
                sh 'cargo build --release --bin deployer'
            }
        }

        stage('Deploy to Staging') {
            when {
                branch 'develop'
            }
            steps {
                sh './deploy-staging.sh'
            }
        }

        stage('Deploy to Production') {
            when {
                branch 'main'
            }
            steps {
                input message: 'Deploy to production?', ok: 'Deploy'
                sh './deploy-production.sh'
            }
        }
    }

    post {
        success {
            echo 'Deployment successful!'
        }
        failure {
            echo 'Deployment failed!'
        }
    }
}
```

## Testing Tools

### Unit Testing with Jest

Install Jest:

```bash
npm install --save-dev jest
```

Create `tests/api/users.test.js`:

```javascript
// Mock aiwebengine functions
global.console = {
  log: jest.fn(),
  error: jest.fn(),
  warn: jest.fn(),
  info: jest.fn(),
  debug: jest.fn(),
};
global.register = jest.fn();

// Import your script
const userScript = require("../../scripts/api/users.js");

describe("User API", () => {
  test("returns users list", () => {
    const request = {
      method: "GET",
      url: "/api/users",
    };

    const response = userScript.getUsers(request);

    expect(response.status).toBe(200);
    expect(response.headers["Content-Type"]).toBe("application/json");

    const body = JSON.parse(response.body);
    expect(Array.isArray(body)).toBe(true);
  });

  test("creates new user", () => {
    const request = {
      method: "POST",
      url: "/api/users",
      body: JSON.stringify({
        name: "John Doe",
        email: "john@example.com",
      }),
    };

    const response = userScript.createUser(request);

    expect(response.status).toBe(201);

    const body = JSON.parse(response.body);
    expect(body.id).toBeDefined();
    expect(body.name).toBe("John Doe");
  });
});
```

Add to `package.json`:

```json
{
  "scripts": {
    "test": "jest",
    "test:watch": "jest --watch",
    "test:coverage": "jest --coverage"
  }
}
```

Run tests:

```bash
npm test
```

### Integration Testing

Create `tests/integration/api.test.js`:

```javascript
const fetch = require("node-fetch");

const BASE_URL = "http://localhost:8080";

describe("API Integration Tests", () => {
  test("GET /api/users returns users", async () => {
    const response = await fetch(`${BASE_URL}/api/users`);
    const data = await response.json();

    expect(response.status).toBe(200);
    expect(Array.isArray(data)).toBe(true);
  });

  test("POST /api/users creates user", async () => {
    const userData = {
      name: "Jane Doe",
      email: "jane@example.com",
    };

    const response = await fetch(`${BASE_URL}/api/users`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(userData),
    });

    const data = await response.json();

    expect(response.status).toBe(201);
    expect(data.name).toBe(userData.name);
  });
});
```

## Other Editors

### Vim/Neovim

Create `.vim/ftplugin/javascript.vim`:

```vim
" Auto-deploy on save
autocmd BufWritePost *.js !deployer --uri http://localhost:8080/%:t:r --file % --watch false

" Quick deployment command
command! Deploy !deployer --uri http://localhost:8080/%:t:r --file % --watch false

" Map to <leader>d
nnoremap <leader>d :Deploy<CR>
```

### Emacs

Add to `.emacs` or `init.el`:

```elisp
(defun deploy-aiwebengine ()
  "Deploy current JavaScript file to aiwebengine"
  (interactive)
  (let* ((file (buffer-file-name))
         (name (file-name-sans-extension (file-name-nondirectory file)))
         (uri (concat "http://localhost:8080/" name)))
    (shell-command (format "deployer --uri %s --file %s --watch false" uri file))))

(global-set-key (kbd "C-c d") 'deploy-aiwebengine)
```

### Sublime Text

Create build system (`Tools` → `Build System` → `New Build System`):

```json
{
  "cmd": [
    "deployer",
    "--uri",
    "http://localhost:8080/$file_base_name",
    "--file",
    "$file",
    "--watch",
    "false"
  ],
  "selector": "source.js",
  "variants": [
    {
      "name": "Deploy and Watch",
      "cmd": [
        "deployer",
        "--uri",
        "http://localhost:8080/$file_base_name",
        "--file",
        "$file"
      ]
    }
  ]
}
```

Save as `aiwebengine.sublime-build`.

**Usage:** Cmd+B (Mac) or Ctrl+B (Windows/Linux)

## Best Practices

### 1. Use Version Control

Always use Git for your aiwebengine projects:

```bash
git init
git add .
git commit -m "Initial commit"
```

### 2. Separate Environments

Maintain separate configuration for each environment:

```
config/
├── local.js
├── staging.js
└── production.js
```

### 3. Automate Deployments

Don't deploy manually. Use scripts:

```bash
# deploy-staging.sh
#!/bin/bash
./target/release/deployer \
  --server "$STAGING_URL" \
  --uri "$STAGING_URL/api/users" \
  --file "scripts/api/users.js" \
  --watch false
```

### 4. Test Before Deploying

Always test locally first:

```bash
npm test && ./deploy-staging.sh
```

### 5. Use Code Reviews

Require pull request reviews before merging:

```yaml
# .github/workflows/pr.yml
on:
  pull_request:
    types: [opened, synchronize]
```

### 6. Document Your Workflows

Create `DEVELOPMENT.md`:

```markdown
# Development Workflow

## Setup

1. Clone repo
2. Run `npm install`
3. Start server: `cargo run`

## Development

1. Create feature branch
2. Edit scripts
3. Test locally
4. Create PR

## Deployment

- Staging: Automatic on merge to develop
- Production: Manual approval required
```

## Next Steps

- **[Deployer Tool](deployer.md)** - Command-line deployment
- **[Web Editor](editor.md)** - Browser-based development
- **[Deployment Workflow](../getting-started/03-deployment-workflow.md)** - Compare all methods
- **[Examples](../examples/)** - Real-world integration examples

## Quick Reference

### VS Code

```bash
# Deploy current file
Cmd+Shift+B (Mac) / Ctrl+Shift+B (Windows/Linux)

# Run task
Cmd+Shift+P → "Tasks: Run Task"
```

### Git

```bash
# Create feature branch
git checkout -b feature/my-feature

# Commit changes
git add .
git commit -m "feat: add new feature"

# Push and create PR
git push origin feature/my-feature
```

### CI/CD

```yaml
# GitHub Actions - deploy on push
on:
  push:
    branches: [main]
```

External tools integration makes aiwebengine development professional and scalable! 🛠️
