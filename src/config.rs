use anyhow::{Context, Result};
use figment::{
    Figment,
    providers::{Env, Format, Serialized, Toml, Yaml},
};
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, time::Duration};

/// Application configuration with comprehensive settings for all components
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    /// Server configuration
    pub server: ServerConfig,

    /// Logging configuration
    pub logging: LoggingConfig,

    /// JavaScript engine configuration
    pub javascript: JavaScriptConfig,

    /// Repository configuration
    pub repository: RepositoryConfig,

    /// Security configuration
    pub security: SecurityConfig,

    /// Authentication configuration (optional)
    #[serde(default)]
    pub auth: Option<crate::auth::AuthConfig>,

    /// Script revision history and how long it is kept.
    ///
    /// Defaulted rather than required, so a configuration written before
    /// revisions existed keeps loading and gets the same policy a new one
    /// would.
    #[serde(default)]
    pub revisions: RevisionsConfig,

    /// Script log retention.
    ///
    /// Defaulted for the same reason `revisions` is: a configuration written
    /// before this section existed keeps loading, and gets the policy a new
    /// one would.
    #[serde(default)]
    pub logs: LogsConfig,
}

/// How much of a script's history to keep.
///
/// Every write records a revision, which is what makes the history useful and
/// also what makes it grow. These are the two dials that decide when a
/// revision stops being worth its rows — see `revisions::Retention` for what
/// is kept regardless of them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionsConfig {
    /// Whether to prune at all. Off means the history grows forever, which is
    /// a defensible choice for a small engine and a leak for a busy one.
    pub prune_enabled: bool,

    /// Keep every revision younger than this many days.
    pub retention_days: u32,

    /// Keep this many of the newest revisions per script whatever their age.
    pub keep_per_script: u32,

    /// How often to run a pruning pass, in seconds.
    pub prune_interval_secs: u64,
}

impl Default for RevisionsConfig {
    fn default() -> Self {
        Self {
            prune_enabled: true,
            retention_days: 30,
            keep_per_script: 50,
            // Hourly. The pass is a pair of indexed deletes behind an advisory
            // lock, and nothing downstream depends on it being prompt.
            prune_interval_secs: 3600,
        }
    }
}

/// How much of a script's log to keep.
///
/// Logs are written by scripts, in whatever volume a script cares to write
/// them, and nothing downstream depends on an old line still being there.
/// That makes retention here a question of bounding a shared table rather
/// than of preserving history, which is why both dials are enforced and
/// either one alone is enough to delete a row — unlike [`RevisionsConfig`],
/// where a revision has to fall outside *both* before it goes.
///
/// The two bound different things and neither bounds the table alone: a count
/// per script leaves a thousand dormant scripts holding a hundred lines each,
/// and an age window leaves one script logging in a loop to fill it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogsConfig {
    /// Whether to prune at all. Off means the `logs` table grows until
    /// somebody empties it by hand.
    pub prune_enabled: bool,

    /// Delete log lines older than this many hours.
    pub retention_hours: u64,

    /// Keep at most this many of the newest lines per script, whatever their
    /// age.
    pub keep_per_script: u32,

    /// How often to run a pruning pass, in seconds.
    pub prune_interval_secs: u64,
}

impl Default for LogsConfig {
    fn default() -> Self {
        Self {
            prune_enabled: true,
            retention_hours: 24,
            // Was hardcoded at 20 for as long as pruning was a manual action.
            // A background pass makes a larger window affordable, and 20 is
            // too few to debug anything that logs more than a line per step.
            keep_per_script: 100,
            // Hourly, matching the revision pruner: one indexed delete behind
            // an advisory lock, and nothing waits on it.
            prune_interval_secs: 3600,
        }
    }
}

/// Server-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Server bind address
    pub host: String,

    /// Server port
    pub port: u16,

    /// Base URL for the server (e.g., "https://softage.com" or "http://localhost:8080")
    /// Used for OAuth metadata, redirects, and other absolute URLs
    /// If not provided, will be constructed from host and port
    #[serde(default)]
    pub base_url: Option<String>,

    /// Additional base URLs this engine answers to, one per extra hostname
    /// (e.g. ["https://manage.softagen.com", "https://world.softagen.com"]).
    ///
    /// A login started on one of these hosts completes on that same host, so
    /// its session cookie is set there rather than on `base_url`. Every entry
    /// must have its callback path registered with each OAuth provider, and a
    /// hostname absent from this list gets the `base_url` behaviour.
    #[serde(default)]
    pub additional_base_urls: Vec<String>,

    /// Hosts allowed to serve the engine's management APIs — every `/engine/*`
    /// endpoint except the static `/engine/installed` page, which stays
    /// available everywhere as the landing page for `/`.
    ///
    /// Entries are matched against the request's Host header and may be
    /// written either bare (`manage.example.com`, `localhost:3000`) or as a
    /// full base URL. Requests for a management endpoint on any other host are
    /// answered with 404, as though the endpoint did not exist there.
    ///
    /// Leaving this empty serves management on every host, which is the
    /// behaviour of a single-host deployment. Set it as soon as scripts serve
    /// content on a host that should not be able to reach these APIs from a
    /// logged-in administrator's browser.
    #[serde(default)]
    pub management_hosts: Vec<String>,

    /// Proxies whose forwarding headers this engine will believe.
    ///
    /// Entries are addresses (`127.0.0.1`) or networks (`172.16.0.0/12`).
    /// `X-Forwarded-For` and `X-Real-IP` are read only when the connection
    /// itself came from one of these; anything else is keyed by the address it
    /// connected from, because a forwarding header is written by whoever is
    /// talking to us and says whatever they like.
    ///
    /// Empty — the default — trusts nothing, which is right for an engine
    /// reached directly and fails closed for one that is not: everybody is
    /// named by their real peer, which for a proxied deployment is the proxy.
    /// Set it to the proxy's address as soon as one is in front, or every
    /// caller shares a rate-limit bucket.
    #[serde(default)]
    pub trusted_proxies: Vec<String>,

    /// Enable graceful shutdown
    pub graceful_shutdown: bool,

    /// Shutdown timeout in seconds
    pub shutdown_timeout_secs: u64,
}

/// Logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log level (trace, debug, info, warn, error)
    pub level: String,

    /// Log format (json, pretty, compact)
    pub format: String,
}

/// Stack a script may use before QuickJS throws, when nothing says otherwise.
///
/// What `js_engine` hardcoded for as long as `stack_size_bytes` was read by
/// nothing, kept as the default so honouring the setting does not quietly
/// change what an engine that never set it does.
pub const DEFAULT_STACK_SIZE_BYTES: usize = 512 * 1024;

/// Below this a script has too little stack to be worth running.
///
/// Measured against QuickJS, a JavaScript frame costs on the order of a
/// kilobyte: 64 KB buys about 59 frames of recursion, 512 KB about 500. The
/// floor is where a script can still do something; it is not a recommendation.
pub const MIN_STACK_SIZE_BYTES: usize = 64 * 1024;

/// JavaScript engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JavaScriptConfig {
    /// Script execution timeout in milliseconds
    pub execution_timeout_ms: u64,

    /// Maximum memory usage per script in bytes
    pub max_memory_bytes: usize,

    /// Maximum number of concurrent script executions
    pub max_concurrent_executions: usize,

    /// Script execution stack size in bytes
    pub stack_size_bytes: usize,

    /// Enable script init() function calls
    #[serde(default = "default_enable_init_functions")]
    pub enable_init_functions: bool,

    /// Init function timeout in milliseconds (defaults to execution_timeout_ms if not set)
    pub init_timeout_ms: Option<u64>,

    /// Budget for one test module in milliseconds (defaults to
    /// execution_timeout_ms if not set). Each module runs in its own runtime
    /// and gets this budget of its own, so one slow file does not spend the
    /// time the rest of them need.
    #[serde(default)]
    pub test_timeout_ms: Option<u64>,

    /// Ceiling on a whole test run in milliseconds. Reached, the run stops
    /// starting modules and reports what it has, so a script with many test
    /// files cannot hold a request open for modules × test_timeout_ms.
    #[serde(default)]
    pub test_run_timeout_ms: Option<u64>,

    /// Fail server startup if any script init fails
    #[serde(default)]
    pub fail_startup_on_init_error: bool,
}

fn default_enable_init_functions() -> bool {
    true
}

fn default_db_max_connections() -> u32 {
    20
}

fn default_lock_timeout_ms() -> u64 {
    5_000
}

fn default_statement_timeout_ms() -> u64 {
    30_000
}

fn default_idle_in_transaction_timeout_ms() -> u64 {
    300_000
}

fn default_embedded_data_dir() -> String {
    "data/postgres".to_string()
}

