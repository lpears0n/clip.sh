<!--
  Watch a clip, or trim it (in/out, audio mix, render) on the same screen.
  Playback controls overlay the video's bottom edge and hide while playing.
-->
<script lang="ts">
  import { ArrowLeft, CircleHelp, Maximize, Minimize, Pause, Pencil, Play, Plus, Repeat, Star, Volume2, VolumeX, X } from '@lucide/svelte'
  import { getCurrentWindow } from '@tauri-apps/api/window'
  import { type Clip, type ClipPatch, videoUrl } from './api'
  import { clock, dayLabel, size, timecode, trackName } from './format'
  import { playbackStats } from './playbackStats'
  import Timeline from './Timeline.svelte'

  let {
    clip,
    rendering,
    onback,
    onrender,
    ondelete,
    onshare,
    onupdate,
  }: {
    clip: Clip
    rendering: boolean
    onback: () => void
    onrender: (start: number, end: number, volumes: number[]) => void
    ondelete: () => void
    onshare: () => void
    /** Absent for videos opened from outside the library. */
    onupdate?: (patch: ClipPatch) => void
  } = $props()

  const subtitle = $derived(
    [
      clip.title ? clip.game : null,
      `${dayLabel(clip.created_ms)}, ${clock(clip.created_ms)}`,
      size(clip.size_bytes),
      clip.is_edit ? 'edited' : null,
    ]
      .filter(Boolean)
      .join(' · '),
  )

  // Title editing and label entry.
  let editingTitle = $state(false)
  let titleDraft = $state('')
  let titleInput: HTMLInputElement | undefined = $state()
  let addingLabel = $state(false)
  let labelDraft = $state('')
  let labelInput: HTMLInputElement | undefined = $state()

  function startTitle() {
    if (!onupdate) return
    titleDraft = clip.title ?? ''
    editingTitle = true
    queueMicrotask(() => titleInput?.select())
  }

  function commitTitle() {
    if (!editingTitle) return
    editingTitle = false
    if ((clip.title ?? '') !== titleDraft.trim()) onupdate?.({ title: titleDraft })
  }

  function addLabel() {
    const l = labelDraft.trim()
    labelDraft = ''
    if (l) onupdate?.({ labels: [...clip.labels, l] })
  }

  function removeLabel(l: string) {
    onupdate?.({ labels: clip.labels.filter((x) => x !== l) })
  }

  let video: HTMLVideoElement
  let duration = $state(0)
  let time = $state(0)
  let paused = $state(true)
  let muted = $state(false)
  let volume = $state(1)
  let aspect = $state(16 / 9)
  let fullscreen = $state(false)
  let trimming = $state(false)
  let start = $state(0)
  let end = $state(0)
  /** Percent per audio track. */
  let levels = $state<number[]>([])
  let armed = $state(false)
  let speed = $state(1)
  let loop = $state(false)
  let help = $state(false)
  const SPEEDS = [0.25, 0.5, 1, 1.5, 2]
  /** One frame at 60 fps; fine enough to land on a moment. */
  const FRAME = 1 / 60

  // Controls show while paused, or briefly after the pointer moves.
  let active = $state(true)
  let idleTimer: ReturnType<typeof setTimeout>
  const showControls = $derived(paused || active || trimming)

  function poke() {
    active = true
    clearTimeout(idleTimer)
    idleTimer = setTimeout(() => (active = false), 2200)
  }

  $effect(() => {
    // Reset when a different clip is loaded.
    clip.path
    trimming = false
    start = 0
    end = clip.duration ?? 0
    levels = Array.from({ length: clip.audio_tracks }, () => 100)
  })

  function loaded() {
    duration = video.duration
    if (video.videoWidth && video.videoHeight) aspect = video.videoWidth / video.videoHeight
    if (!end || end > duration) end = duration
  }

  function tick() {
    time = video.currentTime
    // While trimming, preview only the selection.
    if (trimming && !paused && time >= end) {
      if (loop) seek(start)
      else {
        video.pause()
        video.currentTime = start
      }
    }
  }

  function ended() {
    if (loop) {
      seek(trimming ? start : 0)
      video.play()
    }
  }

  function step(frames: number) {
    video.pause()
    seek(Math.min(duration, Math.max(0, time + frames * FRAME)))
    poke()
  }

  function setSpeed(v: number) {
    speed = v
    video.playbackRate = v
    poke()
  }

  function cycleSpeed() {
    setSpeed(SPEEDS[(SPEEDS.indexOf(speed) + 1) % SPEEDS.length])
  }

  function nudgeSpeed(dir: 1 | -1) {
    const i = Math.min(SPEEDS.length - 1, Math.max(0, SPEEDS.indexOf(speed) + dir))
    setSpeed(SPEEDS[i])
  }

  function seek(t: number) {
    video.currentTime = t
    time = t
  }

  function toggle() {
    if (video.paused) {
      if (trimming && (time >= end - 0.05 || time < start)) seek(start)
      video.play()
    } else {
      video.pause()
    }
    poke()
  }

  async function setFullscreen(on: boolean) {
    fullscreen = on
    try {
      await getCurrentWindow().setFullscreen(on)
    } catch {
      // Without the window going fullscreen it still fills the app.
    }
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement) return
    if (help && e.code !== 'Escape' && e.key !== '?') return
    if (e.code === 'Space' || e.code === 'KeyK') {
      e.preventDefault()
      toggle()
    } else if (e.code === 'KeyF') {
      setFullscreen(!fullscreen)
    } else if (e.code === 'KeyM') {
      muted = !muted
    } else if (e.key === '?') {
      help = !help
    } else if (e.code === 'Escape') {
      if (help) help = false
      else if (fullscreen) setFullscreen(false)
      else if (trimming) trimming = false
      else onback()
    } else if (e.code === 'ArrowLeft') {
      seek(Math.max(0, time - 5))
      poke()
    } else if (e.code === 'ArrowRight') {
      seek(Math.min(duration, time + 5))
      poke()
    } else if (e.code === 'KeyJ') {
      seek(Math.max(0, time - 10))
      poke()
    } else if (e.code === 'KeyL') {
      seek(Math.min(duration, time + 10))
      poke()
    } else if (e.code === 'Home') {
      seek(trimming ? start : 0)
      poke()
    } else if (e.code === 'End') {
      seek(trimming ? end : duration)
      poke()
    } else if (e.key === ',') {
      step(-1)
    } else if (e.key === '.') {
      step(1)
    } else if (e.key === '<') {
      nudgeSpeed(-1)
    } else if (e.key === '>') {
      nudgeSpeed(1)
    } else if (e.code === 'KeyR') {
      loop = !loop
      poke()
    } else if (e.code === 'KeyT') {
      trimming = !trimming
    } else if (e.code === 'KeyS' && onupdate) {
      onupdate({ favorite: !clip.favorite })
    } else if (e.code === 'KeyN' && onupdate) {
      e.preventDefault()
      startTitle()
    } else if (trimming && e.code === 'KeyI') {
      start = Math.min(time, end - 0.1)
    } else if (trimming && e.code === 'KeyO') {
      end = Math.max(time, start + 0.1)
    }
  }

  const SHORTCUTS: [string, string][] = [
    ['Space / K', 'Play or pause'],
    ['J / L', 'Back or forward 10 s'],
    ['← / →', 'Back or forward 5 s'],
    [', / .', 'Previous or next frame'],
    ['< / >', 'Slower or faster'],
    ['Home / End', 'Jump to start or end'],
    ['R', 'Repeat'],
    ['S', 'Star (favourite)'],
    ['N', 'Rename'],
    ['M', 'Mute'],
    ['F', 'Fullscreen'],
    ['T', 'Trim'],
    ['I / O', 'Set trim start / end'],
    ['Esc', 'Back'],
    ['?', 'This list'],
  ]

  // The <video> element plays the first audio track; mirror its level when trimming.
  $effect(() => {
    if (!video) return
    video.volume = trimming ? Math.min(1, (levels[0] ?? 100) / 100) : volume
    video.muted = muted
  })

  $effect(() => () => {
    clearTimeout(idleTimer)
    if (fullscreen) getCurrentWindow().setFullscreen(false).catch(() => {})
  })
