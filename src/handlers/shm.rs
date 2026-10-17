//! `wl_shm` — shared-memory buffers, what most clients draw into.

use smithay::delegate_shm;
use smithay::wayland::shm::{ShmHandler, ShmState};

use crate::state::Crawm;

impl ShmHandler for Crawm {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_shm!(Crawm);
