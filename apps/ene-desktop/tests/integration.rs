//! Consolidated integration-test harness.
//!
//! The desktop dependency graph is expensive to link on Windows. One harness
//! keeps that cost single while nextest retains per-test process isolation.

#[path = "stage7_b.rs"]
mod stage7_b;
#[path = "stage7_c1.rs"]
mod stage7_c1;
#[path = "stage7_c2.rs"]
mod stage7_c2;
#[path = "stage7_c3.rs"]
mod stage7_c3;
#[path = "stage7_e.rs"]
mod stage7_e;