</script>

<svelte:window {onkeydown} />

<div class="flex h-full flex-col gap-4">
  <div class="flex items-center gap-2">
    <button class="btn-icon -ml-1.5" onclick={onback} title="Back (Esc)" aria-label="Back">
      <ArrowLeft size={20} />
    </button>
    <div class="min-w-0 flex-1">
      <div class="flex items-center gap-2">
        {#if editingTitle}
          <!-- svelte-ignore a11y_autofocus -->
          <input
            bind:this={titleInput}
            bind:value={titleDraft}
            class="field display-face h-8 w-full max-w-[520px] text-[16px] font-semibold"
            placeholder={clip.game}
            maxlength="120"
            autofocus
            onblur={commitTitle}
            onkeydown={(e) => {
              e.stopPropagation()
              if (e.key === 'Enter') commitTitle()
              else if (e.key === 'Escape') editingTitle = false
            }}
            aria-label="Clip title"
          />
        {:else}
          <button
            class="group/title display-face flex min-w-0 items-center gap-2 truncate text-[17px] font-semibold {onupdate ? '' : 'cursor-default'}"
            onclick={startTitle}
            title={onupdate ? 'Rename (N)' : undefined}
            disabled={!onupdate}
          >
            <span class="truncate">{clip.title ?? clip.game}</span>
            {#if onupdate}<Pencil size={13} class="flex-none text-muted opacity-0 transition-opacity group-hover/title:opacity-100" />{/if}
          </button>
        {/if}
        {#if onupdate}
          <button
            class="btn-icon size-7 flex-none {clip.favorite ? 'text-accent' : 'text-muted'}"
            onclick={() => onupdate({ favorite: !clip.favorite })}
            title={clip.favorite ? 'Remove from favourites (S)' : 'Add to favourites (S)'}
            aria-label={clip.favorite ? 'Remove from favourites' : 'Add to favourites'}
            aria-pressed={clip.favorite}
          >
            <Star size={16} fill={clip.favorite ? 'currentColor' : 'none'} />
          </button>
        {/if}
      </div>
      <div class="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-[13px] text-muted tabular-nums">
        <span class="truncate">{subtitle}</span>
        {#if onupdate || clip.labels.length}
          <span class="flex flex-wrap items-center gap-1">
            {#each clip.labels as l (l)}
              <span class="chip gap-1 pr-1">
                {l}
                {#if onupdate}
                  <button class="rounded-full hover:text-white" onclick={() => removeLabel(l)} aria-label="Remove label {l}">
                    <X size={11} />
                  </button>
                {/if}
              </span>
            {/each}
            {#if onupdate}
              {#if addingLabel}
                <!-- svelte-ignore a11y_autofocus -->
                <input
                  bind:this={labelInput}
                  bind:value={labelDraft}
                  class="chip h-5 w-[110px] bg-canvas outline-none focus:border-accent"
                  placeholder="New label"
                  maxlength="40"
                  autofocus
                  onblur={() => {
                    addLabel()
                    addingLabel = false
                  }}
                  onkeydown={(e) => {
                    e.stopPropagation()
                    if (e.key === 'Enter') addLabel()
                    else if (e.key === 'Escape') {
                      labelDraft = ''
                      addingLabel = false
                    }
                  }}
                  aria-label="New label"
                />
              {:else}
                <button class="chip gap-0.5 border-dashed text-muted hover:text-zinc-100" onclick={() => (addingLabel = true)}>
                  <Plus size={11} /> Label
                </button>
              {/if}
            {/if}
          </span>
        {/if}
      </div>
    </div>
    <div class="flex items-center text-[14px] font-medium">
      <button class="action {trimming ? 'text-accent' : ''}" onclick={() => (trimming = !trimming)} title="Trim (T)">
        {trimming ? 'Done' : 'Trim'}
      </button>
      <button class="action" onclick={onshare}>Show in folder</button>
      <button
        class="action {armed ? 'text-danger' : ''}"
        onclick={() => (armed ? ondelete() : (armed = true))}
        onmouseleave={() => (armed = false)}
      >
        {armed ? 'Delete clip?' : 'Delete'}
      </button>
      <button class="btn-icon ml-1" onclick={() => (help = !help)} title="Keyboard shortcuts (?)" aria-label="Keyboard shortcuts">
        <CircleHelp size={18} />
      </button>
    </div>
  </div>

  <!-- Sized to the clip's own aspect ratio, so ultrawide clips have no bars. -->
  <div class="flex min-h-0 flex-1 items-center justify-center [container-type:size]">
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="player group relative overflow-hidden bg-black {fullscreen ? 'fixed inset-0 z-50' : 'rounded-[var(--radius-card)]'}"
      class:hide-cursor={!showControls}
      style:aspect-ratio={fullscreen ? undefined : aspect}
      style:width={fullscreen ? undefined : `min(100cqw, calc(100cqh * ${aspect}))`}
      onpointermove={poke}
      onpointerleave={() => (active = false)}
    >
      <!-- svelte-ignore a11y_media_has_caption, a11y_click_events_have_key_events -->
      <video
        bind:this={video}
        bind:paused
        src={videoUrl(clip.path)}
        onloadedmetadata={loaded}
        ontimeupdate={tick}
        onended={ended}
        onclick={toggle}
        ondblclick={() => setFullscreen(!fullscreen)}
        preload="auto"
        use:playbackStats={clip.path}
        class="size-full object-contain"
      ></video>

      {#if !trimming}
        <div
          class="absolute inset-x-0 bottom-0 bg-gradient-to-t from-black/80 via-black/40 to-transparent px-4 pt-12 pb-2.5 transition-opacity duration-300
            {showControls ? 'opacity-100' : 'pointer-events-none opacity-0'}"
        >
          <Timeline {duration} {time} trim={false} bind:start bind:end onseek={seek} />
          <div class="mt-1 flex items-center gap-1 text-white">
            <button class="glyph" onclick={toggle} aria-label={paused ? 'Play' : 'Pause'} title="Play/Pause (Space)">
              {#if paused}<Play size={20} fill="currentColor" strokeWidth={0} />{:else}<Pause size={20} fill="currentColor" strokeWidth={0} />{/if}
            </button>
            <span class="ml-1 text-[13px] tabular-nums">
              {timecode(time)}<span class="text-white/55"> / {timecode(duration)}</span>
            </span>
            <div class="flex-1"></div>
            <button class="glyph w-auto px-2 text-[13px] font-semibold tabular-nums" onclick={cycleSpeed} title="Playback speed (< >)" aria-label="Playback speed {speed}x">
              {speed}×
            </button>
            <button class="glyph {loop ? 'text-accent' : ''}" onclick={() => (loop = !loop)} title="Repeat (R)" aria-label="Repeat" aria-pressed={loop}>
              <Repeat size={17} />
            </button>
            <div class="volume flex items-center">
              <input
                type="range"
                min="0"
                max="1"
                step="0.05"
                bind:value={volume}
                oninput={() => (muted = false)}
                class="slider w-0 opacity-0 transition-all duration-200"
                style:--fill="{(muted ? 0 : volume) * 100}%"
                aria-label="Volume"
              />
              <button class="glyph" onclick={() => (muted = !muted)} aria-label={muted ? 'Unmute' : 'Mute'} title="Mute (M)">
                {#if muted || volume === 0}<VolumeX size={19} />{:else}<Volume2 size={19} />{/if}
              </button>
            </div>
            <button
              class="glyph"
              onclick={() => setFullscreen(!fullscreen)}
              aria-label={fullscreen ? 'Exit fullscreen' : 'Fullscreen'}
              title="Fullscreen (F)"
            >
              {#if fullscreen}<Minimize size={18} />{:else}<Maximize size={18} />{/if}
            </button>
          </div>
        </div>
      {/if}
    </div>
  </div>

  {#if trimming}
    <div class="space-y-3">
      <div class="flex items-center gap-3">
        <button class="glyph text-white" onclick={toggle} aria-label={paused ? 'Play' : 'Pause'} title="Play/Pause (Space)">
          {#if paused}<Play size={20} fill="currentColor" strokeWidth={0} />{:else}<Pause size={20} fill="currentColor" strokeWidth={0} />{/if}
        </button>
        <span class="w-[132px] flex-none text-[13px] tabular-nums">
          {timecode(time)} <span class="text-muted">/ {timecode(duration)}</span>
        </span>
        <div class="min-w-0 flex-1">
          <Timeline {duration} {time} trim bind:start bind:end onseek={seek} />
        </div>
      </div>
      <div class="flex items-center gap-5 border-t border-line pt-3 text-[13px]">
        <button class="action -ml-2 tabular-nums" onclick={() => (start = Math.min(time, end - 0.1))} title="Set start to playhead (I)">
          <span class="text-muted">In</span>&nbsp;{timecode(start)}
        </button>
        <button class="action -ml-3 tabular-nums" onclick={() => (end = Math.max(time, start + 0.1))} title="Set end to playhead (O)">
          <span class="text-muted">Out</span>&nbsp;{timecode(end)}
        </button>
        {#each levels as _, i (i)}
          <label class="flex items-center gap-2.5">
            <span class="text-muted">{trackName(i)}</span>
            <input type="range" min="0" max="200" step="5" bind:value={levels[i]} class="w-24" style:--fill="{levels[i] / 2}%" />
            <span class="w-10 tabular-nums">{levels[i]}%</span>
          </label>
        {/each}
        <div class="flex-1"></div>
        <span class="text-muted">Length <span class="font-semibold text-zinc-100 tabular-nums">{timecode(end - start)}</span></span>
        <button
          class="btn-primary"
          disabled={rendering || end - start < 0.1}
          onclick={() => onrender(start, end, levels.map((l) => l / 100))}
        >
          {rendering ? 'Rendering…' : 'Render clip'}
        </button>
      </div>
    </div>
  {/if}
</div>

{#if help}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="fixed inset-0 z-[60] flex items-center justify-center bg-black/60 backdrop-blur-[2px]" onclick={() => (help = false)}>
    <div class="card w-[420px] p-5 shadow-2xl shadow-black/60" onclick={(e) => e.stopPropagation()} role="dialog" aria-label="Keyboard shortcuts" tabindex="-1">
      <div class="display-face mb-3 text-[17px] font-semibold">Keyboard shortcuts</div>
      <dl class="grid grid-cols-[auto_1fr] gap-x-5 gap-y-1.5 text-[13px]">
        {#each SHORTCUTS as [keys, what] (keys)}
          <dt class="flex gap-1 whitespace-nowrap">
            {#each keys.split(' / ') as k, i (i)}
              {#if i > 0}<span class="text-muted">/</span>{/if}<kbd>{k}</kbd>
            {/each}
          </dt>
          <dd class="self-center text-zinc-300">{what}</dd>
        {/each}
      </dl>
    </div>
  </div>
{/if}

<style>
  /* Borderless text actions in the header. */
  .action {
    padding: 6px 10px;
    border-radius: var(--radius-ctl);
    color: #c9cbd2;
    transition:
      color 0.12s,
      background 0.12s;
  }

  .action:hover {
    color: #fff;
    background: rgba(255, 255, 255, 0.06);
  }

  /* Bare icon controls on the video. */
  .glyph {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 34px;
    height: 34px;
    opacity: 0.9;
    transition: opacity 0.12s;
  }

  .glyph:hover {
    opacity: 1;
  }

  /* Volume slider slides out next to the speaker icon. */
  .volume:hover .slider,
  .volume:focus-within .slider {
    width: 84px;
    opacity: 1;
    margin-right: 4px;
  }

  .slider::-webkit-slider-thumb {
    width: 10px;
    height: 10px;
  }

  .hide-cursor,
  .hide-cursor video {
    cursor: none;
  }

  /* Seek bar on the video: lighter rail. */
  .player :global(.rail) {
    background: rgba(255, 255, 255, 0.25);
  }
</style>
