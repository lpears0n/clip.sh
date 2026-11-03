// UI smoke test: runs the real frontend in Chromium with the Tauri bridge
// mocked, drives every view, asserts a few behaviours and saves screenshots
// to tests/shots/. No Tauri, no GStreamer, no GPU needed.
//
//   pnpm test:ui            # all scenarios
//   pnpm test:ui full       # one of: full, empty, big
//
// Needs `playwright` (a devDependency) and a Chromium it can launch
// (`npx playwright install chromium` once, unless one is preinstalled).

const { chromium } = require('playwright')
const { spawn } = require('node:child_process')
const path = require('node:path')
const fs = require('node:fs')
const assert = require('node:assert/strict')

const PORT = 5173
const out = path.join(__dirname, 'shots')
fs.mkdirSync(out, { recursive: true })

// ---------------------------------------------------------------- fixtures

function fixtures(scenario) {
  const now = Date.now()
  const h = 3600e3
  const mult = scenario === 'big' ? 40 : 1
  const games = [
    game('ELDEN RING', 1245620, 7, { last: now - 2 * h, total: 14 * h, playing: true, sessions: [{ start: now - 3 * h, end: now - 2 * h }] }),
    game('Counter-Strike 2', 730, 12, { last: now - 26 * h, total: 40 * h }),
    game('Hades II', 1145350, 3, { last: now - 5 * 24 * h, total: 6 * h }),
  ]
  const clips = []
  let i = 0
  for (const g of games) {
    for (let k = 0; k < g.clip_count * mult; k++) {
      const t = now - (i * 7 + 1) * h
      clips.push({
        path: `/home/u/Videos/clip.sh/${g.name}/clip-${i}.mp4`, game: g.name, name: `clip-${i}`, created_ms: t,
        size_bytes: 60e6 + i * 9e6, duration: 20 + (i % 5) * 11, audio_tracks: 2, is_edit: i % 6 === 0,
        steam_app_id: g.steam_app_id, thumbnail: null, sprite: null, sprite_frames: 10,
        title: i === 1 ? 'Malenia no-hit (finally)' : null, favorite: i % 4 === 1,
        labels: i % 3 === 1 ? ['clutch', 'boss'] : i === 4 ? ['funny'] : [],
      })
      i++
    }
  }
  const empty = scenario === 'empty'
  return {
    get_status: empty
      ? { state: 'waiting', game: null, steam_app_id: null, session_start_ms: null, buffered_seconds: 0, replay_seconds: 30, encoder: null, capture_path: null, error: null, backend: 'portal', needs_screen: true }
      : { state: 'recording', game: 'ELDEN RING', steam_app_id: 1245620, session_start_ms: now - h, buffered_seconds: 30, replay_seconds: 30, encoder: 'nvenc', capture_path: 'zerocopy', error: null, backend: 'kwin', needs_screen: false },
    get_settings: {
      replay_seconds: 30, mic: true, clips_dir: '/home/u/Videos/clip.sh', hotkey: 'F8', screenshot_hotkey: 'F9', capture_backend: null,
      launch_at_login: false, start_in_tray: true, close_to_tray: true, notify_on_save: true, sound_on_save: true, overlay: true,
      fps: 60, bitrate_kbps: 20000, codec: 'h264', encoder: null, cursor: true, zero_copy: true, desktop_audio: true, desktop_device: 'out-headset',
      mic_device: null, audio_bitrate_kbps: 160, storage_limit_gb: 25, protect_edits: true, use_trash: true, ads: false,
    },
    list_clips: empty ? [] : clips,
    list_games: empty ? [] : games,
    get_storage: { clips_bytes: 3.7e9, clip_count: clips.length, disk_free: 120e9, disk_total: 500e9, limit_bytes: 25e9 },
    get_capabilities: { display_hz: 144, encoders: [{ encoder: 'nvenc', codecs: ['h264', 'hevc', 'av1'] }, { encoder: 'x264', codecs: ['h264'] }], mic_devices: [{ id: 'mic1', name: 'Blue Yeti' }], output_devices: [{ id: 'out-speakers', name: 'Speakers (Realtek Audio)' }, { id: 'out-headset', name: 'Headset Earphone (Arctis Nova 7)' }] },
    media_base: 'http://127.0.0.1:1/x/',
    hotkey_backend: 'kglobalaccel',
  }
}

function game(name, id, clips, o) {
  return {
    name, steam_app_id: id, art: { capsule: null, hero: null, logo: null }, fallback_image: null,
    last_played_ms: o.last, last_activity_ms: o.last, total_play_ms: o.total, clip_count: clips, clip_bytes: clips * 1.5e8,
    sessions: (o.sessions ?? []).map((s) => ({ game: name, steam_app_id: id, start_ms: s.start, end_ms: s.end })),
    playing: !!o.playing,
  }
}

// ------------------------------------------------------------- tauri mock

