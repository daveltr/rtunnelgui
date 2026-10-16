//! Per-OS child-process handling: hidden console / job object (Windows), parent-death signal (Linux).
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use unix::*;
#[cfg(windows)]
pub use windows::*;
