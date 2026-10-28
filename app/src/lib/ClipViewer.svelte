<!--
  Watch a clip, or trim it (in/out, audio mix, render) on the same screen.
  Playback controls overlay the video's bottom edge and hide while playing.
-->
<script lang="ts">
  import { ArrowLeft, Maximize, Minimize, Pause, Play, Volume2, VolumeX } from '@lucide/svelte'
  import { getCurrentWindow } from '@tauri-apps/api/window'
  import { type Clip, videoUrl } from './api'
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
  }: {
    clip: Clip
    rendering: boolean
    onback: () => void
    onrender: (start: number, end: number, volumes: number[]) => void
    ondelete: () => void
    onshare: () => void
  } = $props()

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
      video.pause()
      video.currentTime = start
    }
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
    if (e.code === 'Space' || e.code === 'KeyK') {
      e.preventDefault()
      toggle()
    } else if (e.code === 'KeyF') {
      setFullscreen(!fullscreen)
    } else if (e.code === 'KeyM') {
      muted = !muted
    } else if (e.code === 'Escape') {
      if (fullscreen) setFullscreen(false)
      else if (trimming) trimming = false
      else onback()
    } else if (e.code === 'ArrowLeft') {
      seek(Math.max(0, time - 5))
      poke()
    } else if (e.code === 'ArrowRight') {
      seek(Math.min(duration, time + 5))
      poke()
    } else if (trimming && e.code === 'KeyI') {
      start = Math.min(time, end - 0.1)
    } else if (trimming && e.code === 'KeyO') {
      end = Math.max(time, start + 0.1)
    }
  }

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
      <div class="display-face truncate text-[17px] font-semibold">{clip.game}</div>
      <div class="text-[13px] text-muted tabular-nums">
        {dayLabel(clip.created_ms)}, {clock(clip.created_ms)} · {size(clip.size_bytes)}{clip.is_edit ? ' · edited' : ''}
      </div>
    </div>
    <div class="flex items-center text-[14px] font-medium">
      <button class="action {trimming ? 'text-accent' : ''}" onclick={() => (trimming = !trimming)}>
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
