//! The engine as an OAuth 2.1 authorization server: authorize, consent and token.

use super::*;
use crate::engine_page::Width;
use crate::security::client_ip;
use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};

/// Authorization code data stored temporarily
#[derive(Debug, Clone, sqlx::FromRow)]
pub(super) struct AuthorizationCodeData {
    pub(super) user_id: String,
    #[allow(dead_code)] // Stored for future validation
    pub(super) client_id: String,
    pub(super) redirect_uri: String,
    pub(super) code_challenge: Option<String>,
    pub(super) code_challenge_method: Option<String>,
    pub(super) scope: Option<String>,
    pub(super) resource: Option<String>,
    pub(super) expires_at: DateTime<Utc>,
    pub(super) used: bool,
}

/// Paths the OAuth 2.0 protocol endpoints are served at.
///
/// These are used to mount the routes, to build the return URL an
/// unauthenticated caller comes back to after logging in, and to advertise the
/// endpoints in the discovery documents, so those cannot drift apart — the
/// return URL did drift once, when the generic `/authorize` alias was
/// withdrawn in favour of the reserved `/auth` prefix.
pub(crate) const AUTHORIZE_PATH: &str = "/auth/oauth2/authorize";
pub(crate) const TOKEN_PATH: &str = "/auth/oauth2/token";
pub(crate) const REGISTRATION_PATH: &str = "/auth/oauth2/register";
/// Where the consent page posts its answer. Not advertised in the discovery
/// metadata: it is part of how this server renders the authorization endpoint,
/// not an endpoint a client ever calls.
pub(crate) const CONSENT_PATH: &str = "/auth/oauth2/consent";

/// Cap on a consent form's size. The body is a handful of short fields the
/// engine wrote into its own page, so anything larger is not a consent form.
pub(super) const MAX_CONSENT_BODY_BYTES: usize = 16 * 1024;

/// OAuth 2.0 authorization request parameters (RFC 6749)
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AuthorizeParams {
    /// Client identifier
    pub(super) response_type: String,

    /// Client identifier
    pub(super) client_id: String,

    /// Redirection URI
    #[serde(default)]
    pub(super) redirect_uri: Option<String>,

    /// Requested scope
    #[serde(default)]
    pub(super) scope: Option<String>,

    /// Opaque value for CSRF protection
    #[serde(default)]
    pub(super) state: Option<String>,

    /// PKCE code challenge (RFC 7636)
    #[serde(default)]
    pub(super) code_challenge: Option<String>,

    /// PKCE code challenge method (S256 or plain)
    #[serde(default)]
    pub(super) code_challenge_method: Option<String>,

    /// Resource indicator (RFC 8707)
    #[serde(default)]
    pub(super) resource: Option<String>,
}

/// Rebuild the authorization request as a relative URL, for a caller who has
/// to log in first and should land back on the request they made.
///
/// The parameters are re-encoded from the parsed request rather than the raw
/// query string being passed through, so nothing the caller appended survives
/// into the URL the login flow will bounce them to.
pub(super) fn authorize_return_url(params: &AuthorizeParams) -> String {
    let mut query_params = vec![
        format!(
            "response_type={}",
            urlencoding::encode(&params.response_type)
        ),
        format!("client_id={}", urlencoding::encode(&params.client_id)),
    ];

    let optional = [
        ("redirect_uri", &params.redirect_uri),
        ("scope", &params.scope),
        ("state", &params.state),
        ("code_challenge", &params.code_challenge),
        ("code_challenge_method", &params.code_challenge_method),
        ("resource", &params.resource),
    ];
    for (name, value) in optional {
        if let Some(value) = value
            && !value.is_empty()
        {
            query_params.push(format!("{}={}", name, urlencoding::encode(value)));
        }
    }

    format!("{}?{}", AUTHORIZE_PATH, query_params.join("&"))
}

/// Shortest and longest a PKCE code challenge may be (RFC 7636 §4.2). An S256
/// challenge is base64url of a SHA-256 digest, so it is always 43 characters;
/// the range is what the spec allows, not what we expect.
pub(super) const MIN_CODE_CHALLENGE_LENGTH: usize = 43;
pub(super) const MAX_CODE_CHALLENGE_LENGTH: usize = 128;

/// Whether a code challenge is shaped like one, before it is stored.
///
/// A malformed challenge would fail verification at the token endpoint anyway,
/// but failing here means the client learns it at the point it made the
/// mistake, rather than after a person has been asked to approve something.
pub(super) fn code_challenge_is_wellformed(challenge: &str) -> bool {
    (MIN_CODE_CHALLENGE_LENGTH..=MAX_CODE_CHALLENGE_LENGTH).contains(&challenge.len())
        && challenge
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~'))
}

/// Whether this engine is willing to mint a token whose audience is `resource`.
///
/// RFC 8707 lets a client name what it wants a token to be good for, and the
/// token endpoint copies that name onto the session's audience verbatim. Two
/// rules, both needed:
///
/// - the resource must name a host this engine actually serves, or the audience
///   is just a string somebody typed; and
/// - it must name *the host the authorization is happening on*. A sign-in
///   completed on a solution's host must not hand out a credential for the
///   management host's `/mcp`, and realm scoping only stops that for accounts
///   whose realm is not `*`.
pub(super) fn resource_is_acceptable(resource: &str, request_host: Option<&str>) -> bool {
    resource_is_acceptable_for(resource, request_host, crate::hosts::config())
}

/// The rule itself, against an explicit host configuration so it can be
/// exercised without the process-global one.
pub(super) fn resource_is_acceptable_for(
    resource: &str,
    request_host: Option<&str>,
    hosts: &crate::hosts::HostConfig,
) -> bool {
    let normalized = crate::security::session::normalize_resource(resource);
    let authority = normalized.split('/').next().unwrap_or_default();
    if authority.is_empty() {
        return false;
    }

    // Before startup configures hosts there is nothing to check against, and a
    // deployment with no base URL set would otherwise be unable to issue any
    // token at all.
    if hosts.all_hosts().is_empty() {
        return true;
    }

    hosts.is_configured(authority) && hosts.canonical_host(request_host) == authority
}

/// The audience to mint for a `resource` a client asked for.
///
/// Reduced to the form audiences are compared in, and pinned to an endpoint. A
/// client discovers this engine through its protected-resource document and
/// asks for what that document names; several ask for the origin instead —
/// `https://example.com/`, the whole site. An audience is matched on host *and*
/// path, so a token carrying an origin authorizes nothing at all: it would be
/// minted, handed over, and refused at the only endpoint it exists for.
///
/// Naming the MCP endpoint instead narrows the token rather than widening it.
/// `/mcp` is the only path a bearer token is audience-checked on, so this is
/// the same reach an origin-wide audience would have had if one were honoured,
/// and it keeps the host binding that separates two `/mcp` endpoints of the
/// same engine.
pub(super) fn resource_audience(resource: &str) -> String {
    let normalized = crate::security::session::normalize_resource(resource);

    if normalized.contains('/') {
        normalized
    } else {
        format!(
            "{}{}",
            normalized,
            crate::auth::mcp_middleware::MCP_ENDPOINT_PATH
        )
    }
}

