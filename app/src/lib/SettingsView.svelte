<script lang="ts" module>
  export const categories = ['General', 'Recording', 'Audio', 'Hotkeys', 'Storage', 'Appearance'] as const
  export type Category = (typeof categories)[number]
</script>

<script lang="ts">
  import { AudioLines, Check, FolderOpen, HardDrive, Keyboard, Palette, SlidersHorizontal, Video } from '@lucide/svelte'
  import { open as openDialog } from '@tauri-apps/plugin-dialog'
  import { api, type Capabilities, type Codec, type Encoder, type Settings, type Status, type Usage } from './api'
  import { bytes } from './format'
  import HotkeyBinder from './HotkeyBinder.svelte'
  import { ACCENTS, type Accent, prefs, setPrefs } from './prefs.svelte'
  import {
    CODEC_LABEL,
    ENCODER_LABEL,
    FPS_OPTIONS,
    activePreset,
    bytesFor,
    presets,
    supportedCodecs,
  } from './quality'
  import Row from './Row.svelte'
  import Segmented from './Segmented.svelte'
  import Toggle from './Toggle.svelte'

  let {
    settings,
    usage,
    caps,
    status,
    category = $bindable(),
    onchange,
    onchoosescreen,
  }: {
    settings: Settings
    usage: Usage | null
    caps: Capabilities | null
    status: Status | null
    category: Category
    onchange: (s: Settings) => Promise<void>
    onchoosescreen: () => void
  } = $props()

  // Which hotkey service the desktop provided (loaded when Hotkeys opens).
  let hotkeyBackend = $state<'kglobalaccel' | 'portal' | null | undefined>(undefined)
  $effect(() => {
    if (category === 'Hotkeys') api.hotkeyBackend().then((b) => (hotkeyBackend = b))
  })
  let copied = $state(false)
  async function copyCommand() {
    await navigator.clipboard.writeText('clipsh save')
    copied = true
    setTimeout(() => (copied = false), 1500)
  }

  const icons = {
    General: SlidersHorizontal,
    Recording: Video,
    Audio: AudioLines,
    Hotkeys: Keyboard,
    Storage: HardDrive,
    Appearance: Palette,
  }

  const lengths = [15, 30, 60, 120, 180, 300]
  const audioRates = [128, 160, 192, 256, 320]
  // Same steps as Medal's PC Storage limit.
  const limits = [1, 5, 10, 25, 50, 100, 250, 500]

  const update = (patch: Partial<Settings>) => onchange({ ...settings, ...patch })

  // Recording
  const hz = $derived(caps?.display_hz ? Math.round(caps.display_hz) : null)
  const codecs = $derived(supportedCodecs(caps))
  const preset = $derived(activePreset(settings, caps))
  const clipBytes = $derived(bytesFor(settings, settings.replay_seconds))
  const encoderOptions = $derived(
    (caps?.encoders ?? []).map((e) => ({ ...e, ok: e.codecs.includes(settings.codec) })),
  )
  const autoEncoder = $derived(encoderOptions.find((e) => e.ok)?.encoder)

  function setCodec(codec: Codec) {
    // Drop a manual encoder choice that can't do the new codec.
    const keep = settings.encoder && caps?.encoders.find((e) => e.encoder === settings.encoder)?.codecs.includes(codec)
    update({ codec, encoder: keep ? settings.encoder : null })
  }

  function applyPreset(id: string) {
    const p = presets(caps).find((p) => p.id === id)
    if (p) update({ fps: p.fps, bitrate_kbps: p.bitrate_kbps, codec: p.codec, encoder: null })
  }

  const lengthLabel = (s: number) => (s < 60 ? `${s} seconds` : `${s / 60} minute${s > 60 ? 's' : ''}`)

  // Storage
  let customLimit = $state(false)
  const limitChoice = $derived(
    settings.storage_limit_gb == null
      ? 'none'
      : customLimit || !limits.includes(settings.storage_limit_gb)
        ? 'custom'
        : String(settings.storage_limit_gb),
  )

  function chooseLimit(v: string) {
    customLimit = v === 'custom'
    if (v === 'none') update({ storage_limit_gb: null })
    else if (v !== 'custom') update({ storage_limit_gb: Number(v) })
    else if (settings.storage_limit_gb == null) update({ storage_limit_gb: 20 })
  }

  async function browse() {
    const dir = await openDialog({ directory: true, defaultPath: settings.clips_dir })
    if (typeof dir === 'string') update({ clips_dir: dir })
  }

  const drive = $derived.by(() => {
    if (!usage || !usage.disk_total) return null
    const t = usage.disk_total
    const other = Math.max(0, t - usage.disk_free - usage.clips_bytes)
    return { clips: (usage.clips_bytes / t) * 100, other: (other / t) * 100 }
  })
