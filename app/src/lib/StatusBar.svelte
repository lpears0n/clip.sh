<!-- Recorder state, in one place: the right side of the top bar. -->
<script lang="ts">
  import { FolderOpen } from '@lucide/svelte'
  import type { Status } from './api'

  let {
    status,
    hotkey,
    saving,
    flash,
    onsave,
    onopen,
    onchoosescreen,
  }: {
    status: Status | null
    hotkey: string
    saving: boolean
    /** Briefly true after a clip is saved; plays the shutter flash. */
    flash: boolean
    onsave: () => void
    onopen: () => void
    onchoosescreen: () => void
  } = $props()

  const recording = $derived(status?.state === 'recording')
</script>

<div class="flex min-w-0 items-center gap-3">
  <button class="btn-icon" onclick={onopen} title="Open a video to trim" aria-label="Open a video to trim">
    <FolderOpen size={18} />
  </button>
  <span class="h-5 w-px bg-line"></span>

  {#if recording}
    <div class="flex min-w-0 items-center gap-2.5">
      <span class="relative flex size-2">
        <span class="absolute inset-0 animate-ping rounded-full bg-danger opacity-60"></span>
        <span class="relative size-2 rounded-full bg-danger"></span>
      </span>
      <span class="display-face max-w-[260px] truncate text-[14px] font-semibold">{status?.game}</span>
      <span class="text-[13px] text-muted tabular-nums">{Math.floor(status?.buffered_seconds ?? 0)}s</span>
    </div>
    <button class="btn-primary relative h-8 overflow-hidden px-3" onclick={onsave} disabled={saving}>
      Clip <kbd class="h-5 border-black/20 bg-black/15 text-black">{hotkey}</kbd>
      {#if flash}
        <span class="shutter absolute inset-0 bg-white"></span>
      {/if}
    </button>
  {:else if status?.needs_screen}
    <button class="btn h-8 border-accent/60 text-accent" onclick={onchoosescreen}>Choose a screen to record</button>
  {:else if status?.state === 'unavailable'}
    <span class="max-w-[420px] truncate text-[13px] text-danger" title={status.error ?? ''}>
      Can't capture: {status.error}
    </span>
  {:else}
    <span class="truncate text-[13px] text-muted">Waiting for a game</span>
  {/if}
</div>

<style>
  .shutter {
    animation: shutter 450ms ease-out forwards;
  }

  @keyframes shutter {
    from {
      opacity: 0.85;
    }
    to {
      opacity: 0;
    }
  }
</style>
