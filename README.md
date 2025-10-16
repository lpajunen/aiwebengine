# aiwebengine

## Overview

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

### Roadmap

The project roadmap includes planned enhancements such as:

- Authentication and security middleware
- Database integration
- Testing framework integration

## Getting Started

### Prerequisites

- Rust (latest stable version recommended)
- Basic understanding of JavaScript

### Documentation

Comprehensive documentation is available for all user roles:

- **📚 [Documentation Index](docs/INDEX.md)** - Complete guide to all documentation
- **👥 Solution Developers** - [Getting Started Guide](docs/solution-developers/APP_DEVELOPMENT.md)
- **🔧 Engine Administrators** - [Docker Deployment](docs/engine-administrators/DOCKER.md) | [Configuration](docs/engine-administrators/CONFIGURATION.md)
- **🛠️ Engine Contributors** - [Requirements](docs/engine-contributors/planning/REQUIREMENTS.md) | [Development Roadmap](docs/engine-contributors/implementing/TODO.md)

For quick reference, see the role-based organization in the [Documentation Index](docs/INDEX.md).

### Installation

```bash
# Clone the repository
git clone https://github.com/lpajunen/aiwebengine.git
cd aiwebengine

# Build the project
cargo build --release

# Run the server
cargo run
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

For detailed Docker deployment instructions, see [docs/DOCKER.md](docs/DOCKER.md).

### Development

For local development:

```bash
# Install development tools
make deps

# Run development server with hot-reload
make dev

# Or use Docker for development
make docker-dev
```

See [docs/engine-administrators/local-development.md](docs/engine-administrators/local-development.md) for more details.

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
