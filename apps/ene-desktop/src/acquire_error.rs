#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquireError {
    Reconfigure,
    Timeout,
    Fatal,
}