// Installed before the page's scripts run; `@tauri-apps/api` talks to this.
function mockBridge(data) {
  let n = 0
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main', windowLabel: 'main' } },
    transformCallback: (cb) => { const id = ++n; window[`_${id}`] = cb; return id },
    convertFileSrc: (p) => 'asset://' + p,
    invoke: async (cmd, args) => {
      if (cmd === 'open_for_edit') return data.list_clips.find((c) => c.path === args.path) || { ...data.list_clips[0], path: args.path }
      if (cmd === 'update_clip') {
        const c = data.list_clips.find((c) => c.path === args.path)
        Object.assign(c, {
          title: args.patch.title === undefined ? c.title : args.patch.title.trim() || null,
          favorite: args.patch.favorite ?? c.favorite,
          labels: args.patch.labels ?? c.labels,
        })
        return c
      }
      if (cmd === 'delete_clip') { data.list_clips = data.list_clips.filter((c) => c.path !== args.path); return null }
      if (cmd === 'plugin:event|listen') {
        // Remember the handler so a test can fire the event at the page.
        (window.__listeners ||= {})[args.event] = args.handler
        return args.handler
      }
      if (cmd.startsWith('plugin:')) return 1
      if (cmd in data) return data[cmd]
      return null
    },
  }
}

// ---------------------------------------------------------------- scenarios

async function runScenario(scenario, browser) {
  const data = fixtures(scenario)
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 }, colorScheme: 'dark' })
  page.on('pageerror', (e) => { throw new Error(`page error in ${scenario}: ${e.message}`) })
  await page.addInitScript(mockBridge, data)
  await page.goto(`http://localhost:${PORT}/`)
  await page.waitForTimeout(800)
  const shot = (name) => page.screenshot({ path: path.join(out, `${scenario}-${name}.png`) })
  const cards = () => page.locator('.clip-grid > *').count()

  await shot('games')
  if (scenario === 'empty') {
    await page.getByRole('button', { name: 'Choose screen' }).waitFor()
    await page.close()
    return
  }

  await page.getByRole('button', { name: 'All clips' }).click()
  await page.waitForTimeout(300)

  if (scenario === 'big') {
    await page.mouse.move(700, 400)
    for (let i = 0; i < 30; i++) { await page.mouse.wheel(0, 600); await page.waitForTimeout(16) }
    await page.waitForTimeout(300)
    await shot('scrolled')
    const n = await cards()
    const height = await page.evaluate(() => document.querySelector('.clip-grid').scrollHeight)
    assert.ok(n < 60, `virtual grid should window ${data.list_clips.length} clips, rendered ${n}`)
    assert.ok(height < 200_000, `grid height ${height}px is runaway`)
    await page.close()
    return
  }

  await shot('clips')
  const before = await cards()

  // Selection, bulk delete, undo.
  await page.getByRole('button', { name: 'Select', exact: true }).click()
  await page.getByRole('button', { name: /^Select .* clip/ }).nth(1).click()
  await page.getByRole('button', { name: /^Select .* clip/ }).nth(3).click()
  await shot('clips-select')
  await page.getByRole('button', { name: 'Delete', exact: true }).click()
  await page.waitForTimeout(200)
  assert.equal(await cards(), before - 2, 'deleted clips vanish at once')
  await shot('clips-undo')
  await page.getByRole('button', { name: 'Undo' }).click()
  await page.waitForTimeout(200)
  assert.equal(await cards(), before, 'undo brings the clips back')

  // Favourites filter.
  await page.getByRole('button', { name: 'Only favourites' }).click()
  await page.waitForTimeout(200)
  assert.ok((await cards()) < before, 'favourites filter narrows the grid')
  await shot('clips-favourites')
  await page.getByRole('button', { name: 'Only favourites' }).click()

  // Viewer: rename, star, label, shortcuts, trim.
  await page.getByRole('button', { name: /Open ELDEN RING clip/ }).nth(1).click()
  await page.waitForTimeout(400)
  await shot('viewer')
  await page.keyboard.press('n')
  await page.keyboard.type('Malenia no-hit, take 2')
  await page.keyboard.press('Enter')
  await page.getByRole('button', { name: 'Label', exact: true }).click()
  await page.keyboard.type('pb')
  await page.keyboard.press('Enter')
  await page.keyboard.press('Escape')
  await page.waitForTimeout(200)
  await page.keyboard.press('s')
  await page.waitForTimeout(200)
  await shot('viewer-meta')
  assert.equal(await page.locator('.chip', { hasText: 'pb' }).count(), 1, 'label added')
  await page.keyboard.press('?')
  await page.waitForTimeout(150)
  await shot('viewer-help')
  await page.keyboard.press('Escape')
  await page.keyboard.press('t')
  await page.waitForTimeout(150)
  await shot('viewer-trim')
  await page.keyboard.press('Escape')
  await page.keyboard.press('Escape')

  // Settings and appearance.
  await page.getByRole('button', { name: 'Settings 3' }).click()
  await page.getByRole('button', { name: 'Appearance' }).click()
  await page.waitForTimeout(150)
  await shot('settings-appearance')
  await page.getByRole('radio', { name: 'Blue' }).click()
  await page.getByRole('button', { name: 'Recording' }).click()
  await page.waitForTimeout(150)
  await shot('settings-recording-blue')
  await page.getByRole('button', { name: 'Audio' }).click()
  await page.waitForTimeout(150)
  assert.equal(await page.getByLabel('Output device').inputValue(), 'out-headset', 'chosen output is selected')
  assert.equal(await page.getByLabel('Output device').locator('option').count(), 3, 'default + two outputs')
  await shot('settings-audio')
  await page.getByRole('button', { name: 'Hotkeys' }).click()
  await page.waitForTimeout(150)
  assert.equal(await page.locator('kbd', { hasText: /^F9$/ }).count(), 1, 'screenshot hotkey row')
  await shot('settings-hotkeys')
  await page.getByRole('button', { name: 'Appearance' }).click()
  await page.getByRole('radio', { name: 'Orange' }).click()

  // Sidebar collapse.
  await page.keyboard.press('1')
  await page.keyboard.press('Control+b')
  await page.waitForTimeout(300)
  await shot('sidebar-collapsed')
  await page.keyboard.press('Control+b')
  await page.close()
}

