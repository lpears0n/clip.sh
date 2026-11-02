<script lang="ts">
  import { Clapperboard, Gamepad2, Settings as Gear } from '@lucide/svelte'
  import { getCurrentWebview } from '@tauri-apps/api/webview'
  import { open as openDialog } from '@tauri-apps/plugin-dialog'
  import { onMount } from 'svelte'
  import AdSlot from './lib/AdSlot.svelte'
  import {
    api,
    type Capabilities,
    type Clip,
    type GameEntry,
    initMedia,
    onClipSaved,
    onLibraryChanged,
    onStatusChanged,
    type Settings,
    type Status,
    type Usage,
  } from './lib/api'
  import ClipsView from './lib/ClipsView.svelte'
  import ClipViewer from './lib/ClipViewer.svelte'
  import GamesView from './lib/GamesView.svelte'
  import GameView from './lib/GameView.svelte'
  import NavButton from './lib/NavButton.svelte'
  import { applyPrefs, prefs } from './lib/prefs.svelte'
  import SettingsView, { type Category } from './lib/SettingsView.svelte'
  import StatusBar from './lib/StatusBar.svelte'
  import StorageRing from './lib/StorageRing.svelte'
  import Toast, { type ToastData } from './lib/Toast.svelte'

  type View = 'games' | 'game' | 'clips' | 'settings'
  const VIDEO = /\.(mp4|mkv|mov|webm)$/i
  /** How long a deleted clip can be brought back from the toast. */
  const UNDO_MS = 6000

  let view = $state<View>('games')
  /** The game page being shown, by name (entries are refreshed often). */
  let gameName = $state<string | null>(null)
  /** Clip open in the viewer, layered over the current view. */
  let viewing = $state<Clip | null>(null)
  let category = $state<Category>('General')

  let status = $state<Status | null>(null)
  let settings = $state<Settings | null>(null)
  let usage = $state<Usage | null>(null)
  let caps = $state<Capabilities | null>(null)
  let allClips = $state<Clip[]>([])
  let games = $state<GameEntry[]>([])
  let loading = $state(true)
  let rendering = $state(false)
  let dragging = $state(false)
  let saving = $state(false)
  let flash = $state(false)
  let toast = $state<ToastData | null>(null)
  let toastTimer: ReturnType<typeof setTimeout>

  // Deletes are deferred so they can be undone; the clips vanish from the
  // UI at once and are only removed from disk when the toast expires.
  let pending = $state<{ clips: Clip[]; timer: ReturnType<typeof setTimeout> }[]>([])
  const hidden = $derived(new Set(pending.flatMap((p) => p.clips.map((c) => c.path))))
  const clips = $derived(hidden.size ? allClips.filter((c) => !hidden.has(c.path)) : allClips)

  const game = $derived(games.find((g) => g.name === gameName) ?? null)
  const hotkey = $derived(settings?.hotkey ?? 'F8')

  function notify(text: string, error = false) {
    showToast({ text, error })
  }

  function showToast(t: ToastData, ms = 3000) {
    toast = t
    clearTimeout(toastTimer)
    toastTimer = setTimeout(() => (toast = null), ms)
  }

  function go(v: View) {
    view = v
    viewing = null
  }

  async function refreshLibrary() {
    try {
      // Fetch clips first: listing them generates the thumbnails games use.
      allClips = await api.clips()
      games = await api.games()
    } catch (e) {
      notify(String(e), true)
    } finally {
      loading = false
    }
  }

  async function refreshStatus() {
    try {
      status = await api.status()
    } catch {
      // The recorder may still be starting.
    }
  }

  async function refreshUsage() {
    try {
      usage = await api.storage()
    } catch {
      // Not critical; the ring just stays as it was.
    }
  }

  async function save() {
    saving = true
    try {
      await api.save()
    } catch (e) {
      notify(String(e), true)
    } finally {
      saving = false
    }
  }

  async function open(path: string) {
    try {
      viewing = await api.openForEdit(path)
    } catch (e) {
      notify(String(e), true)
    }
  }

  async function pickVideo() {
    const path = await openDialog({
      multiple: false,
      filters: [{ name: 'Video', extensions: ['mp4', 'mkv', 'mov', 'webm'] }],
    })
    if (typeof path === 'string') open(path)
  }

  async function render(start: number, end: number, volumes: number[]) {
    if (!viewing) return
    rendering = true
    try {
      await api.render(viewing.path, start, end, volumes)
    } catch (e) {
      notify(String(e), true)
    } finally {
      rendering = false
    }
  }

  /** Hide the clips now; delete them for real when the undo window closes. */
  function remove(list: Clip[]) {
    if (!list.length) return
    if (viewing && list.some((c) => c.path === viewing!.path)) viewing = null
    const entry = { clips: list, timer: setTimeout(() => commit(entry), UNDO_MS) }
    pending = [...pending, entry]
    const label = list.length === 1 ? `Deleted ${list[0].game} clip` : `Deleted ${list.length} clips`
    showToast({ text: label, action: { label: 'Undo', run: () => undo(entry) } }, UNDO_MS)
  }

  function undo(entry: (typeof pending)[number]) {
    clearTimeout(entry.timer)
    pending = pending.filter((p) => p !== entry)
    toast = null
  }

  async function commit(entry: (typeof pending)[number]) {
    pending = pending.filter((p) => p !== entry)
    const failed: string[] = []
    for (const c of entry.clips) {
      try {
        await api.deleteClip(c.path)
      } catch (e) {
        failed.push(String(e))
      }
    }
    if (failed.length) notify(failed[0], true)
    refreshLibrary()
    refreshUsage()
  }

  /** Throws so the hotkey binder can show the reason. */
  async function changeSettings(s: Settings) {
    await api.setSettings(s)
    const dirChanged = s.clips_dir !== settings?.clips_dir
    settings = s
    if (dirChanged) refreshLibrary()
    refreshUsage()
  }

  async function chooseScreen() {
    try {
      await api.chooseScreen()
      refreshStatus()
    } catch (e) {
      notify(String(e), true)
    }
  }

  function openSettings(c: Category) {
    category = c
    go('settings')
  }

  $effect(() => applyPrefs(prefs))

  onMount(() => {
    initMedia()
    refreshStatus()
    refreshLibrary()
    refreshUsage()
    api.settings().then((s) => (settings = s))
    // Probing encoders takes a moment; settings fill in when it's done.
    api.capabilities().then((c) => (caps = c))
    // State changes arrive as events; polls keep buffer, disk and play time fresh.
    const polls = [setInterval(refreshStatus, 2000), setInterval(refreshUsage, 15000), setInterval(refreshLibrary, 60000)]
    const unlisten = [
      onStatusChanged((s) => {
        status = s
        // A game started or stopped: "playing now" and play time changed.
        refreshLibrary()
      }),
      onClipSaved((path) => {
        notify(`Saved ${path.split('/').pop()}`)
        flash = true
        setTimeout(() => (flash = false), 500)
        refreshLibrary()
        refreshUsage()
      }),
      onLibraryChanged(() => {
        refreshLibrary()
        refreshUsage()
      }),
      getCurrentWebview().onDragDropEvent((e) => {
        const p = e.payload
        dragging = p.type === 'enter' || p.type === 'over'
        if (p.type === 'drop') {
          const file = p.paths.find((f) => VIDEO.test(f))
          if (file) open(file)
        }
      }),
    ]
    const motion = matchMedia('(prefers-reduced-motion: reduce)')
    const onMotion = () => applyPrefs(prefs)
    motion.addEventListener('change', onMotion)
    return () => {
      polls.forEach(clearInterval)
      unlisten.forEach((u) => u.then((f) => f()))
      motion.removeEventListener('change', onMotion)
    }
  })
