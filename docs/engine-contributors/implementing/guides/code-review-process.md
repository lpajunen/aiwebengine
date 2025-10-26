# Code Review Process

**Last Updated:** October 24, 2025

How code reviews work and what reviewers look for in aiwebengine.

---

## Overview

All code contributions undergo thorough review before merging. This guide explains the review process, what reviewers look for, and how to get your code merged efficiently.

**Audience:**

- Contributors submitting PRs
- Reviewers conducting reviews
- Maintainers managing the process

**Goals:**

- Maintain code quality
- Share knowledge
- Catch bugs early
- Ensure consistency

---

## The Review Process

### 1. Submission

**Before Submitting:**

```bash
# Run local checks
cargo test
cargo clippy
cargo fmt --check

# Run security scan
cargo audit

# Check documentation builds
cargo doc --no-deps
```

**PR Requirements:**

- [ ] All tests pass
- [ ] Code formatted with `cargo fmt`
- [ ] No clippy warnings
- [ ] Documentation updated
- [ ] CHANGELOG.md updated
- [ ] Security checklist completed

### 2. Automated Checks

**CI Pipeline Runs:**

```yaml
# .github/workflows/ci.yml (conceptual)
- Build check
- Unit tests
- Integration tests
- Clippy linting
- Format check
- Security audit
- Documentation build
```

**Must Pass Before Human Review:**

- All CI checks green
- No merge conflicts
- Branch up to date with main

### 3. Human Review

**Review Timeline:**

- **Simple PRs**: 1-2 days
- **Complex PRs**: 3-5 days
- **Large refactors**: 1-2 weeks

**Reviewers Assigned:**

- Automatic assignment via CODEOWNERS
- Additional reviewers can be requested
- At least one maintainer approval required

### 4. Feedback & Iteration

**Responding to Feedback:**

1. Read all comments carefully
2. Ask questions if unclear
3. Make requested changes
4. Respond to each comment
5. Request re-review

**Making Changes:**

```bash
# Make changes based on feedback
vim src/module.rs

# Commit with descriptive message
git add .
git commit -m "Address review feedback: improve error handling"

# Push to update PR
git push origin feature-branch
```

### 5. Approval & Merge

**Approval Requirements:**

- [ ] At least 1 maintainer approval
- [ ] All conversations resolved
- [ ] All CI checks passing
- [ ] No merge conflicts

**Merge Strategy:**

- **Squash merge** for most PRs
- **Rebase merge** for multi-commit features
- **Merge commit** for major releases

---

## What Reviewers Look For

### Code Quality

#### 1. Correctness

**Does the code work as intended?**

```rust
// ✅ GOOD: Correct logic
pub fn calculate_timeout(base: u64, multiplier: u64) -> Duration {
    Duration::from_secs(base.saturating_mul(multiplier))
}

// ❌ BAD: Can overflow
pub fn calculate_timeout(base: u64, multiplier: u64) -> Duration {
    Duration::from_secs(base * multiplier) // Can panic!
}
```

**Reviewer Checks:**

- [ ] Logic is correct
- [ ] Edge cases handled
- [ ] Error conditions covered
- [ ] No off-by-one errors

#### 2. Safety

**Is the code safe?**

```rust
// ✅ GOOD: Proper error handling
pub fn read_config(path: &Path) -> Result<Config, ConfigError> {
    let contents = fs::read_to_string(path)
        .map_err(|e| ConfigError::ReadFailed(path.to_path_buf(), e))?;

    toml::from_str(&contents)
        .map_err(|e| ConfigError::ParseFailed(path.to_path_buf(), e))
}

// ❌ BAD: Multiple unwraps
pub fn read_config(path: &Path) -> Config {
    let contents = fs::read_to_string(path).unwrap();
    toml::from_str(&contents).unwrap()
}
```

**Reviewer Checks:**

- [ ] No unwrap() or expect() (without justification)
- [ ] No panic!() in library code
- [ ] Proper Result/Option handling
- [ ] No unsafe code (or justified and documented)

#### 3. Performance

**Is the code efficient?**

```rust
// ✅ GOOD: Efficient iteration
pub fn find_scripts(filter: &str) -> Vec<Script> {
    scripts
        .iter()
        .filter(|s| s.name.contains(filter))
        .cloned()
        .collect()
}

// ❌ BAD: Unnecessary allocation
pub fn find_scripts(filter: &str) -> Vec<Script> {
    let mut results = Vec::new();
    for script in &scripts {
        if script.name.contains(filter) {
            results.push(script.clone());
        }
    }
    results
}
```

