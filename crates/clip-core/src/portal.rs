//! Screen selection through the xdg-desktop-portal ScreenCast interface.

use std::os::fd::{AsRawFd, OwnedFd, RawFd};

use anyhow::{Context, Result};
use ashpd::desktop::{
    PersistMode, Session,
    screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType},
};

use crate::capture::PipewireTarget;

/// A live portal screencast: the PipeWire remote fd and the node to read from.
///
/// The portal session stays open for as long as this value is alive.
pub struct ScreenSource {
    fd: OwnedFd,
    node_id: u32,
    size: Option<(i32, i32)>,
    restore_token: Option<String>,
    session: Session<Screencast>,
}

impl ScreenSource {
    /// Ask the portal for a monitor or window to capture.
    ///
    /// Passing the `restore_token` from an earlier session skips the picker
    /// dialog on compositors that support persistence (KDE, GNOME).
    pub async fn open(restore_token: Option<&str>) -> Result<Self> {
        Self::open_sources(restore_token, SourceType::Monitor | SourceType::Window).await
    }

    /// Like [`ScreenSource::open`], but offers monitors only. A monitor choice
    /// can be restored across launches; a game's window is new every time.
    pub async fn open_monitor(restore_token: Option<&str>) -> Result<Self> {
        Self::open_sources(restore_token, SourceType::Monitor.into()).await
    }

    async fn open_sources(
        restore_token: Option<&str>,
        sources: ashpd::enumflags2::BitFlags<SourceType>,
    ) -> Result<Self> {
        let proxy = Screencast::new()
            .await
            .context("ScreenCast portal unavailable; is xdg-desktop-portal running?")?;
        let session = proxy.create_session(Default::default()).await?;
        proxy
            .select_sources(
                &session,
                SelectSourcesOptions::default()
                    .set_cursor_mode(CursorMode::Embedded)
                    .set_sources(sources)
                    .set_multiple(false)
                    .set_restore_token(restore_token)
                    .set_persist_mode(PersistMode::ExplicitlyRevoked),
            )
            .await?;

        let response = proxy
            .start(&session, None, Default::default())
            .await?
            .response()
            .context("screen selection was cancelled")?;
        let stream = response
            .streams()
            .first()
            .context("portal returned no streams")?
            .to_owned();
        let fd = proxy
            .open_pipe_wire_remote(&session, Default::default())
            .await?;

        Ok(Self {
            fd,
            node_id: stream.pipe_wire_node_id(),
            size: stream.size(),
            restore_token: response.restore_token().map(str::to_owned),
            session,
        })
    }

    pub fn target(&self) -> PipewireTarget {
        PipewireTarget::Remote {
            fd: self.raw_fd(),
            node_id: self.node_id,
        }
    }

    pub fn raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    pub fn node_id(&self) -> u32 {
        self.node_id
    }

    /// Stream size reported by the portal, if any.
    pub fn size(&self) -> Option<(i32, i32)> {
        self.size
    }

    /// Token to pass to [`ScreenSource::open`] next time to skip the picker.
    pub fn restore_token(&self) -> Option<&str> {
        self.restore_token.as_deref()
    }

    pub async fn close(self) -> Result<()> {
        self.session.close().await?;
        Ok(())
    }
}
