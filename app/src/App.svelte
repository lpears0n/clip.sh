<script lang="ts">
  import { CircleAlert, PanelLeft } from '@lucide/svelte'
  import { getCurrentWebview } from '@tauri-apps/api/webview'
  import { open as openDialog } from '@tauri-apps/plugin-dialog'
  import { onMount } from 'svelte'
  import AdSlot from './lib/AdSlot.svelte'
  import {
    api,
    type Capabilities,
    type Clip,
    type ClipPatch,
    type GameEntry,
    initMedia,
    onClipSaved,
    onLibraryChanged,
    onRenderProgress,
    onStatusChanged,
    onThumbnailReady,
    type Settings,
    type Status,
    type Usage,
  } from './lib/api'
  import { readLibraryCache, writeLibraryCache } from './lib/cache'
  import ClipsView from './lib/ClipsView.svelte'
  import { coalesce } from './lib/coalesce'
  import ClipViewer from './lib/ClipViewer.svelte'
  import GamesView from './lib/GamesView.svelte'
  import GameView from './lib/GameView.svelte'
  import { applyPrefs, prefs, setPrefs } from './lib/prefs.svelte'
  import SettingsView, { type Category } from './lib/SettingsView.svelte'
  import Sidebar, { type NavId } from './lib/Sidebar.svelte'
  import StateMessage from './lib/StateMessage.svelte'
  import StatusBar from './lib/StatusBar.svelte'
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
  // Raw: replaced wholesale on every refresh; a deep proxy over thousands of
  // clips is slow and unnecessary.
  let allClips = $state.raw<Clip[]>([])
  let games = $state.raw<GameEntry[]>([])
  let loading = $state(true)
  /** Wall clock for live counters (buffer fill, "playing for"); ticks once
   *  a second while recording, otherwise once a minute. No IPC involved. */
  let now = $state(Date.now())
  /** Why the library couldn't be read, shown in place of the views. */
  let libraryError = $state<string | null>(null)
  let rendering = $state(false)
  let renderProgress = $state(0)
  let dragging = $state(false)
  let saving = $state(false)
  let flash = $state(false)
  let toast = $state<ToastData | null>(null)
  let toastTimer: ReturnType<typeof setTimeout>

  // Deletes are deferred so they can be undone; the clips vanish from the
  // UI at once and are only removed from disk when the toast expires.
  // Raw state: entries are compared by identity when undone or committed,
  // which a deep proxy would break. The array is always reassigned.
  let pending = $state.raw<{ clips: Clip[]; timer: ReturnType<typeof setTimeout> }[]>([])
  const hidden = $derived(new Set(pending.flatMap((p) => p.clips.map((c) => c.path))))
  const clips = $derived(hidden.size ? allClips.filter((c) => !hidden.has(c.path)) : allClips)

  const game = $derived(games.find((g) => g.name === gameName) ?? null)
  const hotkey = $derived(settings?.hotkey ?? 'F8')
  const nav = $derived<NavId>(view === 'game' ? 'games' : view)
  const title = $derived(
    view === 'games' ? 'Games' : view === 'game' ? (game?.name ?? 'Games') : view === 'clips' ? 'All clips' : 'Settings',
  )

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

  async function loadLibrary() {
    try {
      allClips = await api.clips()
      games = await api.games()
      libraryError = null
      writeLibraryCache(allClips, games)
    } catch (e) {
      libraryError = String(e)
    } finally {
      loading = false
    }
  }

  // Saves, the folder watcher and storage enforcement can all fire within
  // the same second; they cost one fetch.
  const refreshLibrary = coalesce(loadLibrary)

  /** A clip's images and duration arrived from the thumbnail pool. */
  function thumbnailReady(c: Clip) {
    allClips = allClips.map((x) => (x.path === c.path ? { ...x, ...c, title: x.title, favorite: x.favorite, labels: x.labels } : x))
    if (viewing?.path === c.path) viewing = { ...viewing, ...c, title: viewing.title, favorite: viewing.favorite, labels: viewing.labels }
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
      // Not critical; the bar just stays as it was.
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
    renderProgress = 0
    try {
      await api.render(viewing.path, start, end, volumes)
    } catch (e) {
      if (!/cancelled/.test(String(e))) notify(String(e), true)
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
    const label = list.length === 1 ? `Deleted ${list[0].title ?? list[0].game} clip` : `Deleted ${list.length} clips`
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

  /** Only library clips carry user metadata; opened files are read-only. */
  const inLibrary = (c: Clip) => !!settings && c.path.startsWith(settings.clips_dir + '/')

  /** Title, favourite or labels. Applies optimistically, then takes the
   *  server's tidied version. */
  async function update(clip: Clip, patch: ClipPatch) {
    const optimistic: Clip = {
      ...clip,
      title: patch.title === undefined ? clip.title : patch.title.trim() || null,
      favorite: patch.favorite ?? clip.favorite,
      labels: patch.labels ?? clip.labels,
    }
    const put = (c: Clip) => {
      allClips = allClips.map((x) => (x.path === c.path ? c : x))
      if (viewing?.path === c.path) viewing = c
    }
    put(optimistic)
    try {
      put(await api.updateClip(clip.path, patch))
    } catch (e) {
      put(clip)
      notify(String(e), true)
    }
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

  function onkeydown(e: KeyboardEvent) {
    const typing = e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement
    if ((e.ctrlKey || e.metaKey) && e.key === 'b') {
      e.preventDefault()
      setPrefs({ sidebarCollapsed: !prefs.sidebarCollapsed })
    } else if (!typing && !viewing && !e.ctrlKey && !e.altKey && !e.metaKey) {
      if (e.key === '1') go('games')
      else if (e.key === '2') go('clips')
      else if (e.key === '3') go('settings')
    }
  }

  $effect(() => applyPrefs(prefs))

  onMount(() => {
    initMedia()
    // Paint the last known library at once; the real listing replaces it.
    const cached = readLibraryCache()
    if (cached) {
      allClips = cached.clips
      games = cached.games
      loading = false
    }
    refreshStatus()
    loadLibrary()
    refreshUsage()
    api.settings().then((s) => (settings = s))
    // Probing encoders takes a moment; settings fill in when it's done.
    api.capabilities().then((c) => (caps = c))
    // Everything else arrives as events. Coming back to the window is the one
    // moment worth re-checking, in case an event was missed while hidden.
    const onFocus = () => {
      if (document.visibilityState !== 'visible') return
      refreshStatus()
      refreshLibrary()
      refreshUsage()
    }
    window.addEventListener('focus', onFocus)
    document.addEventListener('visibilitychange', onFocus)
    const tick = setInterval(() => {
      const recording = status?.state === 'recording'
      if (recording || Date.now() - now >= 60_000) now = Date.now()
    }, 1000)
    const unlisten = [
      onStatusChanged((s) => {
        status = s
        // A game started or stopped: "playing now" and play time changed.
        refreshLibrary()
      }),
      onClipSaved((path) => {
        notify(`Saved ${path.split('/').pop()}`)
        flash = true
        setTimeout(() => (flash = false), 400)
        refreshLibrary()
        refreshUsage()
      }),
      onLibraryChanged(() => {
        refreshLibrary()
        refreshUsage()
      }),
      onThumbnailReady(thumbnailReady),
      onRenderProgress((p) => {
        if (viewing?.path === p.path) renderProgress = p.fraction
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
      clearInterval(tick)
      window.removeEventListener('focus', onFocus)
      document.removeEventListener('visibilitychange', onFocus)
      unlisten.forEach((u) => u.then((f) => f()))
      motion.removeEventListener('change', onMotion)
    }
  })
</script>

<svelte:window {onkeydown} />

<div class="flex h-full">
  <Sidebar active={nav} collapsed={prefs.sidebarCollapsed} {usage} onnavigate={go} onstorage={() => openSettings('Storage')} />

  <div class="flex min-w-0 flex-1 flex-col">
    <header class="flex h-14 flex-none items-center gap-3 border-b border-line bg-panel pr-4 pl-3">
      <button
        class="btn-icon text-muted"
        onclick={() => setPrefs({ sidebarCollapsed: !prefs.sidebarCollapsed })}
        aria-label={prefs.sidebarCollapsed ? 'Expand sidebar' : 'Collapse sidebar'}
        title="Toggle sidebar (Ctrl+B)"
      >
        <PanelLeft size={18} />
      </button>
      <h1 class="display-face truncate text-title font-semibold">{title}</h1>
      <div class="min-w-0 flex-1"></div>
      <StatusBar {status} {now} {flash} {saving} {hotkey} onsave={save} onopen={pickVideo} onchoosescreen={chooseScreen} />
    </header>

    <main class="flex min-h-0 flex-1 flex-col bg-canvas">
      <div class="min-h-0 flex-1 overflow-y-auto p-6">
        {#if viewing}
          <ClipViewer
            clip={viewing}
            {rendering}
            progress={renderProgress}
            onback={() => (viewing = null)}
            onrender={render}
            oncancel={() => api.cancelRender()}
            ondelete={() => viewing && remove([viewing])}
            onshare={() => viewing && api.reveal(viewing.path)}
            onupdate={inLibrary(viewing) ? (p) => viewing && update(viewing, p) : undefined}
          />
        {:else if libraryError && view !== 'settings'}
          <StateMessage icon={CircleAlert} title="Couldn't read the clips folder" body={libraryError} error>
            <button class="btn" onclick={() => openSettings('Storage')}>Storage settings</button>
            <button class="btn-primary" onclick={refreshLibrary}>Try again</button>
          </StateMessage>
        {:else if view === 'game' && game}
          <GameView
            {game}
            {clips}
            onback={() => go('games')}
            onopen={(c) => open(c.path)}
            ondelete={(c) => remove([c])}
            onshare={(c) => api.reveal(c.path)}
            onfavorite={(c) => update(c, { favorite: !c.favorite })}
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
            onfavorite={(c) => update(c, { favorite: !c.favorite })}
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
            {now}
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
    <div class="display-face rounded-surface border-2 border-accent px-10 py-6 text-page font-semibold">Drop to open</div>
  </div>
{/if}

{#if toast}
  <Toast data={toast} onclose={() => (toast = null)} />
{/if}
