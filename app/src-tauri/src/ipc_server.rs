//! The control socket (`rerunsctl save` etc.), and single-instance handling: a
//! second launch finds the socket, asks the running app to show itself, and
//! exits.

use clip_ipc::{Request, Response, socket_path};
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use crate::AppState;

/// Linux: the socket, and the lock that makes it ours.
#[cfg(target_os = "linux")]
pub struct Listener {
    socket: tokio::net::UnixListener,
    /// Held for the life of the app; the kernel drops it when the process
    /// exits, crash included.
    lock: std::fs::File,
}
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
///
/// Linux: an exclusive lock on `reruns.lock` beside the socket is the claim.
/// Taking it is atomic, so two launches at the same instant can't both
/// win, and only its holder may replace the socket file, and only when
/// nothing answers on it (a stale one left by a crash, never a live one,
/// even one owned by an older Reruns that doesn't take the lock).
#[cfg(target_os = "linux")]
pub fn claim() -> anyhow::Result<Claim> {
    use std::os::unix::fs::PermissionsExt;

    let path = socket_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let lock_path = path.with_extension("lock");
    let mut last = None;
    // Retried briefly: the holder binds its socket right after taking the
    // lock, and an instance that is quitting lets go of it.
    for _ in 0..20 {
        if let Some(lock) = try_lock(&lock_path)? {
            // A Reruns from before this lock owns the socket without it:
            // hand off to it rather than take its socket away.
            if clip_ipc::request(&Request::Show).is_ok() {
                return Ok(Claim::Running);
            }
            // Nobody answers: the socket file is left over from a crash.
            let _ = std::fs::remove_file(&path);
            let socket =
                tauri::async_runtime::block_on(async { tokio::net::UnixListener::bind(&path) })?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            return Ok(Claim::Ours(Listener { socket, lock }));
        }
        match clip_ipc::request(&Request::Show) {
            Ok(_) => return Ok(Claim::Running),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    anyhow::bail!(
        "another Reruns holds the lock but doesn't answer: {}",
        last.map(|e| e.to_string()).unwrap_or_default()
    )
}

/// Take an exclusive lock on `path` (created if missing) without waiting:
/// `None` while another process holds it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn try_lock(path: &std::path::Path) -> std::io::Result<Option<std::fs::File>> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
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
    let Listener { socket, lock } = listener;
    tauri::async_runtime::spawn(async move {
        // The task runs for the life of the app, and so holds the lock.
        let _lock = lock;
        loop {
            match socket.accept().await {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_process_holds_the_lock() {
        let dir = std::env::temp_dir().join(format!("reruns-lock-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("reruns.lock");
        let held = try_lock(&path).unwrap().expect("a free lock is taken");
        // A second launch (another open of the file) finds it held.
        assert!(try_lock(&path).unwrap().is_none(), "the lock is exclusive");
        // Released when its holder goes away (as at exit, or a crash).
        drop(held);
        assert!(
            try_lock(&path).unwrap().is_some(),
            "free again once dropped"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
