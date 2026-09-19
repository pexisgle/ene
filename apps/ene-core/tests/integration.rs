//! Consolidated integration-test harness.
//!
//! Keeping these suites in one crate avoids repeatedly linking the full Host
//! dependency graph while nextest still schedules each `#[test]` separately.

#[path = "stage5_e2e.rs"]
mod stage5_e2e;
#[path = "stage5_windows_pipe_e2e.rs"]
mod stage5_windows_pipe_e2e;
#[path = "stage6_e2e.rs"]
mod stage6_e2e;
#[path = "stage7_a1.rs"]
mod stage7_a1;
#[path = "vertical_slice.rs"]
mod vertical_slice;
