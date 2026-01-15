# Contributing to aiwebengine Implementation

Welcome! This guide helps you contribute features and improvements to the aiwebengine core codebase.

**Last Updated:** October 24, 2025

---

## 🎯 Before You Start

### 1. Check the Roadmap

Before implementing anything, check [ROADMAP.md](./docs/engine-contributors/implementing/ROADMAP.md) to:

- See if your feature/improvement is already planned
- Understand priority and timeline
- Check if prerequisites are met
- Avoid duplicate work

### 2. Understand Current Focus

As of October 2025, we're focused on:

- **🔴 Critical:** Error handling, security integration, testing
- **🟠 High:** Authentication system (after critical items complete)
- **⏸️ Paused:** All other features until v1.0 foundation is solid

**If your contribution isn't in the current focus area**, discuss it first via GitHub Discussions or Issues.

### 3. Read Development Guidelines

Familiarize yourself with:

- [DEVELOPMENT.md](./docs/engine-contributors/implementing/DEVELOPMENT.md) - Coding standards and best practices
- [guides/adding-new-features.md](./docs/engine-contributors/implementing/guides/adding-new-features.md) - Feature implementation process
- [guides/testing-guidelines.md](./docs/engine-contributors/implementing/guides/testing-guidelines.md) - Testing requirements

---

## 📝 Contribution Process

### Step 1: Discuss First (For Larger Changes)

For significant features or improvements:

1. **Check existing issues/discussions** - Someone may already be working on it
2. **Open a GitHub Discussion** - Describe what you want to build and why
3. **Get feedback** - Maintainers will help refine the approach
4. **Wait for approval** - Ensures alignment with roadmap and architecture

**Skip this step for:** Bug fixes, typos, documentation improvements, small refinements

### Step 2: Create an Issue

Create a GitHub Issue describing:

- **What** you're implementing (feature/improvement name)
- **Why** it's needed (problem being solved)
- **How** you plan to implement it (high-level approach)
- **Testing** strategy
- **Documentation** updates needed

Use issue templates when available.

### Step 3: Claim the Work

Comment on the issue with:

- "I'd like to work on this"
- Your estimated timeline
- Any questions or clarifications needed

A maintainer will assign the issue to you.

### Step 4: Implementation

Follow this workflow:

#### 4.1 Create a Feature Branch

```bash
git checkout main
git pull origin main
git checkout -b feature/your-feature-name

# Naming conventions:
# feature/authentication-oauth
# fix/memory-leak-js-engine
# refactor/error-handling
# docs/api-documentation
# improvement/testing-coverage
```

#### 4.2 Follow Implementation Guidelines

**For Features** (new capabilities):

1. Read the feature guide (e.g., `features/authentication.md`)
2. Create module structure in `src/`
3. Implement core functionality with proper error handling
4. Add comprehensive tests (>90% coverage for new code)
5. Update configuration if needed
6. Add JavaScript APIs if user-facing
7. Create example scripts
8. Write documentation

**For Improvements** (enhancing existing code):

1. Read the improvement guide (e.g., `improvements/error-handling.md`)
2. Identify all affected files
3. Make changes incrementally with tests
4. Ensure no regressions (all existing tests pass)
5. Add new tests for improved behavior
6. Update documentation

#### 4.3 Follow Coding Standards

**Must-follow rules:**

✅ **DO:**

- Use `Result<T, E>` for all fallible operations
- Add comprehensive error handling (zero `unwrap()` calls)
- Write descriptive function and variable names
- Add doc comments for public APIs
- Include examples in documentation
- Keep functions small and focused
- Use strong typing (avoid `String` where better types exist)

❌ **DON'T:**

- Use `unwrap()` or `expect()` in production code
- Add TODO comments without GitHub issues
- Skip tests ("I'll add them later")
- Ignore compiler warnings
- Leave commented-out code
- Commit secrets or credentials

**See [DEVELOPMENT.md](./docs/engine-contributors/implementing/DEVELOPMENT.md) for complete coding standards.**

#### 4.4 Write Tests

**Testing is mandatory.** Every contribution must include:

**Unit Tests:**

- Test each function independently
- Cover happy path and error cases
- Test boundary conditions
- Aim for >90% coverage of new code

**Integration Tests:**

- Test interactions between modules
- Validate end-to-end workflows
- Test with realistic data