**Reviewer Checks:**

- [ ] No unnecessary allocations
- [ ] No N+1 query patterns
- [ ] Appropriate data structures used
- [ ] No expensive operations in loops

#### 4. Clarity

**Is the code easy to understand?**

```rust
// ✅ GOOD: Clear intent
pub fn validate_email(email: &str) -> Result<(), ValidationError> {
    if email.is_empty() {
        return Err(ValidationError::Empty);
    }

    if !email.contains('@') {
        return Err(ValidationError::MissingAtSign);
    }

    let parts: Vec<&str> = email.split('@').collect();
    if parts.len() != 2 {
        return Err(ValidationError::InvalidFormat);
    }

    Ok(())
}

// ❌ BAD: Unclear logic
pub fn validate_email(email: &str) -> Result<(), ValidationError> {
    if email.is_empty() || !email.contains('@')
        || email.split('@').collect::<Vec<_>>().len() != 2 {
        return Err(ValidationError::Invalid);
    }
    Ok(())
}
```

**Reviewer Checks:**

- [ ] Clear variable names
- [ ] Logical function structure
- [ ] Appropriate comments
- [ ] Complex logic explained

### Architecture & Design

#### 1. Separation of Concerns

**Is responsibility properly divided?**

```rust
// ✅ GOOD: Separated concerns
pub struct ScriptService {
    repository: Arc<ScriptRepository>,
    validator: Arc<ScriptValidator>,
}

impl ScriptService {
    pub async fn register(&self, script: NewScript) -> Result<Script, ServiceError> {
        self.validator.validate(&script)?;
        self.repository.save(script).await
    }
}

// ❌ BAD: Mixed concerns
pub fn register_script(db: &Database, script: NewScript) -> Result<Script, Error> {
    // Validation mixed with persistence
    if script.name.is_empty() { /* ... */ }
    db.execute("INSERT INTO scripts...").unwrap()
}
```

**Reviewer Checks:**

- [ ] Single responsibility principle
- [ ] Clear module boundaries
- [ ] Appropriate abstractions
- [ ] No God objects

#### 2. API Design

**Is the public API well-designed?**

```rust
// ✅ GOOD: Builder pattern for complex objects
pub struct ScriptRegistrationBuilder {
    path: Option<String>,
    content: Option<String>,
    capabilities: Vec<Capability>,
}

impl ScriptRegistrationBuilder {
    pub fn new() -> Self { /* ... */ }
    pub fn path(mut self, path: String) -> Self { /* ... */ }
    pub fn content(mut self, content: String) -> Self { /* ... */ }
    pub fn capability(mut self, cap: Capability) -> Self { /* ... */ }
    pub fn build(self) -> Result<ScriptRegistration, BuildError> { /* ... */ }
}

// ❌ BAD: Too many parameters
pub fn register_script(
    path: String,
    content: String,
    cap1: bool,
    cap2: bool,
    cap3: bool,
    cap4: bool,
    metadata: Option<HashMap<String, String>>,
) -> Result<(), Error> {
    // ...
}
```

**Reviewer Checks:**

- [ ] Intuitive function signatures
- [ ] Consistent naming conventions
- [ ] Appropriate use of builders/configs
- [ ] Backward compatibility maintained

#### 3. Error Handling

**Are errors handled appropriately?**

```rust
// ✅ GOOD: Typed errors with context
#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("Script not found: {0}")]
    NotFound(String),

    #[error("Invalid script path: {path}")]
    InvalidPath { path: String },

    #[error("Compilation failed: {0}")]
    CompilationFailed(String),

    #[error("Repository error: {0}")]
    Repository(#[from] RepositoryError),
}

// ❌ BAD: Generic errors
pub enum ScriptError {
    Error(String),
}
```

**Reviewer Checks:**

- [ ] Specific error types
- [ ] Helpful error messages
- [ ] Errors include context
- [ ] Error conversions implemented

### Testing

#### 1. Test Coverage

