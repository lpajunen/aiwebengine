# aiwebengine

## Overview

**aiwebengine** is a lightweight web application engine built in Rust that enables developers to create dynamic web content using JavaScript scripts. The project leverages the QuickJS JavaScript runtime to provide a simple yet powerful platform for building web applications with minimal overhead.

## Key Features

- **JavaScript-Powered Content Creation**: Write web application logic using familiar JavaScript syntax
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

For comprehensive development guidance, including detailed API documentation, examples, and best practices, please refer to our [Developer Guide](docs/APP_DEVELOPMENT.md).

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

See [docs/local-development.md](docs/local-development.md) for more details.

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

Please see `TODO.md` for detailed information about planned features and development priorities.

## License

This project is licensed under the terms specified in the LICENSE file.
