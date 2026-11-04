//! The control socket (`clipsh save` etc.), and single-instance handling: a
//! second launch finds the socket, asks the running app to show itself, and
//! exits.

use clip_ipc::{Request, Response, socket_path};
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use crate::AppState;

/// If clip.sh is already running, bring its window up and return true.
pub fn hand_off_to_running_instance() -> bool {
    // An error means no socket, or a stale one from a crash: we're the only
    // instance.
    clip_ipc::request(&Request::Show).is_ok()
}

/// Listen on the control socket for the life of the app.
#[cfg(target_os = "linux")]
pub fn serve(app: AppHandle) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use tokio::net::UnixListener;

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

/// Named pipe `\\.\pipe\clipsh-<user>`; one instance per connection, as
/// Windows pipes work.
#[cfg(windows)]
pub fn serve(app: AppHandle) -> anyhow::Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let name = socket_path().to_string_lossy().into_owned();
    // Tokio's pipe needs its reactor, and setup runs outside the runtime.
    let mut server = tauri::async_runtime::block_on(async {
        ServerOptions::new().first_pipe_instance(true).create(&name)
    })?;
    tracing::info!(pipe = %name, "control pipe ready");
    tauri::async_runtime::spawn(async move {
        loop {
            if let Err(e) = server.connect().await {
                tracing::warn!("control pipe connect: {e}");
                continue;
            }
            let conn = server;
            server = match ServerOptions::new().create(&name) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("control pipe: {e}");
                    return;
                }
            };
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = handle(app, conn).await {
                    tracing::debug!("control pipe: {e:#}");
                }
            });
        }
    });
    Ok(())
}

async fn handle<S: AsyncRead + AsyncWrite + Unpin>(app: AppHandle, sock: S) -> anyhow::Result<()> {
    let (read, mut write) = tokio::io::split(sock);
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
        Request::Save => saved(state.service.save(state.clips_dir()).await),
        Request::Screenshot => saved(state.service.screenshot(state.clips_dir()).await),
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

fn saved(result: Result<std::path::PathBuf, String>) -> Response {
    match result {
        Ok(path) => Response {
            path: Some(path.display().to_string()),
            ..Response::ok(format!("saved {}", path.display()))
        },
        Err(e) => Response::err(e),
    }
}
