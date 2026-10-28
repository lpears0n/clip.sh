<script lang="ts">
  import type { Clip } from './api'
  import ClipCard from './ClipCard.svelte'
  import { bytes } from './format'

  let {
    clips,
    loading,
    hotkey,
    onopen,
    ondelete,
    onshare,
  }: {
    clips: Clip[]
    loading: boolean
    hotkey: string
    onopen: (c: Clip) => void
    ondelete: (c: Clip) => void
    onshare: (c: Clip) => void
  } = $props()

  let game = $state('')
  const games = $derived([...new Set(clips.map((c) => c.game))].sort((a, b) => a.localeCompare(b)))
  const shown = $derived(game ? clips.filter((c) => c.game === game) : clips)
  const total = $derived(shown.reduce((n, c) => n + c.size_bytes, 0))

  $effect(() => {
    if (game && !games.includes(game)) game = ''
  })
</script>

{#if clips.length}
  <div class="mx-auto max-w-[1600px] space-y-5">
    <div class="flex items-center gap-4">
      <h2 class="display-face text-[24px] font-semibold">Clips</h2>
      <span class="text-[13px] text-muted tabular-nums">{shown.length} · {bytes(total)}</span>
      <div class="flex-1"></div>
      {#if games.length > 1}
        <select class="field w-[220px]" bind:value={game} aria-label="Filter by game">
          <option value="">All games</option>
          {#each games as g (g)}
            <option value={g}>{g}</option>
          {/each}
        </select>
      {/if}
    </div>
    <div class="grid grid-cols-1 gap-x-4 gap-y-5 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
      {#each shown as clip (clip.path)}
        <ClipCard {clip} onopen={() => onopen(clip)} ondelete={() => ondelete(clip)} onshare={() => onshare(clip)} />
      {/each}
    </div>
  </div>
{:else if !loading}
  <div class="flex h-full flex-col items-center justify-center gap-3 text-center">
    <p class="display-face text-[22px] font-semibold">No clips yet.</p>
    <p class="flex items-center gap-2 text-[14px] text-muted">Press <kbd>{hotkey}</kbd> in a game to save one.</p>
  </div>
{/if}
