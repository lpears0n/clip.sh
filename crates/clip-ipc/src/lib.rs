//! The clip.sh control socket: one JSON request per line, one JSON response
//! per line. The app listens at [`socket_path`]; `clipsh` and the app itself
//! (to find an already-running instance) connect to it.

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    time::Duration,
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum Request {
    /// Save the replay buffer as a clip.
    Save,
    /// Recorder status.
    Status,
    /// Bring the window to the front.
    Show,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    /// Human-readable result or error.
    pub message: String,
    /// Saved clip, for `save`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Full status, for `status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<serde_json::Value>,
}

impl Response {
    pub fn ok(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: message.into(),
            ..Default::default()
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: message.into(),
            ..Default::default()
        }
    }
}

/// `$XDG_RUNTIME_DIR/clipsh.sock` (per user, cleaned up at logout).
pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
            std::env::temp_dir().join(format!("clipsh-{user}"))
        });
    dir.join("clipsh.sock")
}

/// Send one request to the running app and wait for its answer. Saving can
/// take a few seconds (muxing the clip), so the read timeout is generous.
pub fn request(req: &Request) -> std::io::Result<Response> {
    let mut sock = UnixStream::connect(socket_path())?;
    sock.set_read_timeout(Some(Duration::from_secs(60)))?;
    sock.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    sock.write_all(line.as_bytes())?;
    let mut answer = String::new();
    BufReader::new(sock).read_line(&mut answer)?;
    Ok(serde_json::from_str(&answer)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_format() {
        assert_eq!(
            serde_json::to_string(&Request::Save).unwrap(),
            r#"{"cmd":"save"}"#
        );
        let r: Request = serde_json::from_str(r#"{"cmd":"status"}"#).unwrap();
        assert_eq!(r, Request::Status);
        let resp = Response::ok("saved");
        assert_eq!(
            serde_json::to_string(&resp).unwrap(),
            r#"{"ok":true,"message":"saved"}"#
        );
    }
}
