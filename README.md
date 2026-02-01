# aiwebengine

[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg)](https://www.gnu.org/licenses/agpl-3.0)
[![Rust Edition](https://img.shields.io/badge/rust-2024-orange.svg)](https://www.rust-lang.org)
[![Version](https://img.shields.io/badge/version-0.1.0-green.svg)](https://github.com/lpajunen/aiwebengine/releases)
[![Language](https://img.shields.io/badge/language-Rust%20%7C%20JavaScript-red.svg)](https://github.com/lpajunen/aiwebengine)

AI and Web Engine for JavaScript / TypeScript based Solutions - Secure and Scalable Platform for Building Websites, APIs, Web Apps, and AI Tools and agents

Excels when developing solutions using generative AI technologies.

Supports JavaScript / TypeScript as the primary programming language. Also supports JSX / TSX for building user interfaces.

Code first approach. Then provide tools for verification and testing.
Verify code by providing automatic API descriptions such as OpenAPI, GraphQL schema, and MCP tool list.
Test code by providing automatic test case generation and execution environment.

API access: script-internal, engine-internal, and external. Script-internal APIs are available only to the script itself. Engine-internal APIs are available to all scripts running in the same engine instance. External APIs are available to outside world.

External API access: public, authenticated, role based. There are engine provided roles such as editor and adinistrator. Scripts can provide additional roles for authenticated users. When API endpoint required authentication, there can be a handler that checks user roles before proceeding.

Scripts can be privileged or restricted. Privileged scripts have access to all engine-internal APIs. Restricted scripts have access only to selected engine-internal APIs.

Users with editor and administrator roles can force all script APIs to be external for debugging and testing purposes. This done only per script basis.

Engine supports user authentication and authorization. Engine supports user role and permission management. Engine supports script management and deployment. Engine supports logging and monitoring. (In the future, it could be possible to separate user authentication and authorization to a separate service if some cloud environment is used and they provide such capabilities. In that case, the engine would focus on script management and deployment, logging and monitoring, and API access control.)

Solution development support is separate to own repositories. There are tools for creating, testing, and deploying solutions. In addition, there are documentation and examples for solution developers.

## Overview

**aiwebengine** (AI Web Engine) is an open-source project designed to facilitate the development of web-based solutions using JavaScript by providing a secure sandbox for executing untrusted code. It is an application engine for software written in the AI era. The engine implements core protocols and features needed for building websites, GraphQL APIs, web applications, and AI tools with minimal overhead. The solution developers can focus on writing JavaScript scripts to implement their business logic, while the engine handles the underlying infrastructure and common functionalities.

In addition to being a web application engine, aiwebengine provides an editorial environment for creating, testing, and deploying JavaScript and related web resource based solutions.

AI Web Engine consists of the following main components:

- **Engine Core Runtime**: The core of the engine, implemented in Rust, which provides the main functionality for handling HTTP requests, managing scripts, and executing JavaScript code securely.
- **JavaScript Runtime**: An embedded QuickJS JavaScript engine that allows the execution of JavaScript code within the Rust application.
- **Server Script and Asset Repository**: A module for managing and storing JavaScript scripts and related web assets, allowing dynamic loading and updating of scripts without restarting the engine.
- **Logging System**: A built-in logging mechanism for monitoring and debugging purposes.
- **Editorial Environment**: A web-based interface for solution developers to create, test, and deploy their JavaScript-based solutions.

**aiwebengine** is a lightweight web application engine built in Rust that enables developers to create secure solutions using JavaScript scripts. The project leverages the QuickJS JavaScript runtime to provide a simple yet powerful platform for building websites, GraphQL APIs, web applications, and AI tools with minimal overhead.

## User Roles

Understanding the different roles in the aiwebengine ecosystem:

| Role                        | Description                                                                    | Primary Activities                                                                         |
| --------------------------- | ------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------ |
| **End Users**               | People using websites, applications, and AI tools built on aiwebengine         | Interact with solutions through web browsers or APIs                                       |
| **Solution Developers**     | Developers building solutions (websites, web apps, AI tools) using aiwebengine | Write and edit JavaScript scripts, design application logic, create user interfaces        |
| **Solution Administrators** | People deploying and managing individual solutions built on aiwebengine        | Configure solution settings, monitor performance, manage deployments                       |
| **Engine Administrators**   | People deploying and managing aiwebengine instances                            | Install and configure aiwebengine, manage infrastructure, ensure security and availability |
| **Engine Contributors**     | Developers contributing to the aiwebengine core project                        | Implement features, fix bugs, improve performance, enhance documentation                   |

**Note**: The same person may fulfill multiple roles. For example, a Solution Developer might also be an Engine Administrator for their deployment.

### What are "Solutions"?

In the context of aiwebengine, a **solution** refers to any website, GraphQL API, web application, or AI tool built using the engine. Solutions are created by writing JavaScript scripts that handle HTTP requests, process data, and generate responses. Examples include:

- Public-facing websites and blogs
- RESTful and GraphQL APIs
- AI-powered tools and services
- Custom web applications with dynamic content

## Key Features

- **JavaScript-Powered Solutions**: Build complete solutions using familiar JavaScript syntax
- **Lightweight Architecture**: Built with Rust for high performance and low resource consumption
- **Embedded JavaScript Runtime**: Utilizes QuickJS for efficient server-side JavaScript execution
- **RESTful API Support**: Handle HTTP requests and responses with JavaScript handlers
- **Built-in Logging**: Integrated logging system for debugging and monitoring
- **Script Management**: Dynamic loading and management of JavaScript scripts

## Project Status

⚠️ **Work in Progress**: This project is currently in active development. Core functionality is implemented and functional, but additional features and enhancements are planned for future releases.

### Current Capabilities

- Basic HTTP request handling (GET, POST)
- JavaScript script execution and registration
- Query parameter and form data parsing
- Response generation with custom status codes and content types
- In-memory logging system
- Script repository management
- Authentication and security middleware

### Roadmap

The project roadmap includes planned enhancements such as:

- Database integration
- Testing framework integration for user generated scripts
- Public cloud deployment guides like Terraform scripts
- System monitoring and alerting

## Getting Started

### Prerequisites

- Rust (latest stable version recommended)
- Basic understanding of JavaScript

### Documentation

Comprehensive documentation is available for all user roles:

- **📚 [Documentation Index](docs/INDEX.md)** - Complete guide to all documentation
- **🔧 Engine Administrators** - [Getting Started](docs/engine-administrators/01-GETTING-STARTED.md) | [Configuration](docs/engine-administrators/02-CONFIGURATION.md) | [Running Environments](docs/engine-administrators/03-RUNNING-ENVIRONMENTS.md) | [Quick Reference](docs/engine-administrators/QUICK-REFERENCE.md)
- **🛠️ Engine Contributors** - [Requirements](docs/engine-contributors/planning/REQUIREMENTS.md) | [Development Roadmap](docs/engine-contributors/implementing/TODO.md)

**Engine Administrators**: New task-based documentation guides you from setup to production deployment. Start with [Getting Started](docs/engine-administrators/01-GETTING-STARTED.md) or jump to the [Quick Reference](docs/engine-administrators/QUICK-REFERENCE.md) for command lookups.

For quick reference, see the role-based organization in the [Documentation Index](docs/INDEX.md).

### Installation

```bash
# Clone the repository
git clone https://github.com/lpajunen/aiwebengine.git
cd aiwebengine

# Set up configuration
cp config.local.toml config.toml
cp .env.example .env
# Edit .env with your OAuth credentials and secrets

# Build the project
cargo build --release

# Run the server
source .env && cargo run
```

### Docker Deployment

The easiest way to get started is with Docker:

```bash
# Quick start with Docker Compose
make docker-setup
make docker-prod

# Or manually
cp .env.example .env
# Edit .env with your configuration
docker-compose up -d
```

For detailed Docker deployment instructions, see [docs/engine-administrators/03-RUNNING-ENVIRONMENTS.md](docs/engine-administrators/03-RUNNING-ENVIRONMENTS.md).

### Development

For local development, you have multiple options depending on your needs:

#### Option 1: Cargo Run (`http://localhost:3000`)

Simplest option for quick development without Docker:

```bash
# Set up local configuration
cp config.local.toml config.toml
cp .env.example .env
# Edit .env with your development credentials

# Install development tools
make deps

# Run development server with localhost OAuth
make dev-local
# Or manually: source .env && APP_AUTH__PROVIDERS__GOOGLE__REDIRECT_URI=http://localhost:3000/auth/callback/google cargo run
```

#### Option 2: Docker with Localhost (`https://localhost`)

Works offline, no DNS setup required:

```bash
# Set up environment
cp .env.example .env
# Edit .env with your credentials (DIGITALOCEAN_TOKEN not required)

# Run with Docker Compose
make docker-localhost
# Access at: https://localhost
# Note: Accept self-signed certificate warning in browser
```

#### Option 3: Docker with DNS Domain (`https://local.softagen.com`)

Uses Let's Encrypt for real SSL certificate (requires DNS token):

```bash
# Set up environment
cp .env.example .env
# Edit .env and add: DIGITALOCEAN_TOKEN=your_token_here

# Check DNS configuration
make check-dns

# Run with Docker Compose
make docker-dns
# Access at: https://local.softagen.com
```

#### Option 4: /etc/hosts Method (DNS name without external DNS)

Use DNS names locally without Let's Encrypt:

```bash
# Add to /etc/hosts
sudo sh -c 'echo "127.0.0.1 local.softagen.com" >> /etc/hosts'

# Run without DNS token (uses self-signed cert)
make docker-localhost
# Access at: https://local.softagen.com
# Note: Accept self-signed certificate warning in browser
```

**Important for Google OAuth**: Add redirect URIs to your Google Cloud Console based on which option you use:

- Option 1: `http://localhost:3000/auth/callback/google`
- Option 2: `https://localhost/auth/callback/google`
- Option 3/4: `https://local.softagen.com/auth/callback/google`

See [docs/engine-administrators/02-CONFIGURATION.md](docs/engine-administrators/02-CONFIGURATION.md) for detailed configuration options.

## Architecture

The engine consists of several key components:

- **Server Layer**: Built with Axum web framework for HTTP handling
- **JavaScript Runtime**: QuickJS integration for script execution
- **Script Repository**: In-memory storage and management of JavaScript code
- **Request Processing**: Automatic parsing of HTTP requests and routing to appropriate handlers

## Contributing

This project welcomes contributions! As it's in active development, there are many opportunities to:

- Implement new features from the roadmap
- Improve documentation and examples
- Add comprehensive tests
- Enhance performance and security

Please see [docs/engine-contributors/implementing/TODO.md](docs/engine-contributors/implementing/TODO.md) for detailed information about planned features and development priorities.

## License

This project is licensed under the terms specified in the LICENSE file.
