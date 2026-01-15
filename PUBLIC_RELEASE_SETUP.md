# Repository Public Release - Metadata Setup

This file contains instructions for setting up GitHub repository metadata after making the repository public.

## 1. Repository Description

Go to: **Repository Settings → General → About**

**Description:**

```
AI and Web Engine for JavaScript - Secure sandbox for building websites, APIs, web apps, and AI tools
```

**Website:** _(optional)_

```
https://github.com/lpajunen/aiwebengine
```

## 2. Repository Topics/Tags

Go to: **Repository Settings → General → About → Topics**

Add these topics:

```
rust
javascript
typescript
webengine
ai
graphql
quickjs
sandbox
api
web-framework
server
runtime
```

## 3. Make Repository Public

Go to: **Repository Settings → General → Danger Zone → Change repository visibility**

1. Click "Change visibility"
2. Select "Make public"
3. Type repository name to confirm: `aiwebengine`
4. Click "I understand, change repository visibility"

## 4. Post-Release Setup (Optional but Recommended)

### Enable GitHub Discussions

Go to: **Repository Settings → General → Features**

- [x] Enable Discussions

### Enable GitHub Issues

- [x] Enable Issues (should be enabled by default)

### Enable GitHub Wiki (optional)

- [ ] Enable Wiki (optional - we keep docs in repository)

### Set up GitHub Pages (optional)

- [ ] Pages (optional - keeping docs in repository for now)

### Configure Branch Protection (recommended)

Go to: **Repository Settings → Branches → Add rule**

**Branch name pattern:** `main`

Settings:

- [x] Require a pull request before merging
  - [x] Require approvals (1)
- [x] Require status checks to pass before merging
- [x] Require conversation resolution before merging
- [ ] Do not allow bypassing the above settings (optional)

## 5. After Making Public

### Announce the release:

1. Create a GitHub Discussion in "Announcements"
2. Share on social media (optional)
3. Submit to relevant communities (optional)

### Monitor:

1. Watch for new issues and discussions
2. Respond to community questions
3. Review incoming pull requests

### Maintain:

1. Keep dependencies updated
2. Address security issues promptly
3. Respond to community contributions

---

**Note:** This file is for internal use during the public release process. You can delete it after completing these steps.