**Are tests comprehensive?**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_email() {
        assert!(validate_email("user@example.com").is_ok());
    }

    #[test]
    fn test_empty_email() {
        let result = validate_email("");
        assert!(matches!(result, Err(ValidationError::Empty)));
    }

    #[test]
    fn test_missing_at_sign() {
        let result = validate_email("userexample.com");
        assert!(matches!(result, Err(ValidationError::MissingAtSign)));
    }

    #[test]
    fn test_multiple_at_signs() {
        let result = validate_email("user@@example.com");
        assert!(matches!(result, Err(ValidationError::InvalidFormat)));
    }
}
```

**Reviewer Checks:**

- [ ] Happy path tested
- [ ] Error cases tested
- [ ] Edge cases covered
- [ ] Coverage >90% for new code

#### 2. Test Quality

**Are tests well-written?**

```rust
// ✅ GOOD: Focused, clear test
#[tokio::test]
async fn test_script_registration_requires_authentication() {
    let service = create_test_service().await;
    let script = NewScript::builder()
        .path("/test.js")
        .content("console.log('test')")
        .build();

    let result = service.register(script, None).await;

    assert!(matches!(result, Err(ServiceError::Unauthenticated)));
}

// ❌ BAD: Testing multiple things
#[test]
fn test_script_stuff() {
    let service = create_service();
    assert!(service.register(script1).is_ok());
    assert!(service.delete(script1.id).is_ok());
    assert!(service.register(script2).is_err());
    assert!(service.list().len() == 1);
}
```

**Reviewer Checks:**

- [ ] One assertion per test (generally)
- [ ] Clear test names
- [ ] Proper setup/teardown
- [ ] No flaky tests

### Security

**See [security-checklist.md](security-checklist.md) for complete details**

**Key Security Checks:**

- [ ] Input validation present
- [ ] No SQL injection vulnerabilities
- [ ] Authentication enforced
- [ ] Authorization checked
- [ ] Secrets not hardcoded
- [ ] Error messages don't leak info
- [ ] Rate limiting on user endpoints

### Documentation

#### 1. Code Documentation

**Are public APIs documented?**

````rust
/// Registers a new script in the system.
///
/// # Arguments
///
/// * `registration` - The script registration details
/// * `user` - The authenticated user performing the registration
///
/// # Returns
///
/// Returns `Ok(Script)` with the registered script on success,
/// or `Err(ServiceError)` if:
/// - User lacks `WriteScripts` capability
/// - Script path is invalid
/// - Script content is malformed
///
/// # Example
///
/// ```no_run
/// # use aiwebengine::{ScriptService, NewScript, UserContext};
/// # async fn example(service: ScriptService, user: UserContext) {
/// let script = NewScript::builder()
///     .path("/example.js")
///     .content("console.log('Hello')")
///     .build()?;
///
/// let registered = service.register(script, Some(user)).await?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// ```
pub async fn register(
    &self,
    registration: NewScript,
    user: Option<UserContext>,
) -> Result<Script, ServiceError> {
    // ...
}
````

**Reviewer Checks:**

- [ ] Public functions documented
- [ ] Public types documented
- [ ] Module-level docs present
- [ ] Examples provided

#### 2. External Documentation

**Is user-facing documentation updated?**

**Reviewer Checks:**

- [ ] README updated if needed
- [ ] CHANGELOG updated
- [ ] Migration guide for breaking changes
- [ ] API documentation current

---

## Common Feedback Patterns

### Code Structure

**"Consider extracting this to a separate function"**

```rust
// Before
pub fn process_request(req: Request) -> Response {
    // 50 lines of validation logic
    // 30 lines of processing logic
    // 20 lines of response building
}

// After
pub fn process_request(req: Request) -> Response {
    let validated = validate_request(req)?;
    let result = process_validated_request(validated)?;
    build_response(result)
}
```

**"This could be more idiomatic"**

```rust
// Before
let mut result = Vec::new();
for item in items {
    if item.is_valid() {
        result.push(item.value);
    }
}

// After
let result: Vec<_> = items
    .iter()
    .filter(|item| item.is_valid())
    .map(|item| item.value)
    .collect();
```

### Error Handling

**"Don't use unwrap() here"**

```rust
// Before
let value = map.get(&key).unwrap();

// After
let value = map.get(&key)
    .ok_or(Error::KeyNotFound(key.clone()))?;
```

**"Add context to this error"**

```rust
// Before
let file = fs::read_to_string(path)?;

// After
let file = fs::read_to_string(path)
    .map_err(|e| Error::ConfigReadFailed {
        path: path.to_path_buf(),
        source: e,
    })?;
```

### Testing

**"Add a test for the error case"**

```rust
#[test]
fn test_parse_config_invalid_format() {
    let invalid_config = "not valid toml {{{";
    let result = parse_config(invalid_config);
    assert!(matches!(result, Err(ConfigError::ParseFailed(_))));
}
```

**"Use a more specific assertion"**

