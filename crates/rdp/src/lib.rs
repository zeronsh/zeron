//! Local RDP client. No GUI, engine, cloud, or native clipboard dependencies.
//! Creating configuration never starts a connection; a session has one owner.

mod model;
pub use model::*;

mod graphics;
mod session;
mod tls;
pub use session::connect;

mod input;

mod resize;

pub mod clipboard;

mod display_control;