/// An authorization request that survived validation.
///
/// Holding the looked-up client rather than the caller's `client_id` is the
/// point: everything downstream reads the registration, so there is no path
/// where an unregistered value is used by accident.
#[derive(Debug)]
pub(super) struct ValidatedAuthorization {
    pub(super) client: crate::auth::client_registration::RegisteredClient,
    pub(super) redirect_uri: String,
    pub(super) scope: Option<String>,
    pub(super) state: Option<String>,
    pub(super) code_challenge: String,
    pub(super) resource: Option<String>,
}

/// Why an authorization request was refused, and where the answer goes.
#[derive(Debug)]
pub(super) enum AuthorizeRejection {
    /// Refused before the redirect URI could be trusted, so the answer is shown
    /// to the browser. Redirecting an error to a URI that has not been matched
    /// against a registration is the hole itself — it is how an unregistered
    /// URI gets to hear from this endpoint at all.
    Direct {
        status: StatusCode,
        error: &'static str,
        description: String,
    },
    /// Refused after the client and its redirect URI checked out, so the error
    /// goes back to the client the way RFC 6749 §4.1.2.1 asks.
    Redirect {
        redirect_uri: String,
        state: Option<String>,
        error: &'static str,
        description: String,
    },
}

impl AuthorizeRejection {
    /// Render the refusal, naming the issuer on the ones that go back to the
    /// client.
    ///
    /// RFC 9207 puts `iss` on the authorization *response*, and an error
    /// returned through the redirect URI is one — which matters more than it
    /// sounds, because the attack the parameter exists to stop works by mixing
    /// up which authorization server a response came from, and an attacker
    /// choosing between servers can choose to send an error. The direct arm
    /// gets none: nothing is being redirected, so there is nothing to confuse
    /// it with.
    pub(super) fn into_response(self, issuer: &str) -> Response {
        match self {
            AuthorizeRejection::Direct {
                status,
                error,
                description,
            } => (
                status,
                Json(ErrorResponse {
                    error: error.to_string(),
                    message: description,
                }),
            )
                .into_response(),
            AuthorizeRejection::Redirect {
                redirect_uri,
                state,
                error,
                description,
            } => {
                let mut url = append_query_param(&redirect_uri, "error", error);
                url = append_query_param(&url, "error_description", &description);
                if let Some(state) = state {
                    url = append_query_param(&url, "state", &state);
                }
                url = append_query_param(&url, "iss", issuer);
                redirect_to_client(&url)
            }
        }
    }
}

/// Append one query parameter to a URL that may or may not already have some.
pub(super) fn append_query_param(url: &str, name: &str, value: &str) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!(
        "{}{}{}={}",
        url,
        separator,
        name,
        urlencoding::encode(value)
    )
}

/// Send the browser on to a client's redirect URI.
///
/// A meta refresh plus a scripted assignment rather than a 302, because client
/// redirect URIs are routinely custom schemes (`vscode://`, `cursor://`) that
/// `Location` handling treats inconsistently. The URL is escaped for the two
/// contexts it appears in and the script runs under a per-response nonce.
pub(super) fn redirect_to_client(target: &str) -> Response {
    let js_target = serde_json::to_string(target).unwrap_or_else(|_| "\"/\"".to_string());
    let nonce = crate::security::generate_nonce();
    let head = format!(
        r#"
    <meta http-equiv="refresh" content="0;url={}">"#,
        html_attribute(target),
    );
    let body = format!(
        r#"<h1>Returning to the application</h1>
        <p class="aw-small">If nothing happens, <a href="{}">continue</a>.</p>
        <script nonce="{}">window.location.href = {};</script>"#,
        html_attribute(target),
        html_attribute(&nonce),
        js_target
    );

    crate::engine_page::response(
        StatusCode::OK,
        crate::engine_page::document_with_head("Redirecting…", &nonce, Width::Narrow, &head, &body),
        &nonce,
    )
}

/// Check an authorization request against the client registry and the rules.
///
/// This is the whole of the gate, and both the `GET` that shows a consent page
/// and the `POST` that acts on the answer run it — the second time because a
/// consent form is a caller-supplied body like any other, and re-deriving the
/// decision from it is cheaper than trusting it.
pub(super) async fn validate_authorize_request(
    pool: &PgPool,
    params: &AuthorizeParams,
    request_host: Option<&str>,
) -> Result<ValidatedAuthorization, AuthorizeRejection> {
    let direct =
        |status: StatusCode, error: &'static str, description: &str| AuthorizeRejection::Direct {
            status,
            error,
            description: description.to_string(),
        };

    if params.client_id.trim().is_empty() {
        return Err(direct(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Missing client_id parameter",
        ));
    }

    let client =
        match crate::auth::client_registration::lookup_client(pool, &params.client_id).await {
            Ok(Some(client)) => client,
            Ok(None) => {
                return Err(direct(
                    StatusCode::BAD_REQUEST,
                    "invalid_client",
                    "Unknown client_id. Register the client before requesting authorization.",
                ));
            }
            Err(e) => {
                tracing::error!("Client lookup failed: {}", e);
                return Err(direct(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "server_error",
                    "Could not read the client registry",
                ));
            }
        };

    let redirect_uri = params
        .redirect_uri
        .as_deref()
        .map(str::trim)
        .filter(|uri| !uri.is_empty());
    let Some(redirect_uri) = redirect_uri else {
        return Err(direct(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "redirect_uri is required",
        ));
    };

    if !client.redirect_uri_registered(redirect_uri) {
        return Err(direct(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "redirect_uri does not match a URI this client registered",
        ));
    }

    // Past this point the redirect URI is one the client itself registered, so
    // errors are the client's to handle and go back to it.
    let redirect = |error: &'static str, description: &str| AuthorizeRejection::Redirect {
        redirect_uri: redirect_uri.to_string(),
        state: params.state.clone(),
        error,
        description: description.to_string(),
    };

    if params.response_type != "code" {
        return Err(redirect(
            "unsupported_response_type",
            "Only the 'code' response type is supported",
        ));
    }

    if !client.allows_grant("authorization_code") {
        return Err(redirect(
            "unauthorized_client",
            "This client did not register the authorization_code grant",
        ));
    }

    // PKCE is required, not merely verified when it happens to be offered.
    // Checking a challenge only if one arrived means a caller who sends none is
    // never asked for a verifier, and an authorization code becomes usable by
    // whoever manages to read it.
    let code_challenge = params
        .code_challenge
        .as_deref()
        .map(str::trim)
        .filter(|challenge| !challenge.is_empty());
    let Some(code_challenge) = code_challenge else {
        return Err(redirect(
            "invalid_request",
            "code_challenge is required (PKCE, RFC 7636)",
        ));
    };

    // `plain` is accepted by RFC 7636 and is worth nothing here: the challenge
    // travels in the same query string as everything else, so a caller who can
    // read the request can also read the verifier.
    if params.code_challenge_method.as_deref() != Some("S256") {
        return Err(redirect(
            "invalid_request",
            "code_challenge_method must be S256",
        ));
    }

    if !code_challenge_is_wellformed(code_challenge) {
        return Err(redirect(
            "invalid_request",
            "code_challenge is not a well-formed S256 challenge",
        ));
    }

    let resource = params
        .resource
        .as_deref()
        .map(str::trim)
        .filter(|resource| !resource.is_empty());
    if let Some(resource) = resource
        && !resource_is_acceptable(resource, request_host)
    {
        return Err(redirect(
            "invalid_target",
            "resource must name this host's endpoint",
        ));
    }

    Ok(ValidatedAuthorization {
        client,
        redirect_uri: redirect_uri.to_string(),
        scope: params.scope.clone().filter(|s| !s.trim().is_empty()),
        state: params.state.clone(),
        code_challenge: code_challenge.to_string(),
        resource: resource.map(resource_audience),
    })
}

