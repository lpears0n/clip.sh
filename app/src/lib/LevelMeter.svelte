<!--
  A live level bar for one audio device in Settings → Audio, so it is plain
  what a recording would hear: moving with sound, flat with silence, or the
  reason the device didn't open.
-->
<script lang="ts">
  let {
    peakDb,
    error = null,
    label,
  }: {
    /** Loudest channel's peak in dBFS; null before the first reading. */
    peakDb: number | null
    error?: string | null
    label: string
  } = $props()

  const FLOOR = -60
  const fill = $derived(peakDb == null ? 0 : Math.min(1, Math.max(0, (peakDb - FLOOR) / -FLOOR)))
  const silent = $derived(peakDb != null && peakDb < -55)
  const hot = $derived(peakDb != null && peakDb > -6)
  const clipping = $derived(peakDb != null && peakDb > -1)
</script>

<div class="w-full" role="meter" aria-label="{label} level" aria-valuemin={FLOOR} aria-valuemax={0} aria-valuenow={peakDb ?? FLOOR}>
  <div class="relative h-1.5 overflow-hidden rounded-full bg-line-strong">
    <div
      class="meter-fill absolute inset-0 rounded-full {clipping ? 'bg-danger' : hot ? 'bg-warn' : 'bg-ok'}"
      style:transform="scaleX({fill})"
    ></div>
  </div>
  <div class="mt-1 h-4 truncate text-caption {error ? 'text-danger' : 'text-muted'}" title={error ?? ''}>
    {#if error}
      Can't open: {error}
    {:else if peakDb == null}
      Listening…
    {:else if silent}
      Silent. Play some sound to test.
    {:else if clipping}
      Clipping: too loud
    {:else}
      Hearing sound
    {/if}
  </div>
</div>

<style>
  .meter-fill {
    transform-origin: left center;
    transition: transform 80ms linear;
    will-change: transform;
  }
  .bg-warn {
    background: oklch(80% 0.15 85);
  }
  @media (prefers-reduced-motion: reduce) {
    .meter-fill {
      transition: none;
    }
  }
</style>
