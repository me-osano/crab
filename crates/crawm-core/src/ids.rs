//! Opaque identifiers for windows, outputs, and layer surfaces.
//!
//! Identifiers are allocated by the compositor and never exposed to clients
//! directly. They are cheap to copy and stable for the lifetime of the object.

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $name(u64);

        impl $name {
            /// Wraps a raw identifier value.
            #[must_use]
            pub const fn new(raw: u64) -> Self {
                Self(raw)
            }

            /// Returns the raw value.
            #[must_use]
            pub const fn raw(self) -> u64 {
                self.0
            }
        }

        impl core::fmt::Display for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<u64> for $name {
            fn from(raw: u64) -> Self {
                Self(raw)
            }
        }
    };
}

define_id! {
    /// Handle for a managed toplevel window.
    WindowId
}

define_id! {
    /// Handle for an output (monitor / connector).
    OutputId
}

define_id! {
    /// Handle for a layer-shell surface (bar, wallpaper, notification, …).
    LayerId
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_raw() {
        let id = WindowId::new(7);
        assert_eq!(id.raw(), 7);
        assert_eq!(WindowId::from(7u64), id);
        assert_eq!(id.to_string(), "7");
    }
}