/// Repository configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryConfig {
    /// PostgreSQL database connection string (required)
    /// Specified as 'database_url' in config files and APP_REPOSITORY__DATABASE_URL in env
    #[serde(rename = "database_url")]
    pub connection_string: String,

    /// Start a PostgreSQL server inside this process instead of connecting to
    /// one, for a desktop install that has no database to point at.
    ///
    /// Requires a build carrying the `embedded-postgres` feature; without it
    /// this is refused at startup rather than quietly falling back to
    /// `database_url`, which would connect to whatever the placeholder names.
    /// When it is on, `database_url` is ignored: the connection string is
    /// whatever the server that just started is listening on.
    #[serde(default)]
    pub embedded: bool,

    /// Where the embedded server keeps its cluster, its binaries and the
    /// password it was initialised with. Relative paths resolve against the
    /// working directory; a packaged desktop app sets this to its per-user
    /// data directory.
    ///
    /// This is the whole of a desktop install's state — backing it up while the
    /// app is stopped is the backup.
    #[serde(default = "default_embedded_data_dir")]
    pub embedded_data_dir: String,

    /// The loopback port the embedded server listens on. `0` takes a free one
    /// at startup, which is the right default: nothing outside this process
    /// connects to it, and a fixed port is one more thing that can already be
    /// taken.
    #[serde(default)]
    pub embedded_port: u16,

    /// Maximum script size in bytes
    pub max_script_size_bytes: usize,

    /// Maximum upload file size in bytes
    pub max_upload_size_bytes: usize,

    /// Maximum size of the PostgreSQL connection pool.
    ///
    /// Every JavaScript database call blocks a pool connection for its whole
    /// round trip, and a single request can make many — so this bounds how many
    /// script executions can touch the database at once, not just how many
    /// queries run concurrently. Remember to multiply by the number of engine
    /// instances when sizing the server's own `max_connections`.
    #[serde(default = "default_db_max_connections")]
    pub max_connections: u32,

    /// How long a statement waits for a lock before giving up, in milliseconds.
    /// `0` disables the limit, as it does in Postgres.
    ///
    /// Without this a statement blocked on a lock waits forever, and nothing in
    /// the engine can interrupt it: the wait happens inside a host call, where
    /// the interrupt handler that enforces a script's budget cannot reach, and
    /// the outer timeout only abandons the thread. One blocked statement then
    /// holds its own locks indefinitely and every later writer queues behind
    /// it. This turns that from a permanent wedge into an error a script can
    /// see and a log line an operator can find.
    ///
    /// The default sits well inside the JavaScript execution budget, so a
    /// handler learns it lost the race with time left to answer.
    #[serde(default = "default_lock_timeout_ms")]
    pub lock_timeout_ms: u64,

    /// How long any single statement may run before Postgres cancels it, in
    /// milliseconds. `0` disables the limit.
    ///
    /// A backstop rather than a policy: the JavaScript budget already bounds
    /// what a handler can ask for, so this is aimed at the query no timeout
    /// upstream managed to bound. Migrations are exempt — see
    /// [`Database::migrate`].
    #[serde(default = "default_statement_timeout_ms")]
    pub statement_timeout_ms: u64,

    /// How long a transaction may sit idle before Postgres ends it and releases
    /// its locks, in milliseconds. `0` disables the limit.
    ///
    /// This is what reaps a transaction whose handler was abandoned mid-run: a
    /// budget kill does not unwind the JavaScript stack, so the commit or
    /// rollback at the handler boundary never runs and the transaction's locks
    /// outlive the request that took them.
    ///
    /// Deliberately generous. A transaction that has been idle this long is
    /// leaked by any reading, and the bound that should actually fire first is
    /// the one `database.beginTransaction(timeoutMs)` asks for.
    #[serde(default = "default_idle_in_transaction_timeout_ms")]
    pub idle_in_transaction_timeout_ms: u64,
}

/// Security configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Whether the engine applies a CORS policy to its own paths at all.
    pub enable_cors: bool,

    /// Origins allowed to read engine-owned responses (`/engine`, `/auth`,
    /// `/graphql`, `/mcp`, …). Empty means same-origin only.
    ///
    /// Each entry is an origin — `scheme://host[:port]`, no path — matched
    /// exactly. `"*"` allows any origin *without credentials*, which is the
    /// only thing a browser will honour for a wildcard; see
    /// [`crate::security::cors`].
    pub cors_allowed_origins: Vec<String>,

    /// Optional base64-encoded 32-byte CSRF key used to validate tokens across instances
    /// Example (env): APP_SECURITY__CSRF_KEY
    #[serde(default)]
    pub csrf_key: Option<String>,

    /// Enable security headers
    pub enable_security_headers: bool,

    /// Content Security Policy header value
    pub content_security_policy: Option<String>,

    /// Maximum request body size in bytes
    pub max_request_body_bytes: usize,

    /// Hold every session to the address it was signed in from.
    ///
    /// Off by default: a phone changing networks mid-session would otherwise be
    /// signed out for it. On a deployment whose callers do not move — a
    /// personal install, an engine reached from fixed addresses — it is cheap,
    /// and it turns a stolen session token into one that only works from the
    /// place it was stolen from.
    ///
    /// Only meaningful because `server.trusted_proxies` establishes the address
    /// from the connection rather than from a header the caller wrote.
    #[serde(default)]
    pub strict_ip_validation: bool,

    /// Optional base64-encoded 32-byte encryption key used for session encryption
    /// Example (env): APP_SECURITY__SESSION_ENCRYPTION_KEY
    #[serde(default)]
    pub session_encryption_key: Option<String>,

    /// Optional base64-encoded 32-byte key for encrypting secrets stored in the database.
    /// If not set, secrets are stored as plaintext with a warning logged at startup.
    /// Example (env): APP_SECURITY__SECRET_ENCRYPTION_KEY
    #[serde(default)]
    pub secret_encryption_key: Option<String>,

    /// Optional API key for machine-to-machine authentication (e.g. MCP)
    /// Example (env): APP_SECURITY__API_KEY
    #[serde(default)]
    pub api_key: Option<String>,
}

/// Whether a configured secret is one nobody chose.
///
/// Two shapes matter: the value shipped in a config template, which is public
/// and long enough to pass every other check, and an unexpanded `${VAR}`
/// placeholder, which means the environment the template expected was not
/// there.
fn placeholder_secret(secret: &str) -> Option<&'static str> {
    let trimmed = secret.trim();

    if trimmed.starts_with("${") && trimmed.ends_with('}') {
        return Some("an unexpanded environment placeholder");
    }

    let lowered = trimmed.to_lowercase();
    if ["dev-jwt-secret", "change-me", "changeme", "your-secret"]
        .iter()
        .any(|marker| lowered.contains(marker))
    {
        return Some("a shipped placeholder value");
    }

    None
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8080,
            base_url: None,
            additional_base_urls: Vec::new(),
            management_hosts: Vec::new(),
            trusted_proxies: Vec::new(),
            graceful_shutdown: true,
            shutdown_timeout_secs: 30,
        }
    }
}

