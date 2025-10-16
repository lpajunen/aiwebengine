# Docker Publishing Status

**Current Status**: 🟡 DISABLED (Development Mode)

## What's Happening Now

When you push to GitHub:

- ✅ Docker images are **BUILT** and **TESTED** in CI
- ❌ Docker images are **NOT PUSHED** to registry
- ❌ Images are **NOT PUBLICLY AVAILABLE**

This is intentional during development to avoid publishing incomplete versions.

## How to Enable Publishing

When you're ready to release v1.0.0, follow the guide:

📖 **[Enabling Docker Publishing Guide](.github/ENABLING_DOCKER_PUBLISHING.md)**

Quick summary:

1. Edit `.github/workflows/docker.yml`
2. Uncomment publishing sections
3. Create v1.0.0 tag
4. Push and verify

## Current Workflow

The GitHub Actions workflow (`.github/workflows/docker.yml`):

- Triggers on pushes to `main` and `develop` branches
- Builds Docker image to verify Dockerfile works
- Uses layer caching for faster builds
- Does NOT authenticate to registry
- Does NOT push images anywhere

## Local Docker Usage

You can still use Docker locally:

```bash
# Build locally
make docker-build

# Run locally
make docker-prod

# Development with hot-reload
make docker-dev
```

See **[docs/DOCKER.md](docs/DOCKER.md)** for complete Docker documentation.

---

**Note**: This status will be updated to ✅ ENABLED after v1.0.0 release.