</script>

<div class="flex h-full gap-8">
  <nav class="w-[188px] flex-none space-y-0.5">
    {#each categories as c (c)}
      {@const Icon = icons[c]}
      <button
        class="flex h-9 w-full items-center gap-3 rounded-[var(--radius-ctl)] px-3 text-left text-[14px] font-medium transition-colors
          {category === c ? 'bg-raised text-white' : 'text-muted hover:bg-white/[0.04] hover:text-zinc-100'}"
        onclick={() => (category = c)}
      >
        <Icon size={16} class={category === c ? 'text-accent' : ''} />
        {c}
      </button>
    {/each}
  </nav>

  <div class="max-w-[760px] min-w-0 flex-1 space-y-6 pb-10">
    <h2 class="display-face text-[24px] font-semibold">{category}</h2>

    {#if category === 'General'}
      <section class="space-y-2">
        <h3 class="section-title px-1">Startup</h3>
        <div class="card divide-y divide-line">
          <Row title="Launch at login" description="Start clip.sh in the background when you sign in.">
            <Toggle label="Launch at login" checked={settings.launch_at_login} onchange={(launch_at_login) => update({ launch_at_login })} />
          </Row>
          <Row title="Start in tray" description="When launched at login, don't open the window." disabled={!settings.launch_at_login}>
            <Toggle
              label="Start in tray"
              disabled={!settings.launch_at_login}
              checked={settings.start_in_tray}
              onchange={(start_in_tray) => update({ start_in_tray })}
            />
          </Row>
          <Row title="Closing the window" description="Keep recording in the tray, or quit completely.">
            <select
              class="field"
              value={settings.close_to_tray ? 'tray' : 'quit'}
              onchange={(e) => update({ close_to_tray: e.currentTarget.value === 'tray' })}
            >
              <option value="tray">Minimizes to tray</option>
              <option value="quit">Quits clip.sh</option>
            </select>
          </Row>
        </div>
      </section>
      <section class="space-y-2">
        <h3 class="section-title px-1">When a clip is saved</h3>
        <div class="card divide-y divide-line">
          <Row title="Show a notification" description="Includes the game the clip was saved under.">
            <Toggle label="Show a notification" checked={settings.notify_on_save} onchange={(notify_on_save) => update({ notify_on_save })} />
          </Row>
          <Row title="Play a sound" description="A short shutter sound you can hear in-game.">
            <Toggle label="Play a sound" checked={settings.sound_on_save} onchange={(sound_on_save) => update({ sound_on_save })} />
          </Row>
        </div>
      </section>
    {:else if category === 'Recording'}
      <section class="space-y-2">
        <h3 class="section-title px-1">Quality</h3>
        <div class="card divide-y divide-line">
          <Row title="Preset" description="Tuned for your display and GPU. Changing any value below makes it Custom.">
            <Segmented
              label="Preset"
              value={preset}
              onchange={applyPreset}
              options={[
                ...presets(caps).map((p) => ({ value: p.id as string, label: p.label })),
                ...(preset === 'custom' ? [{ value: 'custom', label: 'Custom' }] : []),
              ]}
            />
          </Row>
          <Row
            title="Frame rate"
            description={hz ? `Your display runs at ${hz} Hz. Higher rates repeat frames.` : 'Frames per second.'}
          >
            <select class="field w-[180px]" value={settings.fps} onchange={(e) => update({ fps: Number(e.currentTarget.value) })}>
              {#each FPS_OPTIONS as f (f)}
                <option value={f}>{f} FPS{hz && f > hz + 1 ? ' · above display' : ''}</option>
              {/each}
            </select>
          </Row>
          <Row title="Bitrate" description="More bitrate keeps fast motion sharp.">
            <input
              type="range"
              min="5"
              max="100"
              step="5"
              class="w-44"
              style:--fill="{((settings.bitrate_kbps / 1000 - 5) / 95) * 100}%"
              value={settings.bitrate_kbps / 1000}
              onchange={(e) => update({ bitrate_kbps: Number(e.currentTarget.value) * 1000 })}
              oninput={(e) => e.currentTarget.style.setProperty('--fill', `${((Number(e.currentTarget.value) - 5) / 95) * 100}%`)}
            />
            <span class="w-[74px] text-right text-[14px] font-medium tabular-nums">{settings.bitrate_kbps / 1000} Mbps</span>
          </Row>
          <Row
            title="Codec"
            description={settings.codec === 'h264'
              ? 'H.264 plays everywhere, including Discord and phones.'
              : settings.codec === 'hevc'
                ? 'HEVC is about a third smaller at the same quality. Some apps can’t play it.'
                : 'AV1 gives the best quality per megabyte. Needs a recent player.'}
          >
            <Segmented
              label="Codec"
              value={settings.codec}
              onchange={setCodec}
              options={(['h264', 'hevc', 'av1'] as Codec[]).map((c) => ({
                value: c,
                label: CODEC_LABEL[c],
                disabled: !codecs.has(c),
                title: codecs.has(c) ? undefined : 'Not supported by your GPU',
              }))}
            />
          </Row>
          <Row title="Encoder" description="Hardware encoders cost almost no game performance.">
            <select
              class="field w-[200px]"
              value={settings.encoder ?? 'auto'}
              onchange={(e) => {
                const v = e.currentTarget.value
                update({ encoder: v === 'auto' ? null : (v as Encoder) })
              }}
            >
              <option value="auto">Auto{autoEncoder ? ` · ${ENCODER_LABEL[autoEncoder]}` : ''}</option>
              {#each encoderOptions as e (e.encoder)}
                <option value={e.encoder} disabled={!e.ok}>
                  {ENCODER_LABEL[e.encoder]}{e.ok ? '' : ' · unsupported'}
                </option>
              {/each}
            </select>
          </Row>
        </div>
        <p class="px-1 text-[13px] text-muted">
          A {lengthLabel(settings.replay_seconds)} clip is about
          <span class="font-medium text-zinc-200">{bytes(clipBytes)}</span>, and the replay buffer holds the same in memory.
          {#if clipBytes > 2e9}
            <span class="text-accent">That’s a lot of RAM; consider a shorter replay or lower bitrate.</span>
          {/if}
        </p>
      </section>

      <section class="space-y-2">
        <h3 class="section-title px-1">Capture</h3>
        <div class="card divide-y divide-line">
          <Row title="Replay length" description="How far back a clip reaches when you press the hotkey.">
            <select
              class="field w-[180px]"
              value={settings.replay_seconds}
              onchange={(e) => update({ replay_seconds: Number(e.currentTarget.value) })}
            >
              {#each lengths as s (s)}
                <option value={s}>{lengthLabel(s)}</option>
              {/each}
            </select>
          </Row>
          <Row title="Record cursor" description="Draw the mouse pointer into clips.">
            <Toggle label="Record cursor" checked={settings.cursor} onchange={(cursor) => update({ cursor })} />
          </Row>
          <Row
            title="Capture method"
            description={status?.backend === 'kwin'
              ? 'Recording each game’s window directly, with no permission prompts (KDE Plasma).'
              : status?.backend === 'portal'
                ? 'Recording the screen you chose through your desktop. Works on any Wayland desktop.'
                : 'How games are captured.'}
          >
            <select
              class="field w-[200px]"
              value={settings.capture_backend ?? 'auto'}
              onchange={(e) => {
                const v = e.currentTarget.value
                update({ capture_backend: v === 'auto' ? null : (v as 'kwin' | 'portal') })
              }}
            >
              <option value="auto">Automatic</option>
              <option value="kwin">Game window (KDE)</option>
              <option value="portal">Screen (any desktop)</option>
            </select>
          </Row>
          {#if status?.backend === 'portal'}
            <Row
              title="Screen"
              description={status.needs_screen
                ? 'Choose which screen to record. Your desktop asks once and remembers.'
                : 'Your desktop remembers this choice; games are recorded without asking.'}
            >
              <button class="btn {status.needs_screen ? 'border-accent/60 text-accent' : ''}" onclick={onchoosescreen}>
                {status.needs_screen ? 'Choose screen' : 'Change screen'}
              </button>
            </Row>
          {/if}
        </div>
        <p class="px-1 text-[13px] text-muted">
          Recording changes apply from the next game you launch; a new capture method applies after restarting clip.sh.
        </p>
      </section>
    {:else if category === 'Audio'}
      <section class="space-y-2">
        <h3 class="section-title px-1">Sources</h3>
        <div class="card divide-y divide-line">
          <Row title="Game and desktop audio" description="Everything playing through your default output.">
            <Toggle label="Game and desktop audio" checked={settings.desktop_audio} onchange={(desktop_audio) => update({ desktop_audio })} />
          </Row>
          <Row title="Microphone" description="Saved as its own track, so you can mix it when trimming.">
            <Toggle label="Microphone" checked={settings.mic} onchange={(mic) => update({ mic })} />
          </Row>
          <Row title="Input device" disabled={!settings.mic}>
            <select
              class="field w-[260px]"
              disabled={!settings.mic}
              value={settings.mic_device ?? ''}
              onchange={(e) => update({ mic_device: e.currentTarget.value || null })}
            >
              <option value="">System default</option>
              {#each caps?.mic_devices ?? [] as d (d.id)}
                <option value={d.id}>{d.name}</option>
              {/each}
            </select>
          </Row>
        </div>
      </section>
      <section class="space-y-2">
        <h3 class="section-title px-1">Quality</h3>
        <div class="card divide-y divide-line">
          <Row title="Audio bitrate" description="Per track, AAC. 160 kbps is transparent for most games.">
            <select
              class="field w-[140px]"
              value={settings.audio_bitrate_kbps}
              onchange={(e) => update({ audio_bitrate_kbps: Number(e.currentTarget.value) })}
            >
              {#each audioRates as r (r)}
                <option value={r}>{r} kbps</option>
              {/each}
            </select>
          </Row>
        </div>
        <p class="px-1 text-[13px] text-muted">Recording changes apply from the next game you launch.</p>
      </section>
    {:else if category === 'Hotkeys'}
      <section class="space-y-2">
        <h3 class="section-title px-1">Clipping</h3>
        <div class="card divide-y divide-line">
          {#if hotkeyBackend !== null}
            <Row
              title="Clip that"
              description={hotkeyBackend === 'portal'
                ? 'Registered with your desktop, which may ask to confirm it once.'
                : 'Saves the replay buffer from the current game. Works in fullscreen games.'}
            >
              <HotkeyBinder value={settings.hotkey} onbind={(hotkey) => update({ hotkey })} />
            </Row>
          {/if}
          <Row
            title="Command"
            description={hotkeyBackend === null
              ? 'Your desktop has no global shortcut service. Bind this command to a key in its keyboard settings.'
              : 'Bind this to any key in your desktop’s keyboard settings, or use it from scripts.'}
          >
            <code class="rounded-[var(--radius-ctl)] border border-line bg-canvas px-2.5 py-1.5 text-[13px] text-zinc-200">clipsh save</code>
            <button class="btn" onclick={copyCommand}>{copied ? 'Copied' : 'Copy'}</button>
          </Row>
        </div>
        {#if hotkeyBackend === 'kglobalaccel'}
          <p class="px-1 text-[13px] text-muted">Also listed in System Settings → Shortcuts, under clip.sh.</p>
        {/if}
      </section>
    {:else if category === 'Storage'}
      <section class="space-y-2">
        <h3 class="section-title px-1">Location</h3>
        <div class="card divide-y divide-line">
          <Row title="Clips folder" description="One folder per game.">
            <input class="field w-[280px] cursor-pointer" readonly value={settings.clips_dir} onclick={browse} title={settings.clips_dir} />
            <button class="btn" onclick={browse}>Browse</button>
            <button class="btn w-9 px-0" onclick={() => api.openFolder()} title="Open folder" aria-label="Open folder">
              <FolderOpen size={16} />
            </button>
          </Row>
          {#if usage && drive}
            <div class="px-4 py-4">
              <div class="flex items-baseline justify-between text-[13px]">
                <span class="font-medium text-zinc-100">Drive</span>
                <span class="text-muted tabular-nums">{bytes(usage.disk_free)} free of {bytes(usage.disk_total)}</span>
              </div>
              <div class="mt-2.5 flex h-1.5 overflow-hidden rounded-full bg-zinc-800">
                <div class="bg-accent" style:width="{Math.max(drive.clips, usage.clips_bytes ? 0.5 : 0)}%"></div>
                <div class="bg-zinc-500" style:width="{drive.other}%"></div>
              </div>
              <div class="mt-2.5 flex gap-5 text-[12px] text-muted">
                <span class="flex items-center gap-1.5"><span class="size-2 rounded-[2px] bg-accent"></span>Clips · {bytes(usage.clips_bytes)}</span>
                <span class="flex items-center gap-1.5"><span class="size-2 rounded-[2px] bg-zinc-500"></span>Other files</span>
                <span class="flex items-center gap-1.5"><span class="size-2 rounded-[2px] bg-zinc-800"></span>Free</span>
              </div>
            </div>
          {/if}
        </div>
      </section>

      <section class="space-y-2">
        <h3 class="section-title px-1">File limits</h3>
        <div class="card divide-y divide-line">
          <Row title="Storage limit" description="Over the limit, the oldest clips are removed after each save.">
            {#if limitChoice === 'custom'}
              <input
                class="field w-20 text-right tabular-nums"
                type="number"
                min="1"
                step="1"
                value={settings.storage_limit_gb ?? 20}
                onchange={(e) => {
                  const v = Number(e.currentTarget.value)
                  if (v >= 1) update({ storage_limit_gb: v })
                }}
              />
              <span class="text-[13px] text-muted">GB</span>
            {/if}
            <select class="field w-[140px]" value={limitChoice} onchange={(e) => chooseLimit(e.currentTarget.value)}>
              <option value="none">No limit</option>
              {#each limits as l (l)}
                <option value={String(l)}>{l} GB</option>
              {/each}
              <option value="custom">Custom</option>
            </select>
          </Row>
          <Row
            title="Protect edited clips"
            description="Only remove original captures, never clips you rendered in the trimmer."
            disabled={settings.storage_limit_gb == null}
          >
            <Toggle
              label="Protect edited clips"
              disabled={settings.storage_limit_gb == null}
              checked={settings.protect_edits}
              onchange={(protect_edits) => update({ protect_edits })}
            />
          </Row>
          <Row title="Move deleted clips to Trash" description="Deleted and removed clips stay restorable until you empty the Trash.">
            <Toggle label="Move deleted clips to Trash" checked={settings.use_trash} onchange={(use_trash) => update({ use_trash })} />
          </Row>
        </div>
      </section>
    {:else if category === 'Appearance'}
      <section class="space-y-2">
        <h3 class="section-title px-1">Theme</h3>
        <div class="card divide-y divide-line">
          <Row title="Accent colour" description="Used for buttons, the record button and the trim range.">
            <div class="flex items-center gap-2" role="radiogroup" aria-label="Accent colour">
              {#each Object.entries(ACCENTS) as [id, a] (id)}
                <button
                  role="radio"
                  aria-checked={prefs.accent === id}
                  aria-label={a.label}
                  title={a.label}
                  class="flex size-7 items-center justify-center rounded-full text-black ring-offset-2 ring-offset-panel transition-shadow hover:ring-2 hover:ring-zinc-500
                    {prefs.accent === id ? 'ring-2 ring-white' : ''}"
                  style:background={a.hex}
                  onclick={() => setPrefs({ accent: id as Accent })}
                >
                  {#if prefs.accent === id}<Check size={14} strokeWidth={3} />{/if}
                </button>
              {/each}
            </div>
          </Row>
          <Row title="Card size" description="How many clips fit across the library.">
            <Segmented
              label="Card size"
              value={prefs.density}
              onchange={(density) => setPrefs({ density })}
              options={[
                { value: 'comfortable', label: 'Comfortable' },
                { value: 'compact', label: 'Compact' },
              ]}
            />
          </Row>
        </div>
      </section>
      <section class="space-y-2">
        <h3 class="section-title px-1">Motion</h3>
        <div class="card divide-y divide-line">
          <Row title="Preview clips on hover" description="Play a silent preview when the pointer rests on a clip.">
            <Toggle label="Preview clips on hover" checked={prefs.hoverPreviews} onchange={(hoverPreviews) => setPrefs({ hoverPreviews })} />
          </Row>
          <Row title="Reduce motion" description="Turn off animations and transitions. Follows your system setting when off.">
            <Toggle label="Reduce motion" checked={prefs.reduceMotion} onchange={(reduceMotion) => setPrefs({ reduceMotion })} />
          </Row>
        </div>
      </section>
      <p class="px-1 text-[13px] text-muted">Appearance settings are stored on this computer only.</p>
    {/if}
  </div>
</div>