impl ServerConfig {
    /// Get the base URL for the server
    /// If base_url is configured, use it. Otherwise construct from host and port.
    pub fn get_base_url(&self) -> String {
        if let Some(ref base_url) = self.base_url {
            base_url.clone()
        } else {
            // Construct from host and port
            // If host is 0.0.0.0 or ::, use localhost instead
            let host = if self.host == "0.0.0.0" || self.host == "::" {
                "localhost"
            } else {
                &self.host
            };

            // Use https for standard port 443, http otherwise
            let scheme = if self.port == 443 { "https" } else { "http" };

            // Omit standard ports (80 for http, 443 for https)
            if (scheme == "http" && self.port == 80) || (scheme == "https" && self.port == 443) {
                format!("{}://{}", scheme, host)
            } else {
                format!("{}://{}:{}", scheme, host, self.port)
            }
        }
    }

    /// Every base URL this engine answers to: the primary one first, then any
    /// configured extras, with duplicates removed.
    pub fn all_base_urls(&self) -> Vec<String> {
        let mut urls = vec![self.get_base_url()];
        for extra in &self.additional_base_urls {
            if !urls.iter().any(|u| u == extra) {
                urls.push(extra.clone());
            }
        }
        urls
    }

    /// `management_hosts` in the form request Host headers take, with entries
    /// that name no host dropped (configuration validation rejects those, so
    /// this only skips them if validation was bypassed).
    pub fn normalized_management_hosts(&self) -> Vec<String> {
        self.management_hosts
            .iter()
            .filter_map(|entry| normalize_host_entry(entry))
            .collect()
    }
}

/// Normalize a configured host to the form a request's Host header takes.
///
/// Accepts either a bare `host[:port]` or a full base URL, so the host
/// settings can be written in whichever style reads better. Returns `None`
/// when the entry names no host or carries a path or credentials.
pub fn normalize_host_entry(entry: &str) -> Option<String> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    if entry.contains("://") {
        return base_url_authority(entry);
    }
    if entry.contains('/') || entry.contains('@') {
        return None;
    }
    Some(entry.to_lowercase())
}

/// Extract the `Host` header form of a base URL: lowercase hostname, plus the
/// port when it is not the scheme's default (browsers omit default ports, and
/// so does `Url::port`). Returns `None` for a URL without a host.
pub fn base_url_authority(base_url: &str) -> Option<String> {
    let url = url::Url::parse(base_url).ok()?;
    let host = url.host_str()?.to_lowercase();
    match url.port() {
        Some(port) => Some(format!("{}:{}", host, port)),
        None => Some(host),
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            format: "pretty".to_string(),
        }
    }
}

impl Default for JavaScriptConfig {
    fn default() -> Self {
        Self {
            execution_timeout_ms: 5000,
            max_memory_bytes: 10 * 1024 * 1024, // 10MB
            max_concurrent_executions: 100,
            stack_size_bytes: DEFAULT_STACK_SIZE_BYTES,
            enable_init_functions: true,
            init_timeout_ms: None,     // Use execution_timeout_ms by default
            test_timeout_ms: None,     // Use execution_timeout_ms by default
            test_run_timeout_ms: None, // Use DEFAULT_TEST_RUN_TIMEOUT_MS
            fail_startup_on_init_error: false,
        }
    }
}

impl Default for RepositoryConfig {
    fn default() -> Self {
        Self {
            connection_string: "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine"
                .to_string(),
            embedded: false,
            embedded_data_dir: default_embedded_data_dir(),
            embedded_port: 0,
            max_script_size_bytes: 1024 * 1024,      // 1MB
            max_upload_size_bytes: 10 * 1024 * 1024, // 10MB
            max_connections: default_db_max_connections(),
            lock_timeout_ms: default_lock_timeout_ms(),
            statement_timeout_ms: default_statement_timeout_ms(),
            idle_in_transaction_timeout_ms: default_idle_in_transaction_timeout_ms(),
        }
    }
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            enable_cors: true,
            // Same-origin only until an operator names somebody. The default
            // used to be `["*"]`, which was harmless while nothing read it and
            // would have become "any origin may read /engine/*" the moment
            // something did.
            cors_allowed_origins: Vec::new(),
            enable_security_headers: true,
            content_security_policy: Some(
                "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'".to_string()
            ),
            max_request_body_bytes: 1024 * 1024, // 1MB
            csrf_key: None,
            strict_ip_validation: false,
            session_encryption_key: None,
            secret_encryption_key: None,
            api_key: None,
        }
    }
}