**Example:**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feature_success_case() {
        let result = your_function(valid_input);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), expected_output);
    }

    #[test]
    fn test_feature_error_case() {
        let result = your_function(invalid_input);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ExpectedError));
    }

    #[test]
    fn test_feature_boundary_condition() {
        let result = your_function(edge_case_input);
        assert!(result.is_ok());
    }
}
```

**See [guides/testing-guidelines.md](./docs/engine-contributors/implementing/guides/testing-guidelines.md) for details.**

#### 4.5 Update Documentation

**Required documentation updates:**

- [ ] **Code documentation** - Doc comments on public functions/types
- [ ] **Feature guide** - Update relevant file in `features/` or `improvements/`
- [ ] **ROADMAP.md** - Mark tasks complete, update status
- [ ] **Administrator documentation** - If deployment-related, update `docs/engine-administrators/`
- [ ] **CHANGELOG.md** - Add entry describing your change
- [ ] **Example scripts** - If adding JavaScript APIs, create examples in `scripts/example_scripts/`

#### 4.6 Run Quality Checks

Before committing:

```bash
# Format code
cargo fmt --all

# Check for issues
cargo clippy --all-targets -- -D warnings

# Run tests
cargo test --all-features

# Generate coverage report
cargo llvm-cov --all-features --html

# Check documentation builds
cargo doc --no-deps --open
```

**All checks must pass.** Fix any warnings or errors.

### Step 5: Submit Pull Request

#### 5.1 Commit Your Changes

Use conventional commit messages:

```bash
git add .
git commit -m "feat: add OAuth2 authentication support

- Implement Google, Microsoft, and Apple OAuth providers
- Add session management with encrypted storage
- Create authentication middleware
- Include comprehensive tests (95% coverage)

Closes #123"
```

**Commit message format:**

```
<type>: <subject>

<body>

<footer>
```

**Types:**

- `feat:` - New feature
- `fix:` - Bug fix
- `refactor:` - Code restructuring without behavior change
- `test:` - Adding or updating tests
- `docs:` - Documentation changes
- `perf:` - Performance improvements
- `chore:` - Maintenance tasks

#### 5.2 Push and Create PR

```bash
git push origin feature/your-feature-name
```

Go to GitHub and create a Pull Request with:

**Title:** Clear, concise description

```
Add OAuth2 authentication support with 3 providers
```

**Description:** Use this template:

```markdown
## Description

Implements OAuth2 authentication with Google, Microsoft, and Apple providers as outlined in #123.

## Changes Made

- Added `src/auth/` module with OAuth provider implementations
- Implemented secure session management with AES-256 encryption
- Created authentication middleware with rate limiting
- Added JavaScript APIs for user context access
- Included 45 new tests with 95% coverage

## Testing

- [x] Unit tests pass (45 new tests)
- [x] Integration tests pass
- [x] Manual testing in dev environment
- [x] Security review completed
- [x] Performance benchmarks met

## Documentation

- [x] Updated features/authentication.md
- [x] Updated ROADMAP.md
- [x] Created example scripts in scripts/example_scripts/
- [x] Updated CHANGELOG.md

## Breaking Changes

None - authentication is opt-in via configuration

## Screenshots (if applicable)

[Include screenshots for UI changes]

## Checklist

- [x] Code follows DEVELOPMENT.md guidelines
- [x] Zero unwrap() calls in production code
- [x] All tests pass (126/126)
- [x] Coverage >90% for new code
- [x] No compiler warnings
- [x] Documentation updated
- [x] CHANGELOG.md updated
- [x] Security checklist completed

