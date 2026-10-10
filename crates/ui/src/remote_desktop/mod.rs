//! Native, local remote desktops. Sessions are owned by sidebar tabs.
pub mod credentials;
pub mod desktop;
pub mod input;
pub mod profiles;
mod surface;
mod view;
pub use surface::{RemoteDesktopSurface, SurfaceEvent};
pub mod cursor;