impl AppConfig {
    /// Load configuration from multiple sources with precedence:
    /// 1. Command line arguments
    /// 2. Environment variables (use double underscore __ for nesting)
    /// 3. Config file (TOML, YAML, JSON5, etc.)
    /// 4. Default values
    pub fn load() -> Result<Self, anyhow::Error> {
        use tracing::debug;

        // Check if config.toml exists
        let config_toml_exists = std::path::Path::new("config.toml").exists();
        eprintln!("config.toml exists: {}", config_toml_exists);

        // Log environment variables for debugging
        debug!("Loading configuration...");
        for (key, value) in std::env::vars() {
            if key.starts_with("APP_") {
                // Sanitize database URLs in logs
                let safe_value = if key.contains("DATABASE") || key.contains("CONNECTION") {
                    if let Some(at_pos) = value.find('@') {
                        let before_at = &value[..at_pos];
                        let after_at = &value[at_pos..];
                        if let Some(colon_pos) = before_at.rfind(':') {
                            format!("{}:****{}", &before_at[..colon_pos], after_at)
                        } else {
                            value.clone()
                        }
                    } else {
                        value.clone()
                    }
                } else if key.contains("SECRET")
                    || key.contains("CSRF")
                    || key.contains("SESSION_ENCRYPTION")
                    || key.contains("ENCRYPTION_KEY")
                {
                    "****".to_string()
                } else {
                    value.clone()
                };
                eprintln!("Found env var: {} = {}", key, safe_value);
            }
        }

        let figment = Figment::new()
            .merge(Serialized::defaults(Self::default()))
            .merge(Toml::file("config.toml"));

        // Debug: print what we have so far
        if config_toml_exists && let Ok(_partial_config) = figment.clone().extract::<Self>() {
            eprintln!("After loading config.toml - connection_string: (***hidden***)");
        }

        let config: Self = figment
            .merge(Yaml::file("config.yaml"))
            .merge(Yaml::file("config.yml"))
            .merge(Env::prefixed("APP_").split("__"))
            .extract()?;

        eprintln!("Final config - connection_string: (***hidden***)");

        // Validate the configuration
        config.validate()?;

        Ok(config)
    }

    /// Create a test configuration with a specific port
    /// Test configuration with specified port
    /// Uses PostgreSQL with default test database settings
    pub fn test_config_with_port(port: u16) -> Self {
        let mut config = Self::default();
        config.server.port = port;
        config
    }

    /// Create a test configuration using PostgreSQL
    /// Uses DATABASE_URL env var when set, otherwise falls back to the default hardcoded URL.
    /// Requires a running database.
    pub fn test_config_postgres(port: u16) -> Self {
        let mut config = Self::test_config_with_port(port);
        if let Ok(url) = std::env::var("DATABASE_URL") {
            config.repository.connection_string = url;
        }
        config
    }

    /// Load configuration from a specific file
    pub fn load_from_file<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let figment = Figment::new().merge(Serialized::defaults(AppConfig::default()));

        let figment = if path.extension() == Some(std::ffi::OsStr::new("toml")) {
            figment.merge(Toml::file(path))
        } else if path.extension() == Some(std::ffi::OsStr::new("yaml"))
            || path.extension() == Some(std::ffi::OsStr::new("yml"))
        {
            figment.merge(Yaml::file(path))
        } else {
            return Err(anyhow::anyhow!(
                "Unsupported config file format: {:?}",
                path
            ));
        };

        let config: AppConfig = figment
            .merge(Env::prefixed("APP_").split("__"))
            .extract()
            .context("Failed to load configuration from file")?;

