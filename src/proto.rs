//! Private client protocol: length-prefixed (u32 LE) JSON messages over a
//! unix socket. ponytail: JSON while the protocol is young — postcard when
//! bandwidth matters (the frame payload is already pre-diffed ANSI bytes).

use std::io::{Read, Write};

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
const MAX_MSG: u32 = 16 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    Hello { version: u32, cols: u16, rows: u16 },
    /// Parsed host-terminal input, forwarded verbatim.
    Event(crossterm::event::Event),
    Detach,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome { version: u32 },
    /// Pre-diffed ANSI bytes — the client writes them to its stdout as-is.
    Frame(Vec<u8>),
    /// Detach this client; the server keeps running.
    Detach,
    /// The server is shutting down.
    Shutdown,
}

/// Socket path for the session (named via CDOCK_SESSION, default "default").
pub fn socket_path() -> Option<std::path::PathBuf> {
    let name = std::env::var("CDOCK_SESSION").unwrap_or_else(|_| "default".to_string());
    crate::logging::state_dir().map(|d| d.join(format!("session-{name}.sock")))
}

// --- sync framing (client side) ---

pub fn write_msg<T: Serialize>(w: &mut impl Write, msg: &T) -> std::io::Result<()> {
    let body = serde_json::to_vec(msg)?;
    w.write_all(&(body.len() as u32).to_le_bytes())?;
    w.write_all(&body)?;
    w.flush()
}

pub fn read_msg<T: for<'de> Deserialize<'de>>(r: &mut impl Read) -> std::io::Result<T> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len);
    if len > MAX_MSG {
        return Err(std::io::Error::other("message too large"));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(std::io::Error::other)
}

// --- async framing (server side) ---

pub async fn write_msg_async<T: Serialize>(
    w: &mut (impl tokio::io::AsyncWrite + Unpin),
    msg: &T,
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let body = serde_json::to_vec(msg)?;
    w.write_all(&(body.len() as u32).to_le_bytes()).await?;
    w.write_all(&body).await?;
    w.flush().await
}

pub async fn read_msg_async<T: for<'de> Deserialize<'de>>(
    r: &mut (impl tokio::io::AsyncRead + Unpin),
) -> std::io::Result<T> {
    use tokio::io::AsyncReadExt;
    let len = r.read_u32_le().await?;
    if len > MAX_MSG {
        return Err(std::io::Error::other("message too large"));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body).await?;
    serde_json::from_slice(&body).map_err(std::io::Error::other)
}