/// Whether every scope being asked for is one the stored grant already covers.
///
/// No scope requested is covered by anything, including a grant that named
/// none.
pub(super) fn scope_is_covered(granted: Option<&str>, requested: Option<&str>) -> bool {
    let Some(requested) = requested else {
        return true;
    };
    let granted = granted.unwrap_or_default();
    requested
        .split_whitespace()
        .all(|wanted| granted.split_whitespace().any(|held| held == wanted))
}

/// Whether this user has already agreed to this client doing this.
///
/// A stored grant covers a new request only when it is at least as wide.
/// Anything else — a scope that was not approved last time, a different
/// resource — is a widening, and widening is the thing that must not happen
/// without being seen.
pub(super) async fn consent_already_given(
    pool: &PgPool,
    user_id: &str,
    validated: &ValidatedAuthorization,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query(
        "SELECT scope, resource FROM oauth_client_grants WHERE user_id = $1 AND client_id = $2",
    )
    .bind(user_id)
    .bind(&validated.client.client_id)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return Ok(false);
    };

    let granted_scope: Option<String> = row.try_get("scope").unwrap_or(None);
    let granted_resource: Option<String> = row.try_get("resource").unwrap_or(None);

    Ok(
        scope_is_covered(granted_scope.as_deref(), validated.scope.as_deref())
            && granted_resource.as_deref() == validated.resource.as_deref(),
    )
}

/// The elevation a token carries, from the scope its consent named.
///
/// `None` when the engine gates nothing, because then there is nothing to
/// elevate and a token claiming an elevation would be carrying a fact about a
/// policy this engine does not have. `None` too when the scope names no
/// bundle, which is every ordinary OAuth request.
pub(super) fn token_elevation(
    scope: Option<&str>,
    max_session_age_secs: u64,
) -> Option<crate::security::elevation::Elevation> {
    use crate::security::elevation;

    if !elevation::configured().is_active() {
        return None;
    }

    let grades = elevation::grades_in_scope(scope);
    if grades.is_empty() {
        return None;
    }

    let now = Utc::now();
    Some(elevation::Elevation::for_token(
        &grades,
        now + chrono::Duration::seconds(max_session_age_secs as i64),
        now,
    ))
}

/// The same, for a refresh: the narrower of what the token holds and what the
/// person still consents to.
///
/// A consent row that cannot be read is treated as no consent. The failure
/// direction matters and there is only one safe one — a database hiccup must
/// not be a way to keep an elevation alive, and losing one costs a client a
/// re-authorization it can perform.
pub(super) async fn token_elevation_for_refresh(
    pool: &PgPool,
    user_id: &str,
    client_id: &str,
    token_scope: Option<&str>,
    max_session_age_secs: u64,
) -> Option<crate::security::elevation::Elevation> {
    use crate::security::elevation;

    if !elevation::configured().is_active() {
        return None;
    }

    let consented: Option<String> = sqlx::query_scalar(
        "SELECT scope FROM oauth_client_grants WHERE user_id = $1 AND client_id = $2",
    )
    .bind(user_id)
    .bind(client_id)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        tracing::error!(
            "Could not read the consent while refreshing for {}: {}",
            user_id,
            e
        );
        None
    })
    .flatten();

    let grades = elevation::grades_in_both(token_scope, consented.as_deref());
    if grades.is_empty() {
        return None;
    }

    let now = Utc::now();
    Some(elevation::Elevation::for_token(
        &grades,
        now + chrono::Duration::seconds(max_session_age_secs as i64),
        now,
    ))
}

/// Record what a person just approved, replacing whatever they approved before.
///
/// Stored as approved rather than merged with the previous grant: a client that
/// asks for less next time should be held to less, and a union would quietly
/// keep privileges nobody re-approved.
pub(super) async fn record_consent(
    pool: &PgPool,
    user_id: &str,
    validated: &ValidatedAuthorization,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO oauth_client_grants (user_id, client_id, scope, resource, granted_at)
         VALUES ($1, $2, $3, $4, NOW())
         ON CONFLICT (user_id, client_id)
         DO UPDATE SET scope = EXCLUDED.scope, resource = EXCLUDED.resource, granted_at = NOW()",
    )
    .bind(user_id)
    .bind(&validated.client.client_id)
    .bind(&validated.scope)
    .bind(&validated.resource)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Mint an authorization code and send the browser back to the client with it.
