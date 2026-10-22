//! `xdg_activation_v1` — focus requests.
//!
//! Only a recent token that carries a seat serial is honored, and only for a
//! window on the space currently in view. Anything else is ignored: the
//! protocol deliberately leaves the policy to the compositor, and a token a
//! client minted for itself must not be able to steal focus.

use std::time::Duration;

use smithay::delegate_xdg_activation;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::xdg_activation::{
    XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
};

use crate::compositor::state::Crab;

/// How long an activation token stays usable.
const TOKEN_LIFETIME: Duration = Duration::from_secs(10);

impl XdgActivationHandler for Crab {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    fn token_created(&mut self, _token: XdgActivationToken, _data: XdgActivationTokenData) -> bool {
        // The policy is applied when the token is used, where the requesting
        // surface is known too.
        true
    }

    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        // Tokens are single-use, whether or not the request is honored.
        let _ = self.xdg_activation_state.remove_token(&token);

        if token_data.timestamp.elapsed() > TOKEN_LIFETIME {
            tracing::debug!("ignored a stale activation token");
            return;
        }
        if token_data.serial.is_none() {
            tracing::debug!("ignored an activation token without a seat serial");
            return;
        }
        let Some(id) = self.window_id_for(&surface) else {
            tracing::debug!("ignored an activation for an unknown surface");
            return;
        };
        let visible = self.output_state.view.visible;
        if !self
            .window(id)
            .is_some_and(|window| window.window.is_on(visible))
        {
            tracing::debug!(?id, "ignored an activation on a hidden space");
            return;
        }
        self.focus_window(Some(id));
    }
}

delegate_xdg_activation!(Crab);
