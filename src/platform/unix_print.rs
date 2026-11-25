//! Connecting to CUPS over its socket (Linux and macOS).

use std::os::unix::net::UnixStream;
use std::path::Path;

use anyhow::{Context, Result};

use crate::print::Connection;

/// Connects to a printing service's socket.
pub fn connect(socket: &Path) -> Result<Box<dyn Connection>> {
    let s = UnixStream::connect(socket).with_context(|| format!("no printing service at {}", socket.display()))?;
    s.set_read_timeout(Some(crate::cups::TIMEOUT))?;
    s.set_write_timeout(Some(crate::cups::TIMEOUT))?;
    Ok(Box::new(s))
}