pub(super) async fn issue_authorization_code(
    pool: &PgPool,
    user_id: &str,
    validated: &ValidatedAuthorization,
    issuer: &str,
) -> Response {
    let auth_code = format!("code_{}", uuid::Uuid::new_v4());
    let expires_at = Utc::now() + chrono::Duration::minutes(10);

    let stored = sqlx::query(
        "INSERT INTO oauth_authorization_codes (code, user_id, client_id, redirect_uri, code_challenge, code_challenge_method, scope, resource, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"
    )
    .bind(&auth_code)
    .bind(user_id)
    .bind(&validated.client.client_id)
    .bind(&validated.redirect_uri)
    .bind(&validated.code_challenge)
    .bind("S256")
    .bind(&validated.scope)
    .bind(&validated.resource)
    .bind(expires_at)
    .execute(pool)
    .await;

    if let Err(e) = stored {
        tracing::error!("Failed to store authorization code: {}", e);
        return AuthorizeRejection::Redirect {
            redirect_uri: validated.redirect_uri.clone(),
            state: validated.state.clone(),
            error: "server_error",
            description: "Failed to record the authorization".to_string(),
        }
        .into_response(issuer);
    }

    tracing::info!(
        "Issued authorization code to client {} for user {}",
        validated.client.client_id,
        user_id
    );

    let mut target = append_query_param(&validated.redirect_uri, "code", &auth_code);
    if let Some(state) = validated.state.as_deref() {
        target = append_query_param(&target, "state", state);
    }
    // RFC 9207. A client that sent authorization requests to more than one
    // server cannot otherwise tell which of them a code came back from, and a
    // code from the wrong one redeemed at the right one is the mix-up attack
    // the parameter exists to close. The engine is both halves of this on a
    // multi-host deployment, so the issuer named is the one for the host the
    // flow actually ran on rather than the default base URL.
    target = append_query_param(&target, "iss", issuer);

    redirect_to_client(&target)
}

/// The page a person sees before a client is given a code in their name.
///
/// This is what actually stands between a cross-site navigation and an
/// authorization code. Registration is open — an attacker can register a client
/// as easily as anyone else — so validating the client and its redirect URI
/// narrows the attack without ending it. Someone saying yes, on a page that
/// names the client and shows where they will be sent, is what ends it.
///
/// The form carries the request back rather than the engine holding it: every
/// field is re-validated on the way in, so a tampered form buys nothing that
/// forging the original request would not have.
pub(super) fn render_consent_page(
    validated: &ValidatedAuthorization,
    csrf_token: &str,
    user_label: &str,
) -> Response {
    let nonce = crate::security::generate_nonce();

    let scope_block = match validated.scope.as_deref() {
        Some(scope) => {
            // A scope value that names an elevation bundle is rendered in the
            // words the elevation page uses, because it means the same thing
            // and this is the same decision: `administer` in a monospace list
            // beside `openid` reads as protocol furniture, and it is the one
            // line on this page somebody actually has to weigh.
            let items = scope
                .split_whitespace()
                .map(
                    |value| match crate::security::elevation::Grade::parse(value) {
                        Some(grade) => format!(
                            r#"<li><strong>{}</strong></li>"#,
                            html_escape::encode_text(grade.describe())
                        ),
                        None => {
                            format!("<li><code>{}</code></li>", html_escape::encode_text(value))
                        }
                    },
                )
                .collect::<String>();
            format!(
                r#"<div class="aw-detail"><span class="aw-detail-label">Access requested</span><ul class="aw-list">{}</ul></div>"#,
                items
            )
        }
        None => String::new(),
    };

    let resource_block = match validated.resource.as_deref() {
        Some(resource) => format!(
            r#"<div class="aw-detail"><span class="aw-detail-label">For</span><code>{}</code></div>"#,
            html_escape::encode_text(resource)
        ),
        None => String::new(),
    };

    let hidden = |name: &str, value: &str| {
        format!(
            r#"<input type="hidden" name="{}" value="{}">"#,
            name,
            html_attribute(value)
        )
    };

    let mut hidden_fields = String::new();
    hidden_fields.push_str(&hidden("csrf_token", csrf_token));
    hidden_fields.push_str(&hidden("response_type", "code"));
    hidden_fields.push_str(&hidden("client_id", &validated.client.client_id));
    hidden_fields.push_str(&hidden("redirect_uri", &validated.redirect_uri));
    hidden_fields.push_str(&hidden("code_challenge", &validated.code_challenge));
    hidden_fields.push_str(&hidden("code_challenge_method", "S256"));
    if let Some(scope) = validated.scope.as_deref() {
        hidden_fields.push_str(&hidden("scope", scope));
    }
    if let Some(state) = validated.state.as_deref() {
        hidden_fields.push_str(&hidden("state", state));
    }
    if let Some(resource) = validated.resource.as_deref() {
        hidden_fields.push_str(&hidden("resource", resource));
    }

    let body = format!(
        r#"<h1>Authorize {client_title}</h1>
        <p class="aw-identity">Signed in as {user_label}</p>

        <div class="aw-detail">
            <span class="aw-detail-label">Application</span>
            <strong>{client_title}</strong>
        </div>
        <div class="aw-detail">
            <span class="aw-detail-label">Will be sent to</span>
            <code>{redirect_uri}</code>
        </div>
        {scope_block}
        {resource_block}

        <p class="aw-notice aw-notice--warn">Anyone can register an application with this engine. Approve this only if you started it yourself and recognise where it sends you.</p>

        <form method="post" action="{consent_path}">
            {hidden_fields}
            <div class="aw-actions">
                <button type="submit" class="aw-button--secondary" name="decision" value="deny">Cancel</button>
                <button type="submit" name="decision" value="allow">Allow</button>
            </div>
        </form>"#,
        client_title = html_escape::encode_text(validated.client.display_name()),
        user_label = html_escape::encode_text(user_label),
        redirect_uri = html_escape::encode_text(&validated.redirect_uri),
        scope_block = scope_block,
        resource_block = resource_block,
        consent_path = CONSENT_PATH,
        hidden_fields = hidden_fields,
    );

    signed_in_page_response(
        &format!("Authorize {}", validated.client.display_name()),
        Width::Narrow,
        &body,
        &nonce,
    )
}

