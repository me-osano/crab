//! `wl_data_device` — the clipboard selection and drag-and-drop.

use std::os::unix::io::OwnedFd;

use smithay::delegate_data_device;
use smithay::input::Seat;
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::selection::data_device::{
    ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
};

use crate::compositor::state::Crawm;

impl SelectionHandler for Crawm {
    type SelectionUserData = ();
}

impl DataDeviceHandler for Crawm {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for Crawm {}

impl ServerDndGrabHandler for Crawm {
    fn send(&mut self, _mime_type: String, _fd: OwnedFd, _seat: Seat<Self>) {
        // TODO(M6): wire compositor-initiated drag-and-drop.
    }
}

delegate_data_device!(Crawm);
