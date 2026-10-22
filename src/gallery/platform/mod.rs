//! What the library and editor need from the operating system.

#[cfg(not(windows))]
mod unix;
#[cfg(windows)]
mod win;

#[cfg(not(windows))]
pub use unix::*;
#[cfg(windows)]
pub use win::*;
