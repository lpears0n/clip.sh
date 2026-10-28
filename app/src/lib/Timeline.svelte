<!--
  Seek bar. In trim mode it becomes a trim track: drag the handles to set
  in/out; click or drag elsewhere to seek.
-->
<script lang="ts">
  let {
    duration,
    time,
    trim,
    start = $bindable(),
    end = $bindable(),
    onseek,
  }: {
    duration: number
    time: number
    trim: boolean
    start: number
    end: number
    onseek: (t: number) => void
  } = $props()

  const MIN = 0.1
  let track: HTMLDivElement
  let drag: 'start' | 'end' | 'seek' | null = null

  const pct = (t: number) => `${(duration > 0 ? Math.min(1, Math.max(0, t / duration)) : 0) * 100}%`

  function at(e: PointerEvent) {
    const r = track.getBoundingClientRect()
    return Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)) * duration
  }

  function down(e: PointerEvent, what: 'start' | 'end' | 'seek') {
    e.stopPropagation()
    drag = what
    track.setPointerCapture(e.pointerId)
    move(e)
  }

  function move(e: PointerEvent) {
    if (!drag) return
    const t = at(e)
    if (drag === 'start') {
      start = Math.min(t, end - MIN)
      onseek(start)
    } else if (drag === 'end') {
      end = Math.max(t, start + MIN)
      onseek(end)
    } else {
      onseek(t)
    }
  }

  function up() {
    drag = null
  }
</script>

<div
  class="track"
  class:trim
  bind:this={track}
  onpointerdown={(e) => down(e, 'seek')}
  onpointermove={move}
  onpointerup={up}
  onpointercancel={up}
  role="slider"
  aria-label="Timeline"
  aria-valuemin={0}
  aria-valuemax={duration}
  aria-valuenow={time}
  tabindex="-1"
>
  {#if trim}
    <div class="dim" style:left="0" style:width={pct(start)}></div>
    <div class="dim" style:left={pct(end)} style:right="0"></div>
    <div class="range" style:left={pct(start)} style:width={pct(end - start)}></div>
    <div class="handle" style:left={pct(start)} onpointerdown={(e) => down(e, 'start')} role="presentation"></div>
    <div class="handle end" style:left={pct(end)} onpointerdown={(e) => down(e, 'end')} role="presentation"></div>
    <div class="playhead" style:left={pct(time)}></div>
  {:else}
    <div class="rail"><div class="played" style:width={pct(time)}></div></div>
    <div class="thumb" style:left={pct(time)}></div>
  {/if}
</div>

<style>
  .track {
    position: relative;
    height: 20px;
    cursor: pointer;
    touch-action: none;
  }

  /* Seek bar */
  .rail {
    position: absolute;
    top: 8px;
    left: 0;
    right: 0;
    height: 4px;
    border-radius: 2px;
    background: #34363d;
    overflow: hidden;
    transition:
      height 0.12s,
      top 0.12s;
  }

  .track:hover .rail {
    top: 7px;
    height: 6px;
  }

  .played {
    height: 100%;
    background: var(--color-accent);
  }

  .thumb {
    position: absolute;
    top: 4px;
    width: 12px;
    height: 12px;
    margin-left: -6px;
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.5);
    opacity: 0;
    transition: opacity 0.12s;
    pointer-events: none;
  }

  .track:hover .thumb {
    opacity: 1;
  }

  /* Trim track */
  .track.trim {
    height: 48px;
    border-radius: 6px;
    background: #0d0d0f;
    border: 1px solid #26272c;
  }

  .dim {
    position: absolute;
    top: 0;
    bottom: 0;
    background: rgba(0, 0, 0, 0.55);
  }

  .range {
    position: absolute;
    top: 0;
    bottom: 0;
    border-top: 2px solid var(--color-accent);
    border-bottom: 2px solid var(--color-accent);
    background: rgba(255, 122, 26, 0.08);
  }

  .handle {
    position: absolute;
    top: -1px;
    bottom: -1px;
    width: 10px;
    margin-left: -10px;
    background: var(--color-accent);
    border-radius: 4px 0 0 4px;
    cursor: ew-resize;
  }

  .handle.end {
    margin-left: 0;
    border-radius: 0 4px 4px 0;
  }

  .playhead {
    position: absolute;
    top: -4px;
    bottom: -4px;
    width: 2px;
    margin-left: -1px;
    background: #fff;
    pointer-events: none;
  }
</style>
