# JWT Token Implementation for Load Balancer Support

## Overview

This document outlines the approach for implementing JWT (JSON Web Token) based authentication to replace the current in-memory session storage, enabling true horizontal scaling behind a load balancer.

## Current State (In-Memory Sessions)

**Location**: `src/security/session.rs`

**Problem**:

- Sessions stored in `Arc<RwLock<HashMap<String, EncryptedSessionData>>>`
- User authenticated on Server 1 won't be recognized on Server 2
- Users must re-authenticate when load balancer switches servers

**Current Flow**:

```
1. User authenticates → Server creates session in memory
2. Server returns session token (random string) in cookie
3. Subsequent requests → Server looks up token in HashMap
4. Problem: Token only exists in that server's memory
```

## JWT Approach (Stateless Sessions)

### How JWT Works

**JWT Structure**: `header.payload.signature`

1. **Header**: Token type and signing algorithm
2. **Payload**: User data (user_id, email, expires_at, etc.)
3. **Signature**: HMAC signature to prevent tampering

**Example JWT Payload**:

```json
{
  "user_id": "user_12345",
  "provider": "google",
  "email": "user@example.com",
  "name": "John Doe",
  "is_admin": false,
  "iat": 1698765432, // issued at
  "exp": 1698851832, // expires at
  "fingerprint": "hash_of_ip_and_useragent"
}
```

### JWT Benefits

✅ **Load Balancer Compatible**: Token is self-contained, any server can verify it  
✅ **No Shared State**: No Redis or database needed for sessions  
✅ **Scalable**: Add/remove servers without session migration  
✅ **Standard**: Well-established industry practice  
✅ **Libraries Available**: `jsonwebtoken` crate already in dependencies

### JWT Trade-offs

⚠️ **Cannot Invalidate**: Once issued, JWT is valid until expiration  
⚠️ **Larger Cookies**: JWT contains data, not just random token  
⚠️ **No Server-Side Logout**: Logout requires blacklist or short expiration times  
⚠️ **Exposure Risk**: Token contains user data (use HTTPS always)

## Implementation Plan

### Phase 1: JWT Token Generation

**File**: `src/auth/jwt.rs` (new file)

```rust
use jsonwebtoken::{encode, decode, Header, Validation, EncodingKey, DecodingKey};
use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc, Duration};

#[derive(Debug, Serialize, Deserialize)]
pub struct JwtClaims {
    pub sub: String,              // subject (user_id)
    pub provider: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub is_admin: bool,
    pub iat: i64,                 // issued at
    pub exp: i64,                 // expires at
    pub fingerprint: String,      // session fingerprint hash
}

pub struct JwtManager {
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    token_lifetime: Duration,
}

impl JwtManager {
    pub fn new(secret: &[u8; 32], lifetime_seconds: i64) -> Self {
        Self {
            encoding_key: EncodingKey::from_secret(secret),
            decoding_key: DecodingKey::from_secret(secret),
            token_lifetime: Duration::seconds(lifetime_seconds),
        }
    }

    pub fn create_token(
        &self,
        user_id: String,
        provider: String,
        email: Option<String>,
        name: Option<String>,
        is_admin: bool,
        fingerprint: String,
    ) -> Result<String, JwtError> {
        let now = Utc::now();
        let claims = JwtClaims {
            sub: user_id,
            provider,
            email,
            name,
            is_admin,
            iat: now.timestamp(),
            exp: (now + self.token_lifetime).timestamp(),
            fingerprint,
        };

        encode(&Header::default(), &claims, &self.encoding_key)
            .map_err(|e| JwtError::EncodingFailed(e.to_string()))
    }

    pub fn verify_token(&self, token: &str) -> Result<JwtClaims, JwtError> {
        let validation = Validation::default();
        let token_data = decode::<JwtClaims>(token, &self.decoding_key, &validation)
            .map_err(|e| JwtError::ValidationFailed(e.to_string()))?;

        Ok(token_data.claims)
    }
}
```

### Phase 2: Update AuthSessionManager

**File**: `src/auth/session.rs`

Add JWT mode alongside existing session manager:

