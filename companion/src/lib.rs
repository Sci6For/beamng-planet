#[cfg(windows)]
pub mod backend;
pub mod config;
#[cfg(windows)]
pub mod game;
#[cfg(windows)]
pub mod updates;
#[cfg(windows)]
pub mod job;
pub mod lifecycle;
pub mod protocol;
#[cfg(windows)] pub mod audio;
#[cfg(windows)]
pub mod runtime;

#[cfg(test)]
mod tests;

pub mod lan_protocol;
pub mod lan_lifecycle;
#[cfg(windows)] pub mod lan_runtime;
