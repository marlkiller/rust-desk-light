use std::sync::atomic::{AtomicBool, AtomicU64};

pub(crate) struct DesktopStreamState {
    pub(crate) running: AtomicBool,
    pub(crate) generation: AtomicU64,
}
