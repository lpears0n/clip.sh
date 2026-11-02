# Packaging

- `aur/PKGBUILD` — Arch package built from a release tag. Bump `pkgver`, run
  `updpkgsums`, test with `makepkg -si`, then push to the AUR as `clip-sh`.
- `clip-sh.desktop` — the launcher entry the PKGBUILD installs; Tauri's own
  bundlers (AppImage / deb / rpm, built by `.github/workflows/release.yml`)
  generate their own from `tauri.conf.json`.

Flatpak is deliberately not here yet: GStreamer PipeWire capture needs the
`--socket=pipewire` sandbox hole and the KDE window-capture path uses a
private KWin protocol, so a Flathub submission should wait for the portal
path to be the default. See `docs/ROADMAP.md`.
