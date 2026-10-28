<script lang="ts">
  import { Clapperboard, FolderOpen, Scissors, Trash2 } from '@lucide/svelte'
  import { type Clip, fileUrl } from './api'
  import { date, duration } from './format'

  let {
    clip,
    title = clip.game,
    subtitle = date(clip.created_ms),
    onopen,
    ondelete,
    onshare,
  }: {
    clip: Clip
    /** Defaults to the game name; game pages show the time instead. */
    title?: string
    subtitle?: string
    onopen: () => void
    ondelete: () => void
    onshare: () => void
  } = $props()

  // First click arms the delete button, second deletes.
  let armed = $state(false)
</script>

<div class="group min-w-0">
  <div
    class="relative aspect-video cursor-pointer overflow-hidden rounded-[var(--radius-card)] bg-panel ring-1 ring-line transition-shadow ring-inset hover:ring-zinc-500 focus-visible:ring-2 focus-visible:ring-accent focus-visible:outline-none"
    onclick={onopen}
    onkeydown={(e) => e.key === 'Enter' && onopen()}
    onmouseleave={() => (armed = false)}
    role="button"
    tabindex="0"
    aria-label="Open {clip.game} clip from {date(clip.created_ms)}"
  >
    {#if clip.thumbnail}
      <img
        src={fileUrl(clip.thumbnail)}
        alt=""
        loading="lazy"
        draggable="false"
        class="size-full object-cover transition-[filter] duration-200 group-hover:brightness-[0.85]"
      />
    {:else}
      <div class="flex size-full items-center justify-center text-zinc-700">
        <Clapperboard size={28} />
      </div>
    {/if}

    <span class="absolute right-2 bottom-2 rounded-[4px] bg-black/80 px-1.5 py-0.5 text-[12px] font-medium text-white tabular-nums">
      {duration(clip.duration)}
    </span>
    {#if clip.is_edit}
      <span class="absolute bottom-2 left-2 rounded-[4px] bg-black/80 p-1 text-zinc-200" title="Edited clip">
        <Scissors size={12} />
      </span>
    {/if}

    <div class="absolute top-2 right-2 flex gap-1 opacity-0 transition-opacity group-focus-within:opacity-100 group-hover:opacity-100">
      <button
        class="btn-icon bg-black/70 backdrop-blur-sm"
        onclick={(e) => {
          e.stopPropagation()
          onshare()
        }}
        title="Show in folder"
        aria-label="Show in folder"
      >
        <FolderOpen size={16} />
      </button>
      <button
        class="btn-icon {armed ? 'bg-danger text-white hover:bg-danger' : 'bg-black/70 backdrop-blur-sm'}"
        onclick={(e) => {
          e.stopPropagation()
          if (armed) ondelete()
          else armed = true
        }}
        title={armed ? 'Click again to delete' : 'Delete'}
        aria-label={armed ? 'Confirm delete' : 'Delete'}
      >
        <Trash2 size={16} />
      </button>
    </div>
  </div>
  <div class="mt-2 flex items-baseline justify-between gap-3 px-0.5">
    <span class="display-face truncate text-[14px] font-semibold">{title}</span>
    <span class="flex-none text-[12px] text-muted tabular-nums">{subtitle}</span>
  </div>
</div>
