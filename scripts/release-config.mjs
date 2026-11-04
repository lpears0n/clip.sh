// Writes the release-only Tauri config, from what the repository's release
// secrets and variables switch on, and points the build at it:
//
// - UPDATER_PUBKEY (variable) + TAURI_SIGNING_PRIVATE_KEY (secret): the app
//   can update itself; the build signs its bundles and writes latest.json.
// - WINDOWS_SIGN_COMMAND (variable): Windows files are signed by running
//   this command on each (`%1` is the file), e.g. Azure Trusted Signing.
// - WINDOWS_CERT_THUMBPRINT (set by the certificate import step from the
//   WINDOWS_CERTIFICATE secret): signed with that certificate instead.
//
// With none of them set it writes nothing, and the build is the same as a
// development build. Usage: node scripts/release-config.mjs [out.json]
import { appendFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'

const env = process.env
const out = resolve(process.argv[2] ?? 'app/src-tauri/release.conf.json')
const config = {}

if (env.UPDATER_PUBKEY?.trim() && env.TAURI_SIGNING_PRIVATE_KEY?.trim()) {
  const repo = env.GITHUB_REPOSITORY ?? 'lpears0n/reruns'
  config.bundle = { createUpdaterArtifacts: true }
  config.plugins = {
    updater: {
      pubkey: env.UPDATER_PUBKEY.trim(),
      endpoints: [`https://github.com/${repo}/releases/latest/download/latest.json`],
      // The installer shows its progress but asks nothing.
      windows: { installMode: 'passive' },
    },
  }
  console.log('updates: on')
} else if (env.UPDATER_PUBKEY?.trim() || env.TAURI_SIGNING_PRIVATE_KEY?.trim()) {
  console.log('updates: off (needs both UPDATER_PUBKEY and TAURI_SIGNING_PRIVATE_KEY)')
} else {
  console.log('updates: off')
}

if (env.WINDOWS_SIGN_COMMAND?.trim()) {
  config.bundle ??= {}
  config.bundle.windows = { signCommand: env.WINDOWS_SIGN_COMMAND.trim() }
  console.log('windows signing: on (sign command)')
} else if (env.WINDOWS_CERT_THUMBPRINT?.trim()) {
  config.bundle ??= {}
  config.bundle.windows = {
    certificateThumbprint: env.WINDOWS_CERT_THUMBPRINT.trim(),
    digestAlgorithm: 'sha256',
    timestampUrl: env.WINDOWS_TIMESTAMP_URL?.trim() || 'http://timestamp.digicert.com',
  }
  console.log('windows signing: on (certificate)')
} else {
  console.log('windows signing: off')
}

if (Object.keys(config).length) {
  writeFileSync(out, JSON.stringify(config, null, 2) + '\n')
  console.log(`wrote ${out}`)
  if (env.GITHUB_ENV) appendFileSync(env.GITHUB_ENV, `RELEASE_CONFIG_ARGS=--config ${out}\n`)
}
