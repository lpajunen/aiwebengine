//! Build script for capturing build metadata at compile time
//!
//! This script uses vergen to capture git commit information and build timestamps
//! that will be embedded in the binary. If git is unavailable (e.g., building from
//! a source tarball), empty strings will be used as fallbacks.

use vergen::EmitBuilder;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure vergen to emit git and build information
    // If git is not available, vergen will set empty values
    EmitBuilder::builder()
        .git_sha(true)          // Git commit hash (short form)
        .git_commit_timestamp() // Git commit timestamp (ISO 8601)
        .build_timestamp()      // Build timestamp (ISO 8601)
        .emit()?;

    Ok(())
}
