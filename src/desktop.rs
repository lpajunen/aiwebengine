//! Where a desktop install keeps itself, and what it writes on first launch.
//!
//! A desktop install has no operator to generate four base64 keys and no env
//! file to put them in. Until now the Makefile stood in for that — `make
//! run-desktop` wrote `.env-desktop` with `openssl rand` — which works for
//! somebody who has a checkout, a Rust toolchain and `make`, and is exactly
//! the population that does not need a desktop build. A packaged application
//! ships none of those, so the first-run path has to be in the binary.
//!
//! What it writes is an ordinary `config.toml`, loaded by the same
//! [`AppConfig::load_from_file`] every other deployment uses, in a directory
//! the operating system sets aside for application data. There is no second
//! configuration mechanism and no desktop-only code path downstream of this
//! module: the file it generates is one an administrator could have written.
//!
//! **It never regenerates.** `security.secret_encryption_key` is what every
//! script and user secret in the database is encrypted with, so a second set
//! of keys does not reset the install — it makes the install unreadable while
//! leaving it looking healthy. If the file exists, it is used as it stands,
//! whatever else has changed.

use crate::error::{AppError, AppResult};
use base64::Engine as _;
use std::path::{Path, PathBuf};

/// The directory name under the platform's application-data location.
const APP_DIR: &str = "aiwebengine";

/// Overrides the resolved directory entirely, for tests and for running two
/// installs on one machine.
const DATA_DIR_ENV: &str = "AIWEBENGINE_DATA_DIR";

/// Where a desktop install keeps its configuration and its database.
///
/// The platform's convention rather than the working directory, because a
/// packaged application's working directory is wherever the launcher happened
/// to be — `/` on macOS, `C:\Windows\System32` often enough on Windows — and
/// an install that lands there is one nobody can find, back up, or delete.
pub fn data_dir() -> AppResult<PathBuf> {
    if let Ok(explicit) = std::env::var(DATA_DIR_ENV)
        && !explicit.trim().is_empty()
    {
        return Ok(PathBuf::from(explicit));
    }

    let home = |var: &str| std::env::var(var).ok().filter(|v| !v.trim().is_empty());

    let base = if cfg!(target_os = "macos") {
        home("HOME").map(|h| PathBuf::from(h).join("Library").join("Application Support"))
    } else if cfg!(target_os = "windows") {
        home("APPDATA").map(PathBuf::from)
    } else {
        home("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
    };

    base.map(|b| b.join(APP_DIR)).ok_or_else(|| {
        AppError::config(format!(
            "Could not work out where to keep the install: no home directory in the \
             environment. Name one explicitly with {DATA_DIR_ENV}."
        ))
    })
}

/// The configuration file inside that directory.
pub fn config_path() -> AppResult<PathBuf> {
    Ok(data_dir()?.join("config.toml"))
}

/// Make sure a desktop configuration exists, and say whether this call is what
/// created it.
///
/// Creating is the whole of first-run setup: the directory, four fresh keys,
/// and a file describing a loopback install with internal accounts turned on.
pub fn ensure_config() -> AppResult<(PathBuf, bool)> {
    let dir = data_dir()?;
    let path = dir.join("config.toml");

    if path.exists() {
        return Ok((path, false));
    }

    std::fs::create_dir_all(&dir).map_err(|e| {
        AppError::config(format!(
            "Could not create the install directory {}: {e}",
            dir.display()
        ))
    })?;
    restrict_to_owner(&dir, 0o700)?;

    let rendered = render_config(&dir);

    // Written through a temporary file in the same directory and renamed, so
    // that an interrupted first run leaves either no configuration or a whole
    // one. A half-written config.toml is the one case this module must not
    // produce: it exists, so the next run will not regenerate it, and the keys
    // in it are not the keys anything was encrypted with.
    let temp = dir.join("config.toml.partial");
    std::fs::write(&temp, rendered.as_bytes())
        .map_err(|e| AppError::config(format!("Could not write {}: {e}", temp.display())))?;
    restrict_to_owner(&temp, 0o600)?;
    std::fs::rename(&temp, &path).map_err(|e| {
        AppError::config(format!(
            "Could not put the configuration in place at {}: {e}",
            path.display()
        ))
    })?;

    Ok((path, true))
}

/// Narrow a path to its owner.
///
/// A no-op off Unix, where the file inherits the user profile's ACL and there
/// is no mode to set. The configuration holds every key the install has, so
/// this is not decoration — but neither is it the only thing protecting it,
/// which is why a platform without modes is not an error.
fn restrict_to_owner(path: &Path, mode: u32) -> AppResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|e| {
            AppError::config(format!(
                "Could not narrow permissions on {}: {e}",
                path.display()
            ))
        })?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
    Ok(())
}

