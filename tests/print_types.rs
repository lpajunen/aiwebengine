//! `--print-types`: the type definitions script repositories check against.

use std::process::Command;

/// With no `APP_*` variables at all, so no keys: what a script repository's CI
/// has when it runs the image.
#[test]
fn prints_the_served_declarations_without_keys() {
    let output = Command::new(env!("CARGO_BIN_EXE_aiwebengine"))
        .arg("--print-types")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env_clear()
        .output()
        .expect("the engine binary should run");

    assert!(
        output.status.success(),
        "--print-types failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let printed = String::from_utf8(output.stdout).expect("the declarations are UTF-8");
    // Nothing before or after them: the output is redirected into a file.
    let written = aiwebengine::engine_types::TYPE_DEFINITIONS;
    assert_eq!(printed.lines().next(), written.lines().next());
    assert_eq!(
        printed.trim_end().lines().last(),
        written.trim_end().lines().last()
    );
    assert!(
        !printed.contains("{{limits.") && !printed.contains("{{engine."),
        "a placeholder was left unrendered"
    );
    assert!(
        printed.contains("  \"list_scripts\": {"),
        "the operation table should be rendered into EngineOperations"
    );
}