        config.validate()?;
        Ok(config)
    }

    /// Validate configuration values
    pub fn validate(&self) -> Result<()> {
        // Validate server configuration
        if self.server.port == 0 {
            anyhow::bail!("Server port cannot be 0");
        }

        // Port range is validated by u16 type, no need to check upper bound

        // Refused at startup rather than dropped: an unparseable entry would
        // silently leave the proxy untrusted, and the symptom of that is one
        // rate-limit bucket shared by everyone rather than an error.
        if let Err(reason) = crate::security::TrustedProxies::parse(&self.server.trusted_proxies) {
            anyhow::bail!("Invalid server.trusted_proxies entry: {}", reason);
        }

        // Every additional base URL must name a host we can match a request's
        // Host header against, otherwise per-host login cannot be wired up.
        for extra in &self.server.additional_base_urls {
            if base_url_authority(extra).is_none() {
                anyhow::bail!(
                    "Invalid server.additional_base_urls entry '{}': must be an absolute URL with a host, e.g. https://manage.example.com",
                    extra
                );
            }
        }

        // A management host that never matches a Host header would silently
        // take the management APIs offline, so reject it at startup instead.
        for entry in &self.server.management_hosts {
            if normalize_host_entry(entry).is_none() {
                anyhow::bail!(
                    "Invalid server.management_hosts entry '{}': must be a hostname, \
                     optionally with a port, e.g. manage.example.com or localhost:3000",
                    entry
                );
            }
        }

        // A build without the feature has no embedded server to start, and
        // falling back to `database_url` would connect to whatever the
        // placeholder in the shipped config happens to name. Refused here so
        // `--validate-config` reports it too.
        if self.repository.embedded && !crate::embedded_db::SUPPORTED {
            anyhow::bail!(
                "repository.embedded is set, but this build has no embedded database. \
                 Rebuild with --features embedded-postgres-bundled, or set \
                 repository.embedded = false and point database_url at a PostgreSQL server."
            );
        }

        if self.repository.embedded && self.repository.embedded_data_dir.trim().is_empty() {
            anyhow::bail!(
                "repository.embedded_data_dir cannot be empty when repository.embedded is set"
            );
        }

        // Validate logging configuration
        match self.logging.level.as_str() {
            "trace" | "debug" | "info" | "warn" | "error" => {}
            _ => anyhow::bail!(
                "Invalid log level: {}. Must be one of: trace, debug, info, warn, error",
                self.logging.level
            ),
        }

        match self.logging.format.as_str() {
            "json" | "pretty" | "compact" => {}
            _ => anyhow::bail!(
                "Invalid log format: {}. Must be one of: json, pretty, compact",
                self.logging.format
            ),
        }

        // Validate JavaScript configuration
        if self.javascript.execution_timeout_ms == 0 {
            anyhow::bail!("JavaScript execution timeout must be > 0");
        }

        if self.javascript.max_memory_bytes == 0 {
            anyhow::bail!("JavaScript max memory must be > 0");
        }

        if self.javascript.max_concurrent_executions == 0 {
            anyhow::bail!("JavaScript max concurrent executions must be > 0");
        }

        // A stack too small to run anything fails every script with a stack
        // overflow rather than with something that names the cause, so the
        // floor is checked here where it can say so.
        if self.javascript.stack_size_bytes < MIN_STACK_SIZE_BYTES {
            anyhow::bail!(
                "JavaScript stack size must be at least {} bytes",
                MIN_STACK_SIZE_BYTES
            );
        }

        if self.javascript.test_timeout_ms == Some(0) {
            anyhow::bail!("JavaScript test timeout must be > 0");
        }

        if self.javascript.test_run_timeout_ms == Some(0) {
            anyhow::bail!("JavaScript test run timeout must be > 0");
        }

        // PostgreSQL is the only supported storage backend - no validation needed
        // Connection string is required and already enforced by type system
        if self.repository.max_connections == 0 {
            anyhow::bail!("Database max connections must be > 0");
        }

        // Validate security configuration
        // Note: rate_limit_per_minute of 0 means disabled, which is allowed

        if self.security.max_request_body_bytes == 0 {
            anyhow::bail!("Max request body size must be > 0");
        }

        // With authentication enabled, the security keys must be explicitly
        // provisioned. Falling back to random per-boot keys invalidates
        // sessions and CSRF tokens across restarts and instances, and a missing
        // secret encryption key stores secrets as plaintext in the database.
        //
        // This used to be waived in development mode. There is no development
        // mode now, so a local install provides keys like any other — the
        // shipped local template carries throwaway ones, marked as such.
        if let Some(auth) = &self.auth
            && auth.enabled
        {
            let key_missing =
                |key: &Option<String>| key.as_deref().map(str::trim).unwrap_or("").is_empty();

            // The three keys below are checked for being absent; this one is
            // checked for being the value everybody has. It passes the length
            // rule, ships in `config.local.toml`, and a template copied forward
            // into something internet-facing therefore starts and signs tokens
            // with a secret published in the repository.
            if let Some(placeholder) = placeholder_secret(&auth.jwt_secret) {
                anyhow::bail!(
                    "auth.jwt_secret is {} rather than a secret of your own. Set \
                     APP_AUTH__JWT_SECRET. Generate with: openssl rand -base64 32",
                    placeholder
                );
            }

            if key_missing(&self.security.csrf_key) {
                anyhow::bail!(
                    "security.csrf_key (APP_SECURITY__CSRF_KEY) is required when \
                     authentication is enabled. Generate with: openssl rand -base64 32"
                );
            }

            if key_missing(&self.security.session_encryption_key) {
                anyhow::bail!(
                    "security.session_encryption_key (APP_SECURITY__SESSION_ENCRYPTION_KEY) \
                     is required when authentication is enabled. \
                     Generate with: openssl rand -base64 32"
                );
            }

            if key_missing(&self.security.secret_encryption_key) {
                anyhow::bail!(
                    "security.secret_encryption_key (APP_SECURITY__SECRET_ENCRYPTION_KEY) is \
                     required when authentication is enabled (secrets would otherwise be \
                     stored as plaintext). Generate with: openssl rand -base64 32"
                );
            }
        }

        Ok(())
    }

    /// Get server socket address
    pub fn server_address(&self) -> Result<SocketAddr> {
        format!("{}:{}", self.server.host, self.server.port)
            .parse()
            .context("Invalid server address")
    }

    /// Get JavaScript execution timeout as Duration
    pub fn js_execution_timeout(&self) -> Duration {
        Duration::from_millis(self.javascript.execution_timeout_ms)
    }
}

// Keep backward compatibility with the old Config struct
pub type Config = AppConfig;