/// Thirty-two bytes from the system generator, base64-encoded.
///
/// The same shape as `openssl rand -base64 32`, which is what every other
/// deployment's instructions say, so a key generated here and one generated by
/// hand are indistinguishable to the code that reads them.
fn generate_key() -> String {
    let bytes: [u8; 32] = rand::random();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The configuration a desktop install starts life with.
fn render_config(dir: &Path) -> String {
    let postgres = dir.join("postgres");
    format!(
        r#"# aiwebengine — desktop install.
#
# Generated on first launch. The four keys below exist nowhere else: back this
# file up together with the database directory, and never regenerate it.
# security.secret_encryption_key is what every script and user secret in the
# database is encrypted with, so a fresh key does not reset the install, it
# makes the install unreadable.
#
# This is an ordinary configuration file. Edit it, or override any of it with
# APP_* environment variables, exactly as on a server.

[server]
# Loopback only: nothing authenticates at the network edge here, and there is
# no proxy in front. Serving this install to a network means putting a proxy
# and a certificate in front of it, and then it is a server deployment.
host = "127.0.0.1"
port = 3000
base_url = "http://localhost:3000"
# Nothing is in front, so no forwarding header is believed.
trusted_proxies = []

[repository]
# The engine starts and stops a PostgreSQL of its own. database_url is unused
# in this mode; the engine connects to the port the server came up on.
embedded = true
embedded_data_dir = "{postgres}"
# A free loopback port at each start: nothing outside this process connects.
embedded_port = 0
# One user, one machine. The cluster defaults size for hundreds of callers.
max_connections = 5

[javascript]
max_concurrent_executions = 8
max_memory_bytes = 67108864

[auth]
enabled = true
jwt_secret = "{jwt}"

[auth.cookie]
# Plain HTTP on loopback, so the cookie cannot be Secure. The engine drops the
# __Host- prefix to match, which is what keeps sign-in working here.
secure = false

[auth.internal]
# The mode internal authentication was built for: a desktop install has no
# public redirect URI, so no OAuth provider can be configured, and
# auth.bootstrap_admins — which matches a provider-verified address — can never
# name anybody.
enabled = true
allow_guests = true
# Registration is open so that the first account can be created. Turn it off
# once you have one: a desktop install has a single user.
allow_registration = true
# The username that gets the administrator role on sign-in. Register this name
# at http://localhost:3000/auth/login to claim the install.
bootstrap_admin_usernames = ["owner"]

[security]
csrf_key = "{csrf}"
session_encryption_key = "{session}"
# Losing this key makes every stored secret unreadable. It is the reason this
# file is part of the backup rather than something to regenerate.
secret_encryption_key = "{secret}"
# Same-origin only: there is one origin.
cors_allowed_origins = []
"#,
        postgres = escape_toml(&postgres.to_string_lossy()),
        jwt = generate_key(),
        csrf = generate_key(),
        session = generate_key(),
        secret = generate_key(),
    )
}

/// Escape a path for a TOML basic string.
///
/// Windows paths carry backslashes, which a basic string reads as escapes —
/// `C:\Users` becomes an invalid `\U` escape and the file fails to parse on
/// the platform it was generated for.
fn escape_toml(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A generated configuration has to be one the loader accepts. The keys are
    /// the reason: they are checked for being absent, for being a shipped
    /// placeholder, and for parsing as base64 where they are used, and a first
    /// run that produces a file failing any of those is a first run that
    /// cannot be repeated — the file exists, so nothing regenerates it.
    #[test]
    fn generated_config_loads_and_validates() {
        let dir =
            std::env::temp_dir().join(format!("aiwebengine-desktop-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("config.toml");
        std::fs::write(&path, render_config(&dir)).expect("write");

        let loaded = crate::config::AppConfig::load_from_file(&path);

        if crate::embedded_db::SUPPORTED {
            let config = loaded.expect("loads");
            assert!(config.repository.embedded);
            assert_eq!(config.server.host, "127.0.0.1");
            let auth = config.auth.as_ref().expect("auth section");
            assert!(auth.internal.enabled);
            assert_eq!(auth.internal.bootstrap_admin_usernames, vec!["owner"]);
            assert!(!auth.cookie.secure);
            config.validate().expect("validates");
        } else {
            // The default build compiles no supervisor, and `repository.embedded`
            // is refused rather than ignored — which is the answer this file
            // should get from it. Asserting the reason keeps the two halves
            // honest: a template that stopped setting `embedded` would pass a
            // test that only checked for failure.
            let error = loaded.expect_err("a build with no supervisor must refuse this");
            assert!(
                error.to_string().contains("embedded-postgres"),
                "the error should name the feature to rebuild with: {error}"
            );
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Two calls must not produce two sets of keys. Regenerating is the one
    /// failure this module exists to prevent.
    #[test]
    fn ensure_config_never_regenerates() {
        let dir = std::env::temp_dir().join(format!("aiwebengine-once-{}", rand::random::<u64>()));
        // SAFETY: single-threaded test process under nextest.
        unsafe { std::env::set_var(DATA_DIR_ENV, &dir) };

        let (path, created) = ensure_config().expect("first run");
        assert!(created, "the first call creates the configuration");
        let first = std::fs::read_to_string(&path).expect("read");

        let (again, created_again) = ensure_config().expect("second run");
        assert_eq!(path, again);
        assert!(!created_again, "the second call must not create anything");
        assert_eq!(first, std::fs::read_to_string(&again).expect("read"));

        unsafe { std::env::remove_var(DATA_DIR_ENV) };
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The keys have to differ from each other, which a copy-paste in the
    /// template would silently break — one generator call reused for all four
    /// still produces a file that loads and validates.
    #[test]
    fn the_four_keys_are_four_keys() {
        let dir = std::env::temp_dir().join("aiwebengine-keys");
        let rendered = render_config(&dir);
        let keys: Vec<&str> = rendered
            .lines()
            .filter(|l| l.contains("_key = \"") || l.starts_with("jwt_secret"))
            .collect();
        assert_eq!(keys.len(), 4, "four keys in the template: {keys:?}");
        let mut values: Vec<&str> = keys.iter().filter_map(|l| l.split('"').nth(1)).collect();
        values.sort_unstable();
        values.dedup();
        assert_eq!(values.len(), 4, "each key is generated separately");
    }
}
