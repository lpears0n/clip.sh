<!--
  The progress line over the video: a thin rail, the played part in the
  accent, a dot at the playhead. Trimming happens on the same line: the
  kept range is brighter and two small markers set in and out. Moments
  marked while recording show as ticks; clicking one jumps there. Click or
  drag anywhere else to seek.
-->
<script lang="ts">
  let {
    duration,
    time,
    trim,
    start = $bindable(),
    end = $bindable(),
    markers = [],
    onseek,
  }: {
    duration: number
    time: number
    trim: boolean
    start: number
    end: number
    /** Marked moments, in seconds. */
    markers?: number[]
    onseek: (t: number) => void
  } = $props()

  const MIN = 0.1
  let track: HTMLDivElement
  let drag = $state<'start' | 'end' | 'seek' | null>(null)

  const frac = (t: number) => (duration > 0 ? Math.min(1, Math.max(0, t / duration)) : 0)
  const pct = (t: number) => `${frac(t) * 100}%`

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
      onseek(trim ? Math.min(end, Math.max(start, t)) : t)
    }
  }

  function up() {
    drag = null
  }
</script>

<div
  class="track"
  class:dragging={drag !== null}
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
  <div class="rail">
    {#if trim}
      <div class="kept" style:left={pct(start)} style:width="{(frac(end) - frac(start)) * 100}%"></div>
      <div class="played" style:left={pct(start)} style:width="{Math.max(0, frac(Math.min(time, end)) - frac(start)) * 100}%"></div>
    {:else}
      <div class="played" style:left="0" style:width={pct(time)}></div>
    {/if}
  </div>
  {#each markers as m, i (i)}
    <button
      class="tick"
      style:left={pct(m)}
      onpointerdown={(e) => {
        e.stopPropagation()
        onseek(m)
      }}
      aria-label="Marked moment at {m.toFixed(1)} s"
      title="Marked moment"
      tabindex="-1"
    ></button>
  {/each}
  {#if trim}
    <div
      class="marker"
      style:left={pct(start)}
      onpointerdown={(e) => down(e, 'start')}
      role="slider"
      aria-label="Trim start"
      aria-valuemin={0}
      aria-valuemax={duration}
      aria-valuenow={start}
      tabindex="-1"
    ></div>
    <div
      class="marker"
      style:left={pct(end)}
      onpointerdown={(e) => down(e, 'end')}
      role="slider"
      aria-label="Trim end"
      aria-valuemin={0}
      aria-valuemax={duration}
      aria-valuenow={end}
      tabindex="-1"
    ></div>
  {/if}
  <div class="dot" style:left={pct(time)}></div>
</div>

<style>
  /* Tall enough to grab; the visible line is thin and centred in it. */
  .track {
    position: relative;
    height: 18px;
    cursor: pointer;
    touch-action: none;
  }

  .rail {
    position: absolute;
    top: 50%;
    left: 0;
    right: 0;
    height: 3px;
    transform: translateY(-50%);
    border-radius: 999px;
    background: rgb(255 255 255 / 22%);
    transition: height var(--duration-fast);
  }

  .track:hover .rail,
  .track.dragging .rail {
    height: 5px;
  }

  .kept {
    position: absolute;
    top: 0;
    bottom: 0;
    background: rgb(255 255 255 / 45%);
  }

  .played {
    position: absolute;
    top: 0;
    bottom: 0;
    border-radius: inherit;
    background: var(--color-accent);
  }

  .dot {
    position: absolute;
    top: 50%;
    width: 11px;
    height: 11px;
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 0 0 1px rgb(0 0 0 / 25%);
    transform: translate(-50%, -50%);
    pointer-events: none;
    transition:
      width var(--duration-fast),
      height var(--duration-fast);
  }

  .track:hover .dot,
  .track.dragging .dot {
    width: 14px;
    height: 14px;
  }

  /* Marked moments: short white ticks standing above the line, with a
     wider invisible hit area. */
  .tick {
    position: absolute;
    top: 0;
    width: 10px;
    height: 50%;
    padding: 0;
    border: 0;
    background: none;
    transform: translateX(-50%);
    cursor: pointer;
  }

  .tick::after {
    content: '';
    position: absolute;
    left: 50%;
    bottom: 1px;
    width: 2px;
    height: 7px;
    border-radius: 1px;
    background: #fff;
    box-shadow: 0 0 0 1px rgb(0 0 0 / 30%);
    transform: translateX(-50%);
  }

  /* In/out markers: small accent pills across the line, with a wider
     invisible grab area around each. */
  .marker {
    position: absolute;
    top: 50%;
    width: 16px;
    height: 22px;
    transform: translate(-50%, -50%);
    cursor: ew-resize;
  }

  .marker::after {
    content: '';
    position: absolute;
    left: 50%;
    top: 50%;
    width: 4px;
    height: 14px;
    border-radius: 2px;
    background: var(--color-accent);
    box-shadow: 0 0 0 1px rgb(0 0 0 / 35%);
    transform: translate(-50%, -50%);
  }
</style>
