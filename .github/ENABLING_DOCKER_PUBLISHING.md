# Enabling Docker Image Publishing

## Current Status

🟡 **Docker publishing is DISABLED during development**

The GitHub Actions workflow currently:
- ✅ Builds Docker images on every push
- ✅ Validates Dockerfile and build process
- ✅ Uses layer caching for faster builds
- ❌ Does NOT push images to container registry
- ❌ Does NOT run security scans (requires published images)

## When to Enable Publishing

Enable Docker image publishing when:
- ✅ Version 1.0.0 is ready for release
- ✅ All security requirements are met
- ✅ Documentation is complete
- ✅ Production deployment is tested
- ✅ You're ready for public distribution

## How to Enable Publishing (v1.0.0+)

### Step 1: Update Workflow File

Edit `.github/workflows/docker.yml`:

#### 1.1 Update workflow name (line 1)
```yaml
# Change from:
name: Docker Build and Test

# To:
name: Docker Build and Push
```

#### 1.2 Enable version tag triggers (line 14-16)
```yaml
# Uncomment:
tags:
  - 'v*'
```

#### 1.3 Enable registry login (line 32-39)
```yaml
# Uncomment the entire "Log in to Container Registry" step
- name: Log in to Container Registry
  if: github.event_name != 'pull_request'
  uses: docker/login-action@v3
  with:
    registry: ${{ env.REGISTRY }}
    username: ${{ github.actor }}
    password: ${{ secrets.GITHUB_TOKEN }}
```

#### 1.4 Enable image push (line 57)
```yaml
# Change from:
push: false

# To:
push: ${{ github.event_name != 'pull_request' }}
```

#### 1.5 Remove load option (line 64)
```yaml
# Remove this line:
load: true
```

#### 1.6 Enable security scan (lines 76-101)
```yaml
# Uncomment the entire "security-scan" job
security-scan:
  needs: build-and-test
  # ... rest of the job
```

### Step 2: Create Version Tag

```bash
# Create and push version tag
git tag -a v1.0.0 -m "Release version 1.0.0"
git push origin v1.0.0
```

### Step 3: Verify Publishing

After pushing the tag:

1. **Check GitHub Actions**: 
   - Go to: https://github.com/lpajunen/aiwebengine/actions
   - Verify workflow runs successfully

2. **Check Container Registry**:
   - Go to: https://github.com/lpajunen/aiwebengine/pkgs/container/aiwebengine
   - Verify image is published

3. **Test Pulling Image**:
   ```bash
   docker pull ghcr.io/lpajunen/aiwebengine:v1.0.0
   docker pull ghcr.io/lpajunen/aiwebengine:1.0.0
   docker pull ghcr.io/lpajunen/aiwebengine:1.0
   docker pull ghcr.io/lpajunen/aiwebengine:1
   docker pull ghcr.io/lpajunen/aiwebengine:latest
   ```

### Step 4: Update Documentation

Update these files to reflect published images:

1. **README.md**: Add Docker Hub/GHCR badge and pull command
2. **docs/DOCKER.md**: Update with actual image names
3. **DOCKER_QUICK_REFERENCE.md**: Add pull commands

Example README section:
```markdown
## Installation

### Using Docker (Recommended)

```bash
docker pull ghcr.io/lpajunen/aiwebengine:latest
docker run -p 3000:3000 ghcr.io/lpajunen/aiwebengine:latest
```
```

## Image Tagging Strategy

Once enabled, images will be tagged as:

### On version tags (v1.2.3)
- `ghcr.io/lpajunen/aiwebengine:v1.2.3`
- `ghcr.io/lpajunen/aiwebengine:1.2.3`
- `ghcr.io/lpajunen/aiwebengine:1.2`
- `ghcr.io/lpajunen/aiwebengine:1`
- `ghcr.io/lpajunen/aiwebengine:latest`

### On branch pushes
- `ghcr.io/lpajunen/aiwebengine:main`
- `ghcr.io/lpajunen/aiwebengine:develop`

### On commits
- `ghcr.io/lpajunen/aiwebengine:sha-<commit-hash>`

## Image Visibility

By default, GitHub Container Registry images are:
- **Public** for public repositories
- **Free** for public images
- Anyone can pull without authentication

### To make images private:

Add to workflow after the build step:
```yaml
- name: Make image private
  run: |
    echo "${{ secrets.GITHUB_TOKEN }}" | docker login ghcr.io -u ${{ github.actor }} --password-stdin
    # Set package visibility to private
```

Then manually set visibility in GitHub:
1. Go to package settings
2. Change visibility to private

## Security Scanning

Once publishing is enabled, Trivy will:
- ✅ Scan for CVEs in base images
- ✅ Scan for vulnerabilities in dependencies
- ✅ Report to GitHub Security tab
- ✅ Fail builds on critical vulnerabilities (optional)

### To make security scan required:

Change in workflow:
```yaml
security-scan:
  runs-on: ubuntu-latest
  # Add this to fail on high/critical
  continue-on-error: false
```

## Testing Before v1.0.0 Release

To test publishing without making it permanent:

1. Create a test tag:
   ```bash
   git tag v0.9.0-beta
   git push origin v0.9.0-beta
   ```

2. Enable publishing temporarily in a feature branch

3. Verify everything works

4. Delete the test tag and image:
   ```bash
   git tag -d v0.9.0-beta
   git push --delete origin v0.9.0-beta
   ```

5. Delete image from GHCR manually

## Troubleshooting

### Build succeeds but push fails

**Problem**: Permissions error
**Solution**: Ensure workflow has `packages: write` permission (already configured)

### Image not visible in registry

**Problem**: Image is built but not showing up
**Solution**: 
- Check workflow logs
- Verify `push: true` is set
- Ensure login step succeeded

### Security scan fails

**Problem**: Trivy finds vulnerabilities
**Solution**:
- Review security report in GitHub Security tab
- Update base image in Dockerfile
- Update dependencies in Cargo.toml
- Consider using `-alpine` or `distroless` base images

## Checklist for Enabling

Before enabling publishing:

- [ ] All tests passing
- [ ] Security audit completed
- [ ] Documentation updated
- [ ] Version 1.0.0 tag created
- [ ] Changelog prepared
- [ ] Release notes written
- [ ] Production deployment tested
- [ ] Backup/rollback plan ready
- [ ] Monitoring configured
- [ ] Workflow file updated per steps above
- [ ] Test tag pushed successfully
- [ ] Test image pulled successfully
- [ ] All checks passed

## Quick Enable Command Summary

```bash
# 1. Edit workflow file
nano .github/workflows/docker.yml

# 2. Make changes listed above

# 3. Commit changes
git add .github/workflows/docker.yml
git commit -m "Enable Docker image publishing for v1.0.0"

# 4. Create and push version tag
git tag -a v1.0.0 -m "Release version 1.0.0"
git push origin main
git push origin v1.0.0

# 5. Verify
# Check: https://github.com/lpajunen/aiwebengine/actions
# Check: https://github.com/lpajunen/aiwebengine/pkgs/container/aiwebengine
```

## Support

If you encounter issues enabling publishing:
- Check GitHub Actions logs
- Review workflow syntax
- Check GitHub token permissions
- Refer to Docker build-push-action docs