impl AppConfig {
    /// Backward compatibility method - equivalent to load()
    pub fn from_env() -> Self {
        match Self::load() {
            Ok(config) => {
                // Debug: log if auth is configured
                if config.auth.is_some() {
                    eprintln!("DEBUG: Auth configuration loaded successfully");
                } else {
                    eprintln!("DEBUG: No auth configuration found in loaded config");
                }
                config
            }
            Err(e) => {
                eprintln!("DEBUG: Failed to load config: {}. Using defaults.", e);
                Self::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base_url_authority_omits_default_ports() {
        assert_eq!(
            base_url_authority("https://Manage.Softagen.com"),
            Some("manage.softagen.com".to_string())
        );
        assert_eq!(
            base_url_authority("https://softagen.com:443/ignored"),
            Some("softagen.com".to_string())
        );
        assert_eq!(
            base_url_authority("http://localhost:3000"),
            Some("localhost:3000".to_string())
        );
        assert_eq!(base_url_authority("not-a-url"), None);
    }

    #[test]
    fn test_all_base_urls_dedupes_and_leads_with_primary() {
        let mut server = ServerConfig {
            base_url: Some("https://softagen.com".to_string()),
            ..ServerConfig::default()
        };
        server.additional_base_urls = vec![
            "https://manage.softagen.com".to_string(),
            "https://softagen.com".to_string(),
        ];

        assert_eq!(
            server.all_base_urls(),
            vec!["https://softagen.com", "https://manage.softagen.com"]
        );
    }

    #[test]
    fn test_normalize_host_entry_accepts_bare_hosts_and_urls() {
        assert_eq!(
            normalize_host_entry("Manage.Softagen.com"),
            Some("manage.softagen.com".to_string())
        );
        assert_eq!(
            normalize_host_entry("https://manage.softagen.com"),
            Some("manage.softagen.com".to_string())
        );
        assert_eq!(
            normalize_host_entry("localhost:3000"),
            Some("localhost:3000".to_string())
        );
        assert_eq!(
            normalize_host_entry("  manage.softagen.com  "),
            Some("manage.softagen.com".to_string())
        );
    }

    #[test]
    fn test_normalize_host_entry_rejects_non_hosts() {
        // A path or credentials would never match a Host header.
        assert_eq!(normalize_host_entry("manage.softagen.com/engine"), None);
        assert_eq!(normalize_host_entry("user@manage.softagen.com"), None);
        assert_eq!(normalize_host_entry(""), None);
        assert_eq!(normalize_host_entry("   "), None);
    }

    #[test]
    fn test_normalized_management_hosts_uses_host_header_form() {
        let server = ServerConfig {
            management_hosts: vec![
                "https://manage.softagen.com".to_string(),
                "Localhost:3000".to_string(),
            ],
            ..ServerConfig::default()
        };

        assert_eq!(
            server.normalized_management_hosts(),
            vec!["manage.softagen.com", "localhost:3000"]
        );
    }

    #[test]
    fn test_validation_rejects_unmatchable_management_host() {
        let mut config = AppConfig::default();
        config.server.management_hosts = vec!["manage.softagen.com/engine".to_string()];

        // Accepted silently, this would take the management APIs offline.
        let err = config
            .validate()
            .expect_err("a bare entry with a path can never match a Host header");
        assert!(err.to_string().contains("management_hosts"));
    }

    #[test]
    fn test_validation_accepts_full_url_management_host() {
        // A full URL is forgiving input: its path is irrelevant once the entry
        // is reduced to the authority a Host header carries.
        let mut config = AppConfig::default();
        config.server.management_hosts = vec!["https://manage.softagen.com/ignored".to_string()];

        assert!(config.validate().is_ok());
        assert_eq!(
            config.server.normalized_management_hosts(),
            vec!["manage.softagen.com"]
        );
    }

    #[test]
    fn test_validation_rejects_hostless_additional_base_url() {
        let mut config = AppConfig::default();
        config.server.additional_base_urls = vec!["manage.softagen.com".to_string()];

        let err = config
            .validate()
            .expect_err("a URL without a scheme has no host to match on");
        assert!(err.to_string().contains("additional_base_urls"));
    }

    #[test]
    fn test_default_config() {
        let config = AppConfig::default();
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.logging.level, "info");
        assert_eq!(config.javascript.execution_timeout_ms, 5000);
    }

    #[test]
    fn test_config_validation() {
        let mut config = AppConfig::default();

        // Valid config should pass
        assert!(config.validate().is_ok());

        // Invalid port should fail
        config.server.port = 0;
        assert!(config.validate().is_err());

        // Reset and test invalid log level
        config = AppConfig::default();
        config.logging.level = "invalid".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_auth_requires_security_keys() {
        let mut config = AppConfig::test_config_with_port(8080);
        config.auth = Some(crate::auth::AuthConfig {
            enabled: true,
            jwt_secret: "a-jwt-secret-of-at-least-32-characters".to_string(),
            ..Default::default()
        });

        // Authentication with no keys must be rejected. There is no longer a
        // mode that waives this.
        assert!(config.validate().is_err());

        // Empty strings (e.g. from `${VAR:-}` docker-compose defaults) count
        // as missing
        config.security.csrf_key = Some("".to_string());
        config.security.session_encryption_key = Some("".to_string());
        config.security.secret_encryption_key = Some("".to_string());
        assert!(config.validate().is_err());

        // All keys provisioned passes
        config.security.csrf_key = Some("a".repeat(44));
        config.security.session_encryption_key = Some("b".repeat(44));
        config.security.secret_encryption_key = Some("c".repeat(44));
        assert!(config.validate().is_ok());

        // Auth disabled does not require the keys: with no authentication
        // there are no sessions to encrypt and every caller is anonymous.
        config.security.csrf_key = None;
        config.security.session_encryption_key = None;
        config.security.secret_encryption_key = None;
        config.auth = None;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_server_address() {
        let config = AppConfig::default();
        let addr = config.server_address().unwrap();
        assert_eq!(addr.to_string(), "127.0.0.1:8080");
    }

    #[test]
    fn test_timeout_conversions() {
        let config = AppConfig::default();
        assert_eq!(config.js_execution_timeout(), Duration::from_millis(5000));
    }

    #[test]
    fn test_environment_variable_override() {
        // Test that environment loading doesn't panic
        let config = AppConfig::from_env();
        assert!(config.server.port > 0);

        // This would test actual loading, but we need to be careful in tests
        // as it affects other tests. In a real test, you'd use a separate process
        // or mock the environment.
    }

    #[test]
    fn test_validation_edge_cases() {
        let mut config = AppConfig::default();

        // Test port edge cases
        config.server.port = 65535;
        assert!(config.validate().is_ok());

        config.server.port = 0;
        assert!(config.validate().is_err());

        // Reset for memory tests - test zero memory (which should fail)
        config = AppConfig::default();
        config.javascript.max_memory_bytes = 0;
        assert!(config.validate().is_err());

        // Test timeout edge cases - only zero timeout should fail
        config = AppConfig::default();
        config.javascript.execution_timeout_ms = 1;
        assert!(config.validate().is_ok());

        config.javascript.execution_timeout_ms = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_database_pool_size_defaults_and_validation() {
        let mut config = AppConfig::default();
        assert_eq!(config.repository.max_connections, 20);
        assert!(config.validate().is_ok());

        config.repository.max_connections = 0;
        assert!(
            config.validate().is_err(),
            "a pool of zero connections can serve nothing"
        );
    }

    #[test]
    fn test_backward_compatibility() {
        let config = AppConfig::default();

        // Test backward compatibility methods
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.javascript.execution_timeout_ms, 5000);
        assert_eq!(config.javascript.max_concurrent_executions, 100); // Correct default value
        assert_eq!(
            config.server_address().unwrap().to_string(),
            "127.0.0.1:8080"
        );
    }

    #[test]
    fn test_config_file_loading() {
        // Test that loading with missing files doesn't crash
        let result = AppConfig::load();
        // May fail due to missing config files or figment features, but should not panic
        // The important thing is it doesn't panic
        // Config loaded successfully or loading failed, which is acceptable in test environment
        let _ = result.is_ok();
    }

    #[test]
    fn test_test_config_helper() {
        let config = AppConfig::test_config_with_port(3000);
        assert_eq!(config.server.port, 3000);
        assert_eq!(config.server.host, "127.0.0.1"); // Should keep other defaults
    }

    #[test]
    fn test_security_validation() {
        let mut config = AppConfig::default();

        // Test CORS origins validation
        config.security.cors_allowed_origins = vec!["invalid-origin".to_string()];
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_database_type_validation() {
        let config = AppConfig::default();

        // PostgreSQL is the only supported storage backend
        // Connection string is required by type system
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_log_level_validation() {
        let mut config = AppConfig::default();

        // Test all valid log levels
        for level in &["trace", "debug", "info", "warn", "error"] {
            config.logging.level = level.to_string();
            assert!(
                config.validate().is_ok(),
                "Log level {} should be valid",
                level
            );
        }

        // Test invalid log level
        config.logging.level = "invalid".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_duration_helpers() {
        let config = AppConfig::default();

        assert_eq!(config.js_execution_timeout().as_millis(), 5000);
    }

    #[test]
    fn test_logs_config_defaults() {
        let config = AppConfig::default();

        assert!(config.logs.prune_enabled);
        assert_eq!(config.logs.retention_hours, 24);
        assert_eq!(config.logs.keep_per_script, 100);
        assert_eq!(config.logs.prune_interval_secs, 3600);
    }

    /// A configuration written before `[logs]` existed still loads, and gets
    /// the policy a new one would.
    #[test]
    fn test_logs_config_defaults_when_section_absent() {
        let rendered =
            toml::to_string(&AppConfig::default()).expect("default config should serialize");

        // Drop the [logs] table, leaving a file shaped like one written before
        // the section existed.
        let mut without_logs = String::new();
        let mut skipping = false;
        for line in rendered.lines() {
            if line.starts_with('[') {
                skipping = line.starts_with("[logs]");
            }
            if !skipping {
                without_logs.push_str(line);
                without_logs.push('\n');
            }
        }
        assert!(!without_logs.contains("[logs]"));

        let config: AppConfig =
            toml::from_str(&without_logs).expect("config without a [logs] section should load");

        assert!(config.logs.prune_enabled);
        assert_eq!(config.logs.retention_hours, 24);
        assert_eq!(config.logs.keep_per_script, 100);
    }
}