// ------------------------------------------------------------------ runner

// The in-game popup page: fire the events the backend sends and check what
// it draws. The window is transparent, so it's shot over a stand-in game.
async function overlayScenario(browser) {
  const page = await browser.newPage({ viewport: { width: 380, height: 72 }, colorScheme: 'dark' })
  page.on('pageerror', (e) => { throw new Error(`page error in overlay: ${e.message}`) })
  await page.addInitScript(mockBridge, fixtures('full'))
  await page.goto(`http://localhost:${PORT}/overlay.html`)
  await page.waitForFunction(() => window.__listeners && window.__listeners['overlay-show'])
  await page.addStyleTag({ content: 'html { background: linear-gradient(135deg, #3a4a5a, #6b5a3a) !important }' })
  const fire = (payload) =>
    page.evaluate((p) => window[`_${window.__listeners['overlay-show']}`]({ event: 'overlay-show', id: 1, payload: p }), payload)

  await fire({ kind: 'ready', title: 'Ready to record', detail: "Tom Clancy's Rainbow Six Siege", key: 'F8', ms: 3500 })
  await page.waitForTimeout(300)
  assert.equal(await page.locator('#title').textContent(), 'Ready to record')
  assert.ok((await page.locator('#detail').textContent()).startsWith('Press F8 to clip'))
  assert.ok(await page.locator('.dot').isVisible(), 'recording dot on ready')
  await page.screenshot({ path: path.join(out, 'overlay-ready.png') })

  await fire({ kind: 'saved', title: 'Clip saved', detail: "Tom Clancy's Rainbow Six Siege", key: null, ms: 600 })
  await page.waitForTimeout(250)
  assert.equal(await page.locator('#title').textContent(), 'Clip saved')
  assert.ok(await page.locator('.check').isVisible(), 'check mark on saved')
  assert.ok(!(await page.locator('#detail').textContent()).includes('Press'), 'no key hint on saved')
  await page.screenshot({ path: path.join(out, 'overlay-saved.png') })
  // Fades out on its own before the backend hides the window.
  await page.waitForTimeout(700)
  assert.equal(await page.locator('#popup').evaluate((e) => e.classList.contains('in')), false, 'fades out')
  await page.close()
}

async function waitForServer() {
  for (let i = 0; i < 100; i++) {
    try {
      const r = await fetch(`http://localhost:${PORT}/`)
      if (r.ok) return
    } catch {}
    await new Promise((r) => setTimeout(r, 200))
  }
  throw new Error('dev server did not start')
}

;(async () => {
  const wanted = process.argv[2] ? [process.argv[2]] : ['full', 'empty', 'big']
  let server = null
  let ownServer = false
  try {
    await fetch(`http://localhost:${PORT}/`)
  } catch {
    server = spawn('pnpm', ['dev'], { cwd: path.join(__dirname, '..'), stdio: 'ignore' })
    ownServer = true
  }
  // A preinstalled Chromium can be pointed at with CHROMIUM_PATH when the
  // pinned Playwright version hasn't downloaded its own.
  const executablePath = process.env.CHROMIUM_PATH || (fs.existsSync('/opt/pw-browsers/chromium') ? '/opt/pw-browsers/chromium' : undefined)
  const browser = await chromium.launch({ executablePath })
  try {
    await waitForServer()
    for (const s of wanted) {
      await runScenario(s, browser)
      console.log(`ok  ${s}`)
    }
    if (!process.argv[2]) {
      await overlayScenario(browser)
      console.log('ok  overlay')
    }
  } finally {
    await browser.close()
    if (ownServer && server) server.kill()
  }
})().catch((e) => {
  console.error(e)
  process.exit(1)
})
