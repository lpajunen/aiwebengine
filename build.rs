//! Build script for capturing build metadata at compile time
//!
//! This script uses vergen-gix to capture git commit information and build timestamps
//! that will be embedded in the binary. If git is unavailable (e.g., building from
//! a source tarball or in Docker without .git), it will use environment variables
//! passed during build (VERGEN_GIT_SHA, VERGEN_GIT_COMMIT_TIMESTAMP, etc.)

use std::error::Error;
use vergen_gix::{Build, Emitter, Gix};

fn main() -> Result<(), Box<dyn Error>> {
    // Check if git metadata is already provided via environment variables
    // This is the case in Docker builds where .git is not available
    let has_env_metadata = std::env::var("VERGEN_GIT_SHA").is_ok()
        || std::env::var("VERGEN_GIT_COMMIT_TIMESTAMP").is_ok();

    if has_env_metadata {
        // Environment variables already set (e.g., from Docker build args)
        // Skip vergen and let it use the existing values
        println!("cargo:warning=Using git metadata from environment variables");
        Ok(())
    } else {
        // Configure vergen-gix to emit git and build information
        // If git is not available, vergen will set empty values
        let build = Build::all_build();
        let gix = Gix::all_git();
        Emitter::default()
            .add_instructions(&build)?
            .add_instructions(&gix)?
            .emit()?;
        Ok(())
    }
}