</script>

<div class="flex h-full flex-col">
  <header class="flex h-14 flex-none items-center gap-4 border-b border-line bg-panel pr-4 pl-[22px]">
    <span class="display-face flex-none text-[19px] font-bold">clip<span class="text-accent">.sh</span></span>
    <div class="min-w-0 flex-1"></div>
    <StatusBar {status} {flash} {saving} {hotkey} onsave={save} onopen={pickVideo} onchoosescreen={chooseScreen} />
  </header>

  <div class="flex min-h-0 flex-1">
    <nav class="flex w-[72px] flex-none flex-col items-center gap-1.5 border-r border-line bg-panel py-3">
      <NavButton
        icon={Gamepad2}
        label="Games"
        active={!viewing && (view === 'games' || view === 'game')}
        onclick={() => go('games')}
      />
      <NavButton icon={Clapperboard} label="All clips" active={!viewing && view === 'clips'} onclick={() => go('clips')} />
      <div class="flex-1"></div>
      <StorageRing {usage} onmanage={() => openSettings('Storage')} />
      <NavButton icon={Gear} label="Settings" active={!viewing && view === 'settings'} onclick={() => go('settings')} />
    </nav>

    <main class="flex min-w-0 flex-1 flex-col bg-canvas">
      <div class="min-h-0 flex-1 overflow-y-auto p-6">
        {#if viewing}
          <ClipViewer
            clip={viewing}
            {rendering}
            onback={() => (viewing = null)}
            onrender={render}
            ondelete={() => viewing && remove([viewing])}
            onshare={() => viewing && api.reveal(viewing.path)}
          />
        {:else if view === 'game' && game}
          <GameView
            {game}
            {clips}
            onback={() => go('games')}
            onopen={(c) => open(c.path)}
            ondelete={(c) => remove([c])}
            onshare={(c) => api.reveal(c.path)}
          />
        {:else if view === 'clips'}
          <ClipsView
            {clips}
            {loading}
            {hotkey}
            onopen={(c) => open(c.path)}
            ondelete={(c) => remove([c])}
            ondeletemany={remove}
            onshare={(c) => api.reveal(c.path)}
          />
        {:else if view === 'settings' && settings}
          <SettingsView {settings} {usage} {caps} {status} bind:category onchange={changeSettings} onchoosescreen={chooseScreen} />
        {:else}
          <GamesView
            {games}
            {loading}
            {hotkey}
            {status}
            {settings}
            onopen={(g) => {
              gameName = g.name
              go('game')
            }}
            onchoosescreen={chooseScreen}
            onsettings={openSettings}
          />
        {/if}
      </div>
      {#if settings?.ads}
        <div class="flex justify-center border-t border-line p-3">
          <AdSlot enabled width={728} height={90} />
        </div>
      {/if}
    </main>
  </div>
</div>

{#if dragging}
  <div class="pointer-events-none fixed inset-0 z-50 flex items-center justify-center bg-canvas/85">
    <div class="display-face rounded-[var(--radius-card)] border-2 border-accent px-10 py-6 text-[22px] font-semibold">
      Drop to open
    </div>
  </div>
{/if}

{#if toast}
  <Toast data={toast} onclose={() => (toast = null)} />
{/if}
