//! `wl_output` — advertising the nested window as a client-visible output.

use smithay::delegate_output;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::wayland::output::OutputHandler;

use crate::compositor::state::Crab;

impl OutputHandler for Crab {
    fn output_bound(&mut self, _output: Output, _wl_output: WlOutput) {}
}

delegate_output!(Crab);
