#![expect(clippy::unwrap_used, reason = "tests fail fast")]
#![expect(
    unsafe_code,
    reason = "set_var plants a secret so the child env filter can be asserted"
)]

use ene_registry::BuiltinExecutor;
use serde_json::json;
use tempfile::TempDir;

#[test]
fn planted_secret_env_is_not_visible_to_child() {
    let dir = TempDir::new().unwrap();
    // SAFETY: this integration test owns process env for the duration of the call.
    unsafe {
        std::env::set_var("ENE_WORKSPACE", dir.path());
        std::env::set_var("OPENAI_API_KEY", "sk-test-secret");
    }
    let value = BuiltinExecutor
        .execute(
            "exec.run",
            &json!({
                "command": "python3",
                "args": ["-c", "import os; print('OPENAI_API_KEY' in os.environ)"],
                "timeout_ms": 5000
            }),
        )
        .unwrap();
    // SAFETY: restore process env after the child has exited.
    unsafe {
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("ENE_WORKSPACE");
    }
    assert_eq!(value["stdout"].as_str().unwrap_or("").trim(), "False");
}
