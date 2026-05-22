use ene_ai_core::AiRuntime;
use std::ops::{Deref, DerefMut};

pub struct AppContext {
    pub runtime: AiRuntime,
}

impl Deref for AppContext {
    type Target = AiRuntime;
    fn deref(&self) -> &Self::Target {
        &self.runtime
    }
}

impl DerefMut for AppContext {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.runtime
    }
}
