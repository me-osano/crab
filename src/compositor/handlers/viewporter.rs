//! `wp_viewporter` — surface cropping/scaling.

use smithay::delegate_viewporter;

use crate::compositor::state::Crab;

delegate_viewporter!(Crab);