/// OAuth 2.0 authorization endpoint
///
/// Refuses anything it cannot account for: a `client_id` that was never
/// registered, a `redirect_uri` that client did not register, a request with no
/// PKCE challenge, and a `resource` naming somewhere this host does not serve.
/// What survives that is shown to the person it would act for, and only their
/// approval produces a code.
#[utoipa::path(
    get,
    path = "/auth/oauth2/authorize",
    tags = ["Authentication"],
    params(
        ("response_type" = String, Query, description = "Must be 'code' for authorization code flow"),
        ("client_id" = String, Query, description = "Identifier of a registered client"),
        ("redirect_uri" = String, Query, description = "Must exactly match a URI the client registered"),
        ("scope" = Option<String>, Query, description = "Requested scope"),
        ("state" = Option<String>, Query, description = "Opaque value returned with the code"),
        ("code_challenge" = String, Query, description = "PKCE code challenge (RFC 7636); required"),
        ("code_challenge_method" = String, Query, description = "Must be S256"),
        ("resource" = Option<String>, Query, description = "Resource indicator (RFC 8707); must name this host")
    ),
    responses(
        (status = 200, description = "Consent page, or an HTML redirect back to the client", content_type = "text/html"),
        (status = 302, description = "Redirect to login if not authenticated"),
        (status = 400, description = "Unknown client, unregistered redirect URI, or invalid request", body = crate::openapi_schemas::ErrorResponse),
    )
)]
pub async fn oauth2_authorize(
    State(oauth2_state): State<OAuth2State>,
    Query(params): Query<AuthorizeParams>,
    req: axum::extract::Request,
) -> Response {
    let host = get_request_host(req.headers());
    // Resolved from the host the request arrived on, not from the default base
    // URL: every configured host is its own authorization server here
    // (RFC 8414 §3.3), so naming the default one would put an issuer in the
    // response that the client's own discovery document disagrees with — which
    // is precisely the confusion `iss` exists to prevent.
    let issuer = oauth2_state
        .metadata
        .issuer_for_host(host.as_deref())
        .to_string();

    // Validated before authentication is considered, so a request that would be
    // refused anyway does not first cost someone a sign-in.
    let validated =
        match validate_authorize_request(&oauth2_state.pool, &params, host.as_deref()).await {
            Ok(validated) => validated,
            Err(rejection) => return rejection.into_response(&issuer),
        };

    let Some(auth_user) = req.extensions().get::<crate::auth::AuthUser>().cloned() else {
        let return_url = authorize_return_url(&params);
        return Redirect::to(&format!(
            "/auth/login?redirect={}",
            urlencoding::encode(&return_url)
        ))
        .into_response();
    };

    match consent_already_given(&oauth2_state.pool, &auth_user.user_id, &validated).await {
        Ok(true) => {}
        Ok(false) => {
            // Bound to the user, so a token minted for anyone else — including
            // one an attacker fetched from their own server — cannot be posted
            // back as this person's approval.
            let csrf_token = oauth2_state
                .auth_manager
                .security_context()
                .csrf
                .generate_token(Some(auth_user.user_id.clone()))
                .await
                .token;
            let label = user_label_for(&auth_user);
            return render_consent_page(&validated, &csrf_token, &label);
        }
        Err(e) => {
            tracing::error!("Could not read stored consent: {}", e);
            return AuthorizeRejection::Redirect {
                redirect_uri: validated.redirect_uri.clone(),
                state: validated.state.clone(),
                error: "server_error",
                description: "Could not read stored consent".to_string(),
            }
            .into_response(&issuer);
        }
    }

    issue_authorization_code(&oauth2_state.pool, &auth_user.user_id, &validated, &issuer).await
}

/// How to name the signed-in person on the consent page.
pub(super) fn user_label_for(auth_user: &crate::auth::AuthUser) -> String {
    auth_user
        .email
        .clone()
        .or_else(|| auth_user.name.clone())
        .unwrap_or_else(|| auth_user.user_id.clone())
}

/// The consent form's fields: the authorization request, plus the answer.
#[derive(Debug, Default, Deserialize)]
pub struct ConsentForm {
    #[serde(default)]
    pub(super) csrf_token: String,
    #[serde(default)]
    pub(super) decision: String,
    #[serde(default)]
    pub(super) response_type: String,
    #[serde(default)]
    pub(super) client_id: String,
    #[serde(default)]
    pub(super) redirect_uri: Option<String>,
    #[serde(default)]
    pub(super) scope: Option<String>,
    #[serde(default)]
    pub(super) state: Option<String>,
    #[serde(default)]
    pub(super) code_challenge: Option<String>,
    #[serde(default)]
    pub(super) code_challenge_method: Option<String>,
    #[serde(default)]
    pub(super) resource: Option<String>,
}

impl ConsentForm {
    /// The authorization request this form carries, so it can be re-validated
    /// rather than believed.
    pub(super) fn to_params(&self) -> AuthorizeParams {
        AuthorizeParams {
            response_type: self.response_type.clone(),
            client_id: self.client_id.clone(),
            redirect_uri: self.redirect_uri.clone(),
            scope: self.scope.clone(),
            state: self.state.clone(),
            code_challenge: self.code_challenge.clone(),
            code_challenge_method: self.code_challenge_method.clone(),
            resource: self.resource.clone(),
        }
    }
}

/// Act on the answer to a consent page.
///
/// Everything the form carries is re-validated here. The form is a
/// caller-supplied body like any other, and the only thing it is trusted for is
/// the answer itself — which is why it must carry a CSRF token: without one,
/// the page that could not get a code by navigation could get one by posting
/// this form instead.
#[utoipa::path(
    post,
    path = "/auth/oauth2/consent",
    tags = ["Authentication"],
    responses(
        (status = 200, description = "HTML redirect back to the client, with a code or an error", content_type = "text/html"),
        (status = 400, description = "Invalid request or CSRF token", body = crate::openapi_schemas::ErrorResponse),
        (status = 401, description = "No session", body = crate::openapi_schemas::ErrorResponse),
    )
)]
pub async fn oauth2_consent(
    State(oauth2_state): State<OAuth2State>,
    req: axum::extract::Request,
) -> Response {
    let auth_user = req.extensions().get::<crate::auth::AuthUser>().cloned();
    let (parts, body) = req.into_parts();

    let bytes = match axum::body::to_bytes(body, MAX_CONSENT_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "invalid_request".to_string(),
                    message: "Could not read the consent form".to_string(),
                }),
            )
                .into_response();
        }
    };

    let form: ConsentForm = serde_urlencoded::from_bytes(&bytes).unwrap_or_default();

    let Some(auth_user) = auth_user else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "authentication_required".to_string(),
                message: "User must be authenticated".to_string(),
            }),
        )
            .into_response();
    };

    // A form, so it needs a token — and one issued to this user, on the consent
    // page this engine rendered for them. An unbound token would not do: those
    // can be collected from the sign-in page by anyone, with no browser and no
    // account, which would leave nothing here but the cookie's `SameSite=Lax`.
    if oauth2_state
        .auth_manager
        .security_context()
        .csrf
        .validate_token_for(&form.csrf_token, &auth_user.user_id)
        .await
        .is_err()
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid_request".to_string(),
                message: "Missing or invalid CSRF token".to_string(),
            }),
        )
            .into_response();
    }

    let params = form.to_params();
    let host = get_request_host(&parts.headers);
    let issuer = oauth2_state
        .metadata
        .issuer_for_host(host.as_deref())
        .to_string();
    let validated =
        match validate_authorize_request(&oauth2_state.pool, &params, host.as_deref()).await {
            Ok(validated) => validated,
            Err(rejection) => return rejection.into_response(&issuer),
        };

    if form.decision != "allow" {
        return AuthorizeRejection::Redirect {
            redirect_uri: validated.redirect_uri.clone(),
            state: validated.state.clone(),
            error: "access_denied",
            description: "The request was declined".to_string(),
        }
        .into_response(&issuer);
    }

    if let Err(e) = record_consent(&oauth2_state.pool, &auth_user.user_id, &validated).await {
        tracing::error!("Could not record consent: {}", e);
        return AuthorizeRejection::Redirect {
            redirect_uri: validated.redirect_uri.clone(),
            state: validated.state.clone(),
            error: "server_error",
            description: "Could not record the approval".to_string(),
        }
        .into_response(&issuer);
    }

    issue_authorization_code(&oauth2_state.pool, &auth_user.user_id, &validated, &issuer).await
}

