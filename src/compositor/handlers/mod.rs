//! Protocol handlers, one module per protocol family (`COMPOSITOR.md` §5).
//!
//! Each submodule implements the matching Smithay trait for [`Crab`] and
//! invokes the `delegate_*!` macro, which is what registers a protocol's
//! globals and dispatch. Keeping them separate from `state.rs` means that file
//! stays *state*, and adding a protocol (M3's layer shell, idle, screencopy, …)
//! is one new file rather than another arm in a mega-`impl`.
//!
//! [`Crab`]: crate::compositor::state::Crab

mod compositor;
mod data_device;
mod fractional_scale;
mod layer_shell;
mod output;
mod seat;
mod shm;
mod viewporter;
mod xdg_activation;
mod xdg_decoration;
mod xdg_shell;
