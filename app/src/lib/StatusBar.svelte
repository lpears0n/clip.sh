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
  <span class="h-5 w-px bg-line-strong"></span>

  {#if recording}
    <div class="flex min-w-0 items-center gap-2.5">
      <span class="size-2 flex-none rounded-full bg-danger"></span>
      <span class="display-face max-w-64 truncate text-body font-semibold">{status?.game}</span>
      <span class="text-secondary text-muted tabular-nums">{Math.floor(status?.buffered_seconds ?? 0)}s</span>
    </div>
    <button class="btn-primary relative h-8 overflow-hidden px-3" onclick={onsave} disabled={saving}>
      Clip <kbd class="h-5 border-black/20 bg-black/15 text-on-accent">{hotkey}</kbd>
      {#if flash}
        <span class="shutter absolute inset-0 bg-white"></span>
      {/if}
    </button>
  {:else if status?.needs_screen}
    <button class="btn h-8 border-accent/60 text-accent" onclick={onchoosescreen}>Choose a screen to record</button>
  {:else if status?.state === 'unavailable'}
    <span class="max-w-md truncate text-secondary text-danger" title={status.error ?? ''}>
      Can't capture: {status.error}
    </span>
  {:else}
    <span class="truncate text-secondary text-muted">Waiting for a game</span>
  {/if}
</div>

<style>
  .shutter {
    animation: shutter 400ms ease-out forwards;
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
