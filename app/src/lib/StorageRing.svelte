<!--
  Storage used by clips, at the bottom of the rail (where Medal keeps its
  storage indicator). Click for a breakdown.
-->
<script lang="ts">
  import type { Usage } from './api'
  import { bytes, storageFraction } from './format'

  let { usage, onmanage }: { usage: Usage | null; onmanage: () => void } = $props()

  let open = $state(false)

  const R = 12.5
  const C = 2 * Math.PI * R
  const fraction = $derived(usage ? storageFraction(usage) : 0)
  const full = $derived(fraction >= 0.9)
  /** Short label under the ring: "4.2G" / "830M". */
  const short = $derived(usage ? bytes(usage.clips_bytes).replace(/ ([KMGT])B$/, '$1') : '')
</script>

<svelte:window onclick={() => (open = false)} />

<div class="relative">
  <button
    class="flex w-12 flex-col items-center gap-0.5 rounded-[var(--radius-card)] py-1.5 transition-colors hover:bg-white/[0.04]"
    aria-label="Storage: {usage ? bytes(usage.clips_bytes) : 'unknown'} used"
    onclick={(e) => {
      e.stopPropagation()
      open = !open
    }}
  >
    <svg width="26" height="26" viewBox="0 0 34 34" class="-rotate-90">
      <circle cx="17" cy="17" r={R} fill="none" stroke="#34363d" stroke-width="4.5" />
      <circle
        cx="17"
        cy="17"
        r={R}
        fill="none"
        stroke={full ? 'var(--color-danger)' : 'var(--color-accent)'}
        stroke-width="4.5"
        stroke-linecap="round"
        stroke-dasharray={C}
        stroke-dashoffset={C * (1 - Math.max(fraction, usage?.clips_bytes ? 0.04 : 0))}
        class="transition-[stroke-dashoffset] duration-700"
      />
    </svg>
    <span class="text-[11px] font-medium text-muted tabular-nums">{short}</span>
  </button>

  {#if open && usage}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div
      class="absolute bottom-0 left-full z-30 ml-3 w-72 rounded-[var(--radius-card)] border border-line bg-raised p-4 shadow-xl shadow-black/50"
      onclick={(e) => e.stopPropagation()}
    >
      <div class="flex items-baseline justify-between">
        <span class="display-face text-[18px] font-semibold tabular-nums">{bytes(usage.clips_bytes)}</span>
        <span class="text-[13px] text-muted">
          {usage.limit_bytes != null ? `of ${bytes(usage.limit_bytes)}` : 'no limit'}
        </span>
      </div>
      <div class="mt-3 h-1.5 overflow-hidden rounded-full bg-zinc-800">
        <div
          class="h-full rounded-full {full ? 'bg-danger' : 'bg-accent'}"
          style:width="{Math.max(fraction * 100, usage.clips_bytes ? 1 : 0)}%"
        ></div>
      </div>
      <p class="mt-2 text-[13px] text-muted tabular-nums">
        {usage.clip_count} clip{usage.clip_count === 1 ? '' : 's'} · {bytes(usage.disk_free)} free on drive
      </p>
      <button
        class="btn mt-4 h-8 w-full text-[13px]"
        onclick={() => {
          open = false
          onmanage()
        }}
      >
        Storage settings
      </button>
    </div>
  {/if}
</div>