```rust
pub enum SessionBackend {
    InMemory(Arc<SecureSessionManager>),
    Jwt(Arc<JwtManager>),
}

pub struct AuthSessionManager {
    backend: SessionBackend,
}

impl AuthSessionManager {
    pub fn new_with_jwt(jwt_manager: Arc<JwtManager>) -> Self {
        Self {
            backend: SessionBackend::Jwt(jwt_manager),
        }
    }

    pub async fn create_session(&self, ...) -> Result<SessionToken, AuthError> {
        match &self.backend {
            SessionBackend::InMemory(manager) => {
                // Current implementation
            }
            SessionBackend::Jwt(jwt) => {
                let fingerprint = create_fingerprint(ip_addr, user_agent);
                let token = jwt.create_token(
                    user_id, provider, email, name, is_admin, fingerprint
                )?;
                Ok(SessionToken { token, expires_at })
            }
        }
    }

    pub async fn get_session(&self, token: &str, ...) -> Result<AuthSession, AuthError> {
        match &self.backend {
            SessionBackend::InMemory(manager) => {
                // Current implementation
            }
            SessionBackend::Jwt(jwt) => {
                let claims = jwt.verify_token(token)?;

                // Validate fingerprint
                let current_fingerprint = create_fingerprint(ip_addr, user_agent);
                if claims.fingerprint != current_fingerprint {
                    return Err(AuthError::SessionFingerprintMismatch);
                }

                Ok(AuthSession::from(claims))
            }
        }
    }
}
```

### Phase 3: Configuration

**File**: `config.toml`

```toml
[auth]
# Session backend: "memory" or "jwt"
session_backend = "jwt"

# JWT secret (32 bytes, base64 encoded)
# Generate with: openssl rand -base64 32
jwt_secret = "your-secret-key-here"

# Session lifetime in seconds (default: 7 days)
session_timeout = 604800

# Enable strict IP validation (false for mobile users)
strict_ip_validation = false
```

### Phase 4: Handling Logout

Since JWT cannot be invalidated server-side, we have options:

**Option A: Short-Lived Tokens + Refresh Tokens** (Recommended)

- Access token: 15 minutes lifetime (JWT)
- Refresh token: 7 days lifetime (stored in database)
- Logout invalidates refresh token only
- User stays logged in for up to 15 minutes after logout

**Option B: Token Blacklist**

- Maintain a blacklist of revoked tokens in Redis
- Check blacklist on every request
- Defeats the purpose of stateless JWT but provides immediate logout

**Option C: Accept Trade-off**

- Logout clears cookie client-side
- Token remains valid for its lifetime
- Use short token lifetimes (e.g., 1 hour)
- Acceptable for many applications

## Migration Strategy

### Step 1: Add JWT Support (Non-Breaking)

```rust
// Both backends supported
pub enum SessionBackend {
    InMemory(Arc<SecureSessionManager>),
    Jwt(Arc<JwtManager>),
}
```

### Step 2: Test in Development

```toml
# config.local.toml
session_backend = "jwt"
```

### Step 3: Deploy to Production

```toml
# config.production.toml
session_backend = "jwt"
jwt_secret = "production-secret-from-env"
```

### Step 4: Remove In-Memory Backend (Optional)

After JWT is proven stable, remove in-memory code to simplify.

## Security Considerations

### 1. JWT Secret Management

- Use environment variables for production
- Rotate secrets periodically
- Use different secrets per environment

### 2. Token Expiration

- Balance security vs user experience
- Shorter = more secure, more re-authentications
- Longer = better UX, higher risk if stolen

### 3. Fingerprint Validation

- Bind token to IP + User-Agent hash
- Prevents token theft/replay attacks
- May need to disable for mobile users (IP changes)

### 4. HTTPS Required

- JWT contains user data
- Always use HTTPS in production
- Set `Secure` flag on cookies

### 5. Token Size

- JWT is larger than random session ID
- Cookie limit is 4KB (not a concern for typical JWT)
- Consider HttpOnly + SameSite cookie flags

## Testing

```bash
# Test JWT creation and validation
cargo test --lib jwt

# Test session management with JWT backend
cargo test --lib auth::session

# Integration test with load balancer
# 1. Start two servers on different ports
# 2. Configure Caddy to load balance
# 3. Login on server 1
# 4. Make request to server 2
# 5. Verify authentication persists
```

## Estimated Implementation Time

- **Phase 1** (JWT Manager): 2-3 hours
- **Phase 2** (Integration): 3-4 hours
- **Phase 3** (Configuration): 1 hour
- **Phase 4** (Logout Strategy): 2-3 hours
- **Testing**: 2-3 hours

**Total**: ~10-15 hours

## Next Steps

1. ✅ Implement stateless CSRF (DONE)
2. ⏸️ Study JWT approach (this document)
3. ⏭️ Decide on logout strategy
4. ⏭️ Implement JWT manager
5. ⏭️ Test with load balancer
6. ⏭️ Deploy to production

## References

- JWT RFC: https://datatracker.ietf.org/doc/html/rfc7519
- jsonwebtoken crate: https://docs.rs/jsonwebtoken/
- OWASP JWT Cheat Sheet: https://cheatsheetseries.owasp.org/cheatsheets/JSON_Web_Token_for_Java_Cheat_Sheet.html
