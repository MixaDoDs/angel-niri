#[macro_use]
extern crate tracing;

#[cfg(feature = "dbus")]
pub mod a11y;
pub mod animation;
pub mod backend;
pub mod cast_only;
pub mod cast_pause;
pub mod cli;
pub mod cursor;
#[cfg(feature = "dbus")]
pub mod dbus;
pub mod frame_clock;
pub mod game;
pub mod handlers;
pub mod inactive_dim;
pub mod input;
pub mod ipc;
pub mod layer;
pub mod layout;
pub mod minimize;
pub mod niri;
pub mod pixel_corners;
pub mod pixel_overview;
pub mod pixel_shadow;
pub mod privacy_look;
pub mod protocols;
pub mod remember;
pub mod render_helpers;
pub mod rubber_band;
pub mod scene;
#[cfg(feature = "xdp-gnome-screencast")]
pub mod screencasting;
pub mod ui;
pub mod utils;
pub mod window;

#[cfg(test)]
mod tests;
