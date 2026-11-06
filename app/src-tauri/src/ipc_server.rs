//! The control socket (`rerunsctl save` etc.), and single-instance handling: a
//! second launch finds the socket, asks the running app to show itself, and
//! exits.

use clip_ipc::{Request, Response, socket_path};
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use crate::AppState;

#[cfg(target_os = "linux")]
pub type Listener = tokio::net::UnixListener;
#[cfg(windows)]
pub type Listener = tokio::net::windows::named_pipe::NamedPipeServer;

/// Who has the control socket.
pub enum Claim {
    /// This launch: it is the only instance. Pass the listener to [`serve`].
    Ours(Listener),
    /// Another instance, which has been asked to show its window.
    Running,
}

/// Take the control socket, or hand off to the instance that has it. Runs
/// first thing at start-up, before anything slow: the socket is what marks
/// an instance as running, so a second launch during this one's start-up
/// (seconds, while GStreamer loads) must already find it taken.
#[cfg(target_os = "linux")]
pub fn claim() -> anyhow::Result<Claim> {
    use std::os::unix::fs::PermissionsExt;

    if clip_ipc::request(&Request::Show).is_ok() {
        return Ok(Claim::Running);
    }
    let path = socket_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // No live instance answered, so any file here is stale.
    let _ = std::fs::remove_file(&path);
    let listener = tauri::async_runtime::block_on(async { Listener::bind(&path) })?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(Claim::Ours(listener))
}

/// Windows: the first instance of the named pipe is the claim. Creating it
/// fails with access denied while another instance holds it; that instance
/// may still be starting, and the request then waits until it serves.
#[cfg(windows)]
pub fn claim() -> anyhow::Result<Claim> {
    use tokio::net::windows::named_pipe::ServerOptions;
    const ERROR_ACCESS_DENIED: i32 = 5;

    let name = socket_path().to_string_lossy().into_owned();
    let mut last = None;
    // Retried briefly: between two connections the pipe can be busy, and an
    // instance that is quitting lets go of it.
    for _ in 0..20 {
        // Tokio's pipe needs its reactor, and start-up runs outside the runtime.
        match tauri::async_runtime::block_on(async {
            ServerOptions::new().first_pipe_instance(true).create(&name)
        }) {
            Ok(server) => return Ok(Claim::Ours(server)),
            Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) => {
                match clip_ipc::request(&Request::Show) {
                    Ok(_) => return Ok(Claim::Running),
                    Err(e) => last = Some(e),
                }
            }
            Err(e) => return Err(e.into()),
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    anyhow::bail!(
        "the control pipe is held but doesn't answer: {}",
        last.map(|e| e.to_string()).unwrap_or_default()
    )
}

/// Listen on the claimed control socket for the life of the app.
#[cfg(target_os = "linux")]
pub fn serve(app: AppHandle, listener: Listener) {
    tracing::info!(path = %socket_path().display(), "control socket ready");
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
}

/// Named pipe `\\.\pipe\reruns-<user>`; one instance per connection, as
/// Windows pipes work. A client that connected while the app was starting
/// is answered on the first `connect`.
#[cfg(windows)]
pub fn serve(app: AppHandle, mut server: Listener) {
    use tokio::net::windows::named_pipe::ServerOptions;

    let name = socket_path().to_string_lossy().into_owned();
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
        Request::Mark => match state.service.mark().await {
            Ok(at) => Response::ok(format!("marked {at:.1}s into the replay")),
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

fn saved(result: Result<std::path::PathBuf, String>) -> Response {
    match result {
        Ok(path) => Response {
            path: Some(path.display().to_string()),
            ..Response::ok(format!("saved {}", path.display()))
        },
        Err(e) => Response::err(e),
    }
}
