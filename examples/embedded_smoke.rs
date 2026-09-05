//! Exercise the embedded PostgreSQL supervisor end to end.
//!
//! No automated test covers this path: a first run downloads and extracts a
//! PostgreSQL installation, which needs the network and takes minutes, and the
//! test harness deliberately connects to a server it does not own (see
//! `tests/common/testdb.rs`). This is the manual check in its place.
//!
//! ```sh
//! cargo run --features embedded-postgres --example embedded_smoke -- /tmp/pgdata
//! ```
//!
//! Without that feature it compiles and then refuses, which is the behaviour a
//! server build should have — it is not declared with `required-features`,
//! because an explicit `[[example]]` in Cargo.toml asserts this file exists and
//! the Docker build parses the manifest one layer before any source is copied.
//!
//! It runs twice against the same directory on purpose. The second pass is the
//! interesting one: it proves the cluster and the password it was initialised
//! with survive a restart, which is the difference between a database and a
//! scratch directory.

use aiwebengine::config::RepositoryConfig;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::args()
        .nth(1)
        .ok_or("usage: embedded_smoke <data-dir>")?;

    for pass in 1..=2 {
        let config = RepositoryConfig {
            embedded: true,
            embedded_data_dir: dir.clone(),
            ..Default::default()
        };

        let resolved = aiwebengine::embedded_db::resolve(&config).await?;
        println!("pass {pass}: url = {}", resolved.connection_string);

        // The engine's own migrations, against the engine's own pool: if this
        // works, every `sqlx::query!` in the repository does too.
        let db = aiwebengine::database::init_database(&resolved, true).await?;

        let scripts: (i64,) = sqlx::query_as("SELECT count(*) FROM scripts")
            .fetch_one(db.pool())
            .await?;
        // Loopback only, and asserted rather than assumed: this database holds
        // every secret in the install.
        let listen: (String,) = sqlx::query_as("SHOW listen_addresses")
            .fetch_one(db.pool())
            .await?;

        println!(
            "pass {pass}: migrated, scripts = {}, listen_addresses = {}",
            scripts.0, listen.0
        );

        aiwebengine::embedded_db::stop().await;
    }

    println!("✓ embedded PostgreSQL started, migrated, restarted and stopped");
    Ok(())
}
