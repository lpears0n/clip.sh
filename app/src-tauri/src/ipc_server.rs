//! The control socket (`clipsh save` etc.), and single-instance handling: a
//! second launch finds the socket, asks the running app to show itself, and
//! exits.

use std::os::unix::fs::PermissionsExt;

use clip_ipc::{Request, Response, socket_path};
use tauri::{AppHandle, Manager};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
};

use crate::AppState;

/// If clip.sh is already running, bring its window up and return true.
pub fn hand_off_to_running_instance() -> bool {
    // An error means no socket, or a stale one from a crash: we're the only
    // instance.
    clip_ipc::request(&Request::Show).is_ok()
}

/// Listen on the control socket for the life of the app.
pub fn serve(app: AppHandle) -> anyhow::Result<()> {
    let path = socket_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Only reached when no live instance answered, so any file here is stale.
    let _ = std::fs::remove_file(&path);
    let listener = tauri::async_runtime::block_on(async { UnixListener::bind(&path) })?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    tracing::info!(path = %path.display(), "control socket ready");
    tauri::async_runtime::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((sock, _)) => {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = handle(app, sock).await {
                            tracing::debug!("control socket: {e:#}");
                        }
                    });
                }
                Err(e) => tracing::warn!("control socket accept: {e}"),
            }
        }
    });
    Ok(())
}

async fn handle(app: AppHandle, sock: UnixStream) -> anyhow::Result<()> {
    let (read, mut write) = sock.into_split();
    let mut line = String::new();
    BufReader::new(read).read_line(&mut line).await?;
    let resp = match serde_json::from_str::<Request>(&line) {
        Ok(req) => respond(&app, req).await,
        Err(e) => Response::err(format!("bad request: {e}")),
    };
    let mut out = serde_json::to_string(&resp)?;
    out.push('\n');
    write.write_all(out.as_bytes()).await?;
    Ok(())
}

async fn respond(app: &AppHandle, req: Request) -> Response {
    let state = app.state::<AppState>();
    match req {
        Request::Save => match state.service.save(state.clips_dir()).await {
            Ok(path) => Response {
                path: Some(path.display().to_string()),
                ..Response::ok(format!("saved {}", path.display()))
            },
            Err(e) => Response::err(e),
        },
        Request::Status => match state.service.status().await {
            Ok(s) => Response {
                status: serde_json::to_value(&s).ok(),
                ..Response::ok(format!("{:?}", s.state).to_lowercase())
            },
            Err(e) => Response::err(e),
        },
        Request::Show => {
            crate::show_main(app);
            Response::ok("shown")
        }
    }
}
