use ene_runtime::EneHandle;

pub struct AppContext {
    pub handle: EneHandle,
}

impl AppContext {
    pub const fn new(handle: EneHandle) -> Self {
        Self { handle }
    }
}
