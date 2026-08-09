//! Long-running fixture process for the sandbox integration test.
//!
//! The test spawns this binary under the OS sandbox and asserts it survives
//! past exec; a missing allowlist entry would fail the exec immediately.

fn main() {
    std::thread::sleep(std::time::Duration::from_mins(1));
}
