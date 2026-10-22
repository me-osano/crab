//! `wl_data_device` — the clipboard selection and drag-and-drop.

use std::os::unix::io::OwnedFd;

use smithay::delegate_data_device;
use smithay::input::Seat;
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::selection::data_device::{
    ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
};

use crate::compositor::state::Crab;

impl SelectionHandler for Crab {
    type SelectionUserData = ();
}

impl DataDeviceHandler for Crab {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for Crab {}

impl ServerDndGrabHandler for Crab {
    fn send(&mut self, _mime_type: String, _fd: OwnedFd, _seat: Seat<Self>) {
        // TODO(M6): wire compositor-initiated drag-and-drop.
    }
}

delegate_data_device!(Crab);
