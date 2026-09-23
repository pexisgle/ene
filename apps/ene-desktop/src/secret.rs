//! C1 secret intake: not chat, not undo, not a draft, not Targeted Deletion.
//!
//! The buffer is zeroized on take, cancel, and drop. It is never cloned into
//! GUI snapshots, logs, or saved state.

use zeroize::{Zeroize, ZeroizeOnDrop};

/// Owner-typed secret held only while the credential field is open.
#[derive(Zeroize, ZeroizeOnDrop, Default)]
pub struct SecretIntake {
    buffer: String,
}

impl core::fmt::Debug for SecretIntake {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("SecretIntake([redacted])")
    }
}

impl SecretIntake {
    #[must_use]
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    pub fn set(&mut self, value: String) {
        self.buffer.zeroize();
        self.buffer = value;
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Moves the buffer out for control intake and zeroizes this slot.
    #[must_use]
    pub fn take(&mut self) -> String {
        let mut value = String::new();
        core::mem::swap(&mut value, &mut self.buffer);
        self.buffer.zeroize();
        value
    }

    pub fn cancel(&mut self) {
        self.buffer.zeroize();
        self.buffer.clear();
    }
}
