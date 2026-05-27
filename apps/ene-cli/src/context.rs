use ene_core::EneRuntime;
use std::ops::{Deref, DerefMut};

pub struct AppContext {
    pub runtime: EneRuntime,
}

impl Deref for AppContext {
    type Target = EneRuntime;
    fn deref(&self) -> &Self::Target {
        &self.runtime
    }
}

impl DerefMut for AppContext {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.runtime
    }
}