```rust
// Before
assert!(result.is_ok());

// After
assert_eq!(result.unwrap().status, Status::Success);
```

### Documentation

**"Add a doc comment explaining why"**

```rust
// Before
const MAX_RETRIES: u32 = 3;

// After
/// Maximum retry attempts for failed script executions.
///
/// Limited to 3 to prevent infinite loops and excessive
/// resource consumption from repeatedly failing scripts.
const MAX_RETRIES: u32 = 3;
```

---

## How to Respond to Feedback

### Good Responses

**✅ Acknowledge and implement:**

> "Good catch! I'll add error handling for that case."

**✅ Ask for clarification:**

> "I'm not sure I understand. Could you elaborate on what you mean by 'more idiomatic'?"

**✅ Explain reasoning (if needed):**

> "I used unwrap() here because X is guaranteed to exist by Y. I'll add a comment explaining this."

**✅ Suggest alternatives:**

> "I could do that, but would Z approach work better? It would simplify the logic."

### Avoid

**❌ Defensive responses:**

> "This code works fine. I don't see the problem."

**❌ Ignoring feedback:**

> (No response to comment)

**❌ Over-explaining trivial changes:**

> "Sure, I'll rename that variable." (Just do it)

---

## Becoming a Better Reviewer

### Review Etiquette

**DO:**

- Be respectful and constructive
- Explain the "why" behind feedback
- Acknowledge good work
- Offer to pair program on complex issues
- Frame as suggestions, not demands

**DON'T:**

- Be dismissive or condescending
- Give vague feedback ("this is wrong")
- Focus only on negatives
- Nitpick excessively on style
- Block on personal preferences

### Effective Feedback

**✅ GOOD:**

> "This function could panic if the vector is empty. Consider using `first()` which returns an Option, or adding a check before accessing `[0]`."

**❌ BAD:**

> "This will crash."

**✅ GOOD:**

> "Nice use of the builder pattern here! One suggestion: we could add `#[must_use]` to the builder methods to catch accidental non-use."

**❌ BAD:**

> "Missing #[must_use]"

### Review Checklist

- [ ] Code works correctly
- [ ] Tests are comprehensive
- [ ] Documentation is clear
- [ ] Security checklist followed
- [ ] Performance is acceptable
- [ ] Code is maintainable
- [ ] Follows project conventions
- [ ] No obvious bugs

---

## Approval Guidelines

### When to Approve

- [ ] All feedback addressed or discussed
- [ ] All required checks passing
- [ ] Code meets quality standards
- [ ] Documentation is complete
- [ ] Tests are adequate

### When to Request Changes

- [ ] Security vulnerabilities present
- [ ] Tests failing or insufficient
- [ ] Breaking changes without migration
- [ ] Doesn't follow architecture guidelines
- [ ] Major bugs or correctness issues

### When to Comment (Not Block)

- [ ] Minor style suggestions
- [ ] Performance micro-optimizations
- [ ] Alternative approaches to consider
- [ ] Questions for understanding
- [ ] Appreciation for good work

---

## Special Cases

### Large PRs

**Strategy:**

1. Review architecture/design first
2. Then review implementation
3. Consider breaking into smaller PRs

**Example Comment:**

> "This is a lot to review at once. Could we split the refactoring into one PR and the new feature into another?"

### Breaking Changes

**Requirements:**

- [ ] Documented in PR description
- [ ] Migration guide provided
- [ ] Deprecation warnings added (if applicable)
- [ ] CHANGELOG clearly notes breaking change

### Urgent Fixes

**Expedited Review:**

- Security vulnerabilities
- Production outages
- Critical bugs

**Still Required:**

- Tests
- Security review
- At least one approval

---

## Tools & Resources

### Review Tools

```bash
# Local review commands
cargo test
cargo clippy
cargo fmt --check
cargo audit

# Check specific changes
git diff main...feature-branch

# Review history
git log main..feature-branch
```

### References

- [CONTRIBUTING.md](../CONTRIBUTING.md) - Full contribution guide
- [security-checklist.md](security-checklist.md) - Security review
- [testing-guidelines.md](testing-guidelines.md) - Testing standards
- [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)

---

## Getting Help

**If your PR is stalled:**

1. Respond to all comments
2. Ping reviewers after updates
3. Ask in discussions if blocked
4. Request additional reviewers if needed

**If you disagree with feedback:**

1. Explain your reasoning respectfully
2. Ask for clarification
3. Suggest alternatives
4. Escalate to maintainers if needed

---

_Code review is a collaboration, not a critique. We're all working toward better software._
