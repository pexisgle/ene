use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

#[derive(Zeroize, ZeroizeOnDrop)]
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

    #[must_use]
    pub fn take(&mut self) -> Zeroizing<String> {
        let mut value = String::new();
        core::mem::swap(&mut value, &mut self.buffer);
        self.buffer.zeroize();
        Zeroizing::new(value)
    }

    pub fn cancel(&mut self) {
        self.buffer.zeroize();
        self.buffer.clear();
    }
}