/// OAuth 2.0 token request parameters (RFC 6749)
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct TokenParams {
    /// Grant type
    pub(super) grant_type: String,

    /// Authorization code (for authorization_code grant)
    #[serde(default)]
    pub(super) code: Option<String>,

    /// Redirect URI (for authorization_code grant)
    #[serde(default)]
    pub(super) redirect_uri: Option<String>,

    /// PKCE code verifier (RFC 7636)
    #[serde(default)]
    pub(super) code_verifier: Option<String>,

    /// Refresh token (for refresh_token grant)
    #[serde(default)]
    pub(super) refresh_token: Option<String>,

    /// Client identifier
    #[serde(default)]
    pub(super) client_id: Option<String>,

    /// Client secret, for a confidential client that authenticates in the body
    /// rather than with an `Authorization: Basic` header.
    #[serde(default)]
    pub(super) client_secret: Option<String>,
}

/// Client credentials presented at the token endpoint.
///
/// RFC 6749 §2.3.1 puts them in an `Authorization: Basic` header and permits
/// the request body as an alternative; both are read, and the header wins when
/// a client sends both. A public client presents an identifier and no secret,
/// which is the normal case here — every MCP client that registers dynamically
/// is one.
pub(super) fn client_credentials(
    headers: &HeaderMap,
    params: &TokenParams,
) -> (Option<String>, Option<String>) {
    if let Some(encoded) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            let mut parts = value.splitn(2, ' ');
            let scheme = parts.next().unwrap_or_default();
            scheme
                .eq_ignore_ascii_case("basic")
                .then(|| parts.next().unwrap_or_default().trim())
        })
    {
        use base64::Engine;
        if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded)
            && let Ok(decoded) = String::from_utf8(decoded)
            && let Some((id, secret)) = decoded.split_once(':')
        {
            // Both halves are form-urlencoded before being base64'd.
            let decode = |value: &str| {
                urlencoding::decode(value)
                    .map(|decoded| decoded.into_owned())
                    .unwrap_or_else(|_| value.to_string())
            };
            return (Some(decode(id)), Some(decode(secret)));
        }
    }

    (
        params
            .client_id
            .clone()
            .filter(|value| !value.trim().is_empty()),
        params
            .client_secret
            .clone()
            .filter(|value| !value.is_empty()),
    )
}

/// Whether a PKCE verifier matches the challenge its code was issued with.
///
/// S256 only. `plain` is permitted by RFC 7636 and is worth nothing here: the
/// challenge travels in the same query string the verifier would, so anyone
/// able to read one can read the other.
pub(super) fn pkce_verifier_matches(verifier: &str, challenge: &str, method: Option<&str>) -> bool {
    if method != Some("S256") {
        return false;
    }

    use base64::Engine;
    use sha2::{Digest, Sha256};
    let computed = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));

    computed.len() == challenge.len()
        && computed
            .bytes()
            .zip(challenge.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// An OAuth2 error response, in the shape RFC 6749 §5.2 asks for.
pub(super) fn oauth_error(status: StatusCode, error: &str, message: &str) -> Response {
    (
        status,
        Json(ErrorResponse {
            error: error.to_string(),
            message: message.to_string(),
        }),
    )
        .into_response()
}

/// Establish which registered client is making a token request.
///
/// Both grants need the same answer, and both need it before anything is spent:
/// a client that fails authentication must not burn an authorization code or a
/// refresh token that the real one was about to present.
///
/// A confidential client proves it is itself with its secret. A public client
/// holds no secret to prove anything with — PKCE stands in for that on the
/// authorization-code grant, and on the refresh grant what stands in is the
/// token being single-use and bound to the client it was issued to.
// The error is an already-rendered HTTP response, returned straight to the
// caller's caller; boxing it would allocate on a path that immediately unwraps
// it back into a response.
#[allow(clippy::result_large_err)]
pub(super) async fn authenticate_client(
    pool: &PgPool,
    headers: &HeaderMap,
    params: &TokenParams,
) -> Result<crate::auth::client_registration::RegisteredClient, Response> {
    let (presented_client_id, presented_secret) = client_credentials(headers, params);
    let Some(presented_client_id) = presented_client_id else {
        return Err(oauth_error(
            StatusCode::UNAUTHORIZED,
            "invalid_client",
            "client_id is required",
        ));
    };

    let client =
        match crate::auth::client_registration::lookup_client(pool, &presented_client_id).await {
            Ok(Some(client)) => client,
            Ok(None) => {
                return Err(oauth_error(
                    StatusCode::UNAUTHORIZED,
                    "invalid_client",
                    "Unknown client_id",
                ));
            }
            Err(e) => {
                tracing::error!("Client lookup failed: {}", e);
                return Err(oauth_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "server_error",
                    "Could not read the client registry",
                ));
            }
        };

    if let Some(expected_hash) = client.client_secret_hash.as_deref() {
        if let Some(expires_at) = client.client_secret_expires_at
            && expires_at <= Utc::now()
        {
            return Err(oauth_error(
                StatusCode::UNAUTHORIZED,
                "invalid_client",
                "Client secret has expired",
            ));
        }

        let presented_secret = presented_secret.unwrap_or_default();
        if !crate::auth::client_registration::client_secret_matches(
            &presented_secret,
            expected_hash,
        ) {
            return Err(oauth_error(
                StatusCode::UNAUTHORIZED,
                "invalid_client",
                "Client authentication failed",
            ));
        }
    }

    Ok(client)
}

/// OAuth 2.0 token response
#[derive(Debug, Serialize)]
pub struct TokenResponse {
    pub(super) access_token: String,
    pub(super) token_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) expires_in: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) scope: Option<String>,
}