Closes #123
```

### Step 6: Code Review

Maintainers will review your PR and may request changes.

**What reviewers check:**

- ✅ Alignment with roadmap and architecture
- ✅ Code quality and standards compliance
- ✅ Comprehensive testing
- ✅ Security considerations
- ✅ Performance impact
- ✅ Documentation completeness
- ✅ Breaking changes properly documented

**How to handle feedback:**

1. **Be responsive** - Reply within 48 hours
2. **Ask questions** - If feedback is unclear, ask
3. **Make changes** - Address all feedback
4. **Push updates** - Commits to your branch update the PR
5. **Re-request review** - After addressing feedback

### Step 7: Merge

Once approved:

- Maintainers will merge your PR
- Your contribution will be part of the next release
- You'll be credited in CHANGELOG.md and release notes

---

## 🚫 What We Don't Accept

To maintain quality, we won't merge:

❌ **Incomplete implementations**

- Missing tests
- Missing documentation
- Half-implemented features

❌ **Code that doesn't meet standards**

- Compiler warnings
- Clippy warnings
- Use of `unwrap()` in production code
- Poor error handling

❌ **Security risks**

- Unauthenticated access to sensitive operations
- Unvalidated user input
- Hardcoded secrets
- Known vulnerabilities

❌ **Breaking changes without discussion**

- Changing public APIs without RFC
- Removing features without migration path
- Major architectural changes without approval

❌ **Out-of-scope work**

- Features not on roadmap without prior approval
- Low-priority items when critical work is pending
- Personal preferences that don't align with project goals

---

## 🎯 Quality Gates

Your contribution must meet these gates before merging:

### Gate 1: Code Quality

- [ ] Zero `unwrap()` or `expect()` in production code
- [ ] Zero compiler warnings
- [ ] Zero Clippy warnings (with `-- -D warnings`)
- [ ] Code follows Rust idioms and best practices
- [ ] Consistent naming and style

### Gate 2: Testing

- [ ] All existing tests pass (100% pass rate)
- [ ] New tests added for new functionality
- [ ] Coverage >90% for new code
- [ ] Integration tests for cross-module features
- [ ] Error cases tested

### Gate 3: Security

- [ ] Input validation implemented
- [ ] No SQL injection vulnerabilities (when DB added)
- [ ] No XSS vulnerabilities in outputs
- [ ] Authentication/authorization checked
- [ ] Security checklist completed (see guides/security-checklist.md)

### Gate 4: Performance

- [ ] No performance regressions
- [ ] Algorithms use appropriate data structures
- [ ] No memory leaks
- [ ] Resource limits enforced

### Gate 5: Documentation

- [ ] Public APIs documented with examples
- [ ] Feature/improvement guide updated
- [ ] ROADMAP.md updated
- [ ] CHANGELOG.md updated
- [ ] Example scripts provided (if applicable)

---

## 💡 Tips for Successful Contributions

### Start Small

If you're new to the project:

1. Fix a bug or improve documentation
2. Add tests to increase coverage
3. Refactor code for clarity
4. Then tackle larger features

Small contributions build trust and familiarity.

### Communicate Early and Often

- Discuss approach before coding
- Ask questions when stuck
- Share progress updates on the issue
- Respond promptly to code review feedback

### Follow the Roadmap

Work on prioritized items:

- 🔴 Critical items have highest impact
- 🟠 High items needed for v1.0
- 🟡 Medium items improve quality
- 🟢 Low items are future enhancements

### Write Tests First

Consider TDD (Test-Driven Development):

1. Write failing test
2. Implement feature to make test pass
3. Refactor while keeping tests green

This ensures good test coverage and clear requirements.

### Keep PRs Focused

One PR = One logical change

- ✅ "Add OAuth2 authentication"
- ❌ "Add auth, fix bugs, refactor config, update docs"

Small, focused PRs are easier to review and merge.

---

## 🔍 Review Process

### What to Expect

**Timeline:**

- Initial review: Within 3-5 business days
- Follow-up reviews: Within 2 business days
- Merge: After approval from 2 maintainers

**Reviewer Assignments:**

- Architecture changes: Lead maintainer
- Security changes: Security reviewer
- Core features: Two maintainers
- Documentation: Any maintainer

### Common Review Feedback

**Code Quality:**

- "Please remove this unwrap() and handle the error properly"
- "This function is too complex; consider splitting it"
- "Add doc comments explaining what this does"

**Testing:**

- "Please add tests for the error cases"
- "Coverage for this module is only 60%; please add more tests"
- "The integration test is missing"

**Security:**

- "This input needs validation before use"
- "Please add rate limiting to this endpoint"
- "Sensitive data should be encrypted"

**Documentation:**

- "Please update the user guide with this new feature"
- "The example is missing from the documentation"
- "Add an entry to CHANGELOG.md"

---

## 📞 Getting Help

### Where to Ask Questions

- **General questions:** GitHub Discussions
- **Implementation help:** Comment on your issue/PR
- **Security issues:** Email lpajunen@gmail.com privately

### Resources

- [DEVELOPMENT.md](./docs/engine-contributors/implementing/DEVELOPMENT.md) - Development guidelines
- [ROADMAP.md](./docs/engine-contributors/implementing/ROADMAP.md) - What needs to be done
- [guides/](./docs/engine-contributors/implementing/guides/) - Implementation guides
- [features/](./docs/engine-contributors/implementing/features/) - Feature specifications
- [improvements/](./docs/engine-contributors/implementing/improvements/) - Improvement guides

---

## 🙏 Thank You

Every contribution makes aiwebengine better. Whether you:

- Fix a typo
- Add a test
- Implement a feature
- Improve documentation
- Review a PR

**You're making a difference. Thank you!**

---

_For questions about this contribution process, open a GitHub Discussion._
