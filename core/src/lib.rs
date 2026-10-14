//! Core of the GR III look editor, compiled to WebAssembly for the browser.
//! Everything runs here, on the user's machine: the firmware container, the
//! look data, the camera's tone curve and the DNG. It started as a port of a
//! Python editor (removed; see the git history), whose answers the tests
//! still check against.

pub mod addr;
pub mod arm;
pub mod chan;
pub mod curvesel;
pub mod dng;
pub mod fit;
pub mod gamma;
pub mod grfw;
pub mod looks;
pub mod multiaxial;
pub mod session;
pub mod space;
pub mod tone;
pub mod tonecontrol;
pub mod wasm;