/// OAuth 2.0 token endpoint
/// This endpoint issues access tokens in exchange for authorization codes
#[utoipa::path(
    post,
    path = "/auth/oauth2/token",
    tags = ["Authentication"],
    request_body(content = TokenParams, content_type = "application/x-www-form-urlencoded"),
    responses(
        (status = 200, description = "Access token issued successfully", body = crate::openapi_schemas::OAuth2TokenResponse),
        (status = 400, description = "Invalid token request", body = crate::openapi_schemas::ErrorResponse),
        (status = 500, description = "Server error", body = crate::openapi_schemas::ErrorResponse),
    )
)]
pub async fn oauth2_token(
    State(oauth2_state): State<OAuth2State>,
    headers: HeaderMap,
    axum::Form(params): axum::Form<TokenParams>,
) -> Response {
    tracing::info!("📩 Token exchange request received");
    tracing::info!("  grant_type: {}", params.grant_type);
    tracing::info!("  code: {:?}", params.code);
    tracing::info!("  client_id: {:?}", params.client_id);
    tracing::info!("  redirect_uri: {:?}", params.redirect_uri);

    if params.grant_type == "refresh_token" {
        return handle_refresh_token_grant(&oauth2_state, &headers, &params).await;
    }

    // Validate grant_type
    if params.grant_type != "authorization_code" {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "unsupported_grant_type".to_string(),
                message: "Only authorization_code and refresh_token grant types are supported"
                    .to_string(),
            }),
        )
            .into_response();
    }

    // Validate required parameters
    let code = match params.code {
        Some(ref c) if c.starts_with("code_") => c,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "invalid_request".to_string(),
                    message: "Missing or invalid code parameter".to_string(),
                }),
            )
                .into_response();
        }
    };

    tracing::info!("Exchanging code: {}", code);

    // Who is redeeming this? Established before the code is consumed, so a
    // client that fails authentication does not burn a code a legitimate one
    // was about to present.
    let client = match authenticate_client(&oauth2_state.pool, &headers, &params).await {
        Ok(client) => client,
        Err(response) => return response,
    };
    let presented_client_id = client.client_id.clone();

    // Retrieve and validate the authorization code
    let mut tx = match oauth2_state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!("Database error: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "server_error".to_string(),
                    message: "Database error".to_string(),
                }),
            )
                .into_response();
        }
    };

    let code_data_opt: Option<AuthorizationCodeData> =
        sqlx::query_as("SELECT * FROM oauth_authorization_codes WHERE code = $1 FOR UPDATE")
            .bind(code)
            .fetch_optional(&mut *tx)
            .await
            .unwrap_or(None);

    let code_data = match code_data_opt {
        Some(data) if !data.used && data.expires_at > Utc::now() => {
            // Mark code as used
            let _ = sqlx::query("UPDATE oauth_authorization_codes SET used = TRUE WHERE code = $1")
                .bind(code)
                .execute(&mut *tx)
                .await;
            data
        }
        Some(data) if data.used => {
            let _ = tx.rollback().await;
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "invalid_grant".to_string(),
                    message: "Authorization code has already been used".to_string(),
                }),
            )
                .into_response();
        }
        Some(_) => {
            let _ = tx.rollback().await;
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "invalid_grant".to_string(),
                    message: "Authorization code has expired".to_string(),
                }),
            )
                .into_response();
        }
        None => {
            let _ = tx.rollback().await;
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "invalid_grant".to_string(),
                    message: "Invalid authorization code".to_string(),
                }),
            )
                .into_response();
        }
    };

    if let Err(e) = tx.commit().await {
        tracing::error!("Database commit error: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "server_error".to_string(),
                message: "Database error".to_string(),
            }),
        )
            .into_response();
    }

    // The code belongs to the client it was issued to. Without this, a code
    // intercepted from one client is redeemable by any other that can reach
    // this endpoint.
    if code_data.client_id != presented_client_id {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "Authorization code was not issued to this client",
        );
    }

    // Exact match, and required rather than checked only when offered: a
    // redirect URI the client declines to repeat is one it cannot be held to.
    let presented_redirect = params
        .redirect_uri
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    if presented_redirect != code_data.redirect_uri {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "redirect_uri does not match the authorization request",
        );
    }

    // PKCE, unconditionally. Verifying a challenge only when one happened to be
    // stored is what made it optional: a caller who sent none was never asked
    // for a verifier, so the code alone was enough. The authorization endpoint
    // now requires a challenge, and a stored code without one predates that and
    // is refused rather than waved through.
    let Some(challenge) = code_data.code_challenge.as_deref() else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "Authorization code was issued without PKCE and cannot be redeemed",
        );
    };

    let Some(verifier) = params.code_verifier.as_deref().filter(|v| !v.is_empty()) else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "code_verifier is required",
        );
    };

    if !pkce_verifier_matches(
        verifier,
        challenge,
        code_data.code_challenge_method.as_deref(),
    ) {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "PKCE verification failed",
        );
    }

    // Create a session for the user
    let ip_addr = client_ip::from_headers(&headers);
    let user_agent = client_ip::user_agent_from_headers(&headers);

    // Carry the user's identity and roles onto the session. Downstream nothing
    // distinguishes a session minted here from a browser login — the
    // administrator-only engine APIs read `is_admin` straight off it — so the
    // roles have to come from the user repository instead of defaulting to none.
    let identity = session_identity_for_user(&code_data.user_id).await;

    // Every token this endpoint issues carries an audience, whether or not the
    // client sent a `resource` parameter. That is what makes "has an audience"
    // mean "was minted for programmatic use", and so what lets session
    // validation refuse a browser cookie presented as a bearer token. A client
    // that named a resource keeps its own; one that did not gets the MCP
    // endpoint on the host it is talking to.
    //
    // Computed once, because the refresh token records the same audience: a
    // session minted from it must not reach anywhere the original
    // authorization did not.
    let audience = Some(code_data.resource.clone().unwrap_or_else(|| {
        // The canonical host, matching what the MCP endpoint compares against:
        // a request arriving on an unconfigured name is served the default
        // host's content, so a token minted there must name the host it will
        // actually be used against.
        let host = crate::hosts::resolved_host(get_request_host(&headers).as_deref());
        format!("{}{}", host, crate::auth::mcp_middleware::MCP_ENDPOINT_PATH)
    }));

    let session_params = crate::auth::session::CreateAuthSessionParams {
        user_id: code_data.user_id.clone(),
        provider: "oauth2".to_string(),
        email: identity.email,
        name: identity.name,
        is_admin: identity.is_admin,
        is_editor: identity.is_editor,
        ip_addr: ip_addr.clone(),
        user_agent: user_agent.clone(),
        refresh_token: None,
        realm: identity.realm,
        audience: audience.clone(),
        // What the person consented to on the way here. The bundles are read
        // out of the scope that travelled with the code, so a client cannot
        // ask for more at redemption than it was approved for at consent.
        //
        // It lasts as long as the session can rather than the step-up ceiling:
        // a program holds a credential and cannot be asked to type a password
        // in half an hour, and the proof of presence was the consent screen.
        elevation: token_elevation(
            code_data.scope.as_deref(),
            oauth2_state.auth_manager.config().max_session_age,
        ),
    };

    match oauth2_state
        .auth_manager
        .session_manager()
        .create_session(session_params)
        .await
    {
        Ok(session_token) => {
            tracing::info!(
                "Token exchange successful, created session for user: {}",
                code_data.user_id
            );

            let config = oauth2_state.auth_manager.config();

            // A refresh token is a different credential from the session it
            // mints, which is the whole point: this endpoint used to answer
            // with the session token in both fields, so rotation was impossible
            // and a leaked refresh token was a leaked access token.
            let refresh_token = match crate::auth::refresh_tokens::issue(
                &oauth2_state.pool,
                &code_data.user_id,
                &presented_client_id,
                audience.as_deref(),
                code_data.scope.as_deref(),
                None,
                chrono::Duration::seconds(config.max_session_age as i64),
            )
            .await
            {
                Ok(token) => Some(token),
                Err(e) => {
                    // The access token is sound and the client can use it; it
                    // just has to come back through an authorization when the
                    // session times out. Better than failing an exchange that
                    // otherwise succeeded.
                    tracing::error!("Could not issue a refresh token: {}", e);
                    None
                }
            };

            let response = TokenResponse {
                access_token: session_token.token,
                token_type: "Bearer".to_string(),
                expires_in: Some(config.session_timeout),
                refresh_token,
                scope: code_data.scope,
            };

            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            tracing::error!("Failed to create session: {:?}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "server_error".to_string(),
                    message: "Failed to create session".to_string(),
                }),
            )
                .into_response()
        }
    }
}

/// Identity and roles to stamp onto a session minted for a user.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SessionIdentity {
    pub email: Option<String>,
    pub name: Option<String>,
    pub is_admin: bool,
    pub is_editor: bool,
    /// The host the account is a principal on. Empty when the user record
    /// could not be read, which authorizes nothing — the same fail-closed
    /// answer the roles get.
    pub realm: String,
}

/// Look up the roles a session for `user_id` should carry.
///
/// A user whose record cannot be read gets a session with no roles rather than
/// no session at all: the token exchange has already verified the
/// authorization code, and failing closed on roles keeps a transient database
/// error from handing out privileges it could not confirm.
pub async fn session_identity_for_user(user_id: &str) -> SessionIdentity {
    match crate::user_repository::get_user_async(user_id).await {
        Ok(user) => SessionIdentity {
            email: user.email,
            name: user.name,
            is_admin: user
                .roles
                .contains(&crate::user_repository::UserRole::Administrator),
            is_editor: user
                .roles
                .contains(&crate::user_repository::UserRole::Editor),
            realm: user.realm,
        },
        Err(e) => {
            tracing::warn!(
                "Could not load user {} while minting a session; issuing it with no roles: {}",
                user_id,
                e
            );
            SessionIdentity::default()
        }
    }
}

/// The `refresh_token` grant (RFC 6749 §6).
///
/// A refresh token is not a session and cannot be presented as one. It is
/// redeemed here, by the client it was issued to, for a *new* session — which
/// is why this reads roles and realm from the repository rather than copying
/// them off whatever the previous session carried. An account that lost the
/// administrator role does not get it back by refreshing.
///
/// Single use: redeeming rotates the token, and presenting a spent one revokes
/// the whole chain. See [`crate::auth::refresh_tokens`].
pub(super) async fn handle_refresh_token_grant(
    oauth2_state: &OAuth2State,
    headers: &HeaderMap,
    params: &TokenParams,
) -> Response {
    let Some(presented) = params.refresh_token.as_deref().filter(|t| !t.is_empty()) else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "refresh_token is required for refresh_token grant",
        );
    };

    // Established before the token is spent, so a client that fails
    // authentication cannot burn a token the real one was about to present.
    let client = match authenticate_client(&oauth2_state.pool, headers, params).await {
        Ok(client) => client,
        Err(response) => return response,
    };

    let grant =
        match crate::auth::refresh_tokens::redeem(&oauth2_state.pool, presented, &client.client_id)
            .await
        {
            Ok(grant) => grant,
            Err(err) => {
                tracing::warn!("Refresh token grant rejected: {}", err);
                // One answer for every reason. Which of "never existed",
                // "expired", "already spent" and "belongs to another client"
                // it was is not something the presenter should be able to
                // learn by asking.
                return oauth_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_grant",
                    "Invalid or expired refresh token",
                );
            }
        };

    let ip_addr = client_ip::from_headers(headers);
    let user_agent = client_ip::user_agent_from_headers(headers);

    // Read afresh. A session carries the roles and realm it was minted with, so
    // a refresh is the moment a revocation that happened in between takes
    // effect — copying them forward would make a refresh token a way to keep
    // holding a role that was taken away.
    let identity = session_identity_for_user(&grant.user_id).await;

    let session_params = crate::auth::session::CreateAuthSessionParams {
        user_id: grant.user_id.clone(),
        provider: "oauth2".to_string(),
        email: identity.email,
        name: identity.name,
        is_admin: identity.is_admin,
        is_editor: identity.is_editor,
        ip_addr,
        user_agent,
        refresh_token: None,
        realm: identity.realm,
        // The audience the original authorization was for, never re-derived
        // from this request: refreshing must not widen where a token reaches.
        audience: grant.audience.clone(),
        // Re-read, for the reason the roles above are. A session carries what
        // it was minted with, so a refresh is the moment a withdrawal takes
        // effect — and an elevation copied forward would make a refresh token
        // a way to go on holding authority the person has taken back.
        //
        // The narrower of two: what this token was issued for, and what the
        // person consents to now. Either alone leaves one side open — the
        // first would carry a withdrawn grant, the second would let a narrow
        // token widen because the consent is wide.
        elevation: token_elevation_for_refresh(
            &oauth2_state.pool,
            &grant.user_id,
            &client.client_id,
            grant.scope.as_deref(),
            oauth2_state.auth_manager.config().max_session_age,
        )
        .await,
    };

    let session_token = match oauth2_state
        .auth_manager
        .session_manager()
        .create_session(session_params)
        .await
    {
        Ok(token) => token,
        Err(err) => {
            tracing::error!("Refresh token grant could not mint a session: {:?}", err);
            return oauth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "Failed to create session",
            );
        }
    };

    let config = oauth2_state.auth_manager.config();

    // Rotation: the spent token's successor, in the same family. If this fails
    // the client still has a working access token and can re-authorize when it
    // expires, which is better than refusing a refresh that already happened.
    let refresh_token = match crate::auth::refresh_tokens::issue(
        &oauth2_state.pool,
        &grant.user_id,
        &grant.client_id,
        grant.audience.as_deref(),
        grant.scope.as_deref(),
        Some(&grant.family_id),
        chrono::Duration::seconds(config.max_session_age as i64),
    )
    .await
    {
        Ok(token) => Some(token),
        Err(e) => {
            tracing::error!("Could not rotate a refresh token: {}", e);
            None
        }
    };

    (
        StatusCode::OK,
        Json(TokenResponse {
            access_token: session_token.token,
            token_type: "Bearer".to_string(),
            expires_in: Some(config.session_timeout),
            refresh_token,
            scope: grant.scope,
        }),
    )
        .into_response()
}
