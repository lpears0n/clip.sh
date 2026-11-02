<script lang="ts">
  import { Check, Clapperboard, FolderOpen, Scissors, Star, Trash2 } from '@lucide/svelte'
  import { type Clip, fileUrl, videoUrl } from './api'
  import { date, duration } from './format'
  import { prefs } from './prefs.svelte'

  let {
    clip,
    fallback = clip.game,
    subtitle = date(clip.created_ms),
    selectable = false,
    selected = false,
    onopen,
    ondelete,
    onshare,
    onselect,
    onfavorite,
  }: {
    clip: Clip
    /** Shown when the clip has no title: the game name, or the time on game pages. */
    fallback?: string
    subtitle?: string
    /** Selection mode: clicking toggles selection instead of opening. */
    selectable?: boolean
    selected?: boolean
    onopen: () => void
    ondelete: () => void
    onshare: () => void
    onselect?: () => void
    /** Absent for clips outside the library, which can't be starred. */
    onfavorite?: () => void
  } = $props()

  const title = $derived(clip.title ?? fallback)

  // First click arms the delete button, second deletes.
  let armed = $state(false)

  // Hover preview: after the pointer rests a moment, a muted video plays
  // in place of the thumbnail (Medal calls these quick previews).
  let previewing = $state(false)
  let previewTimer: ReturnType<typeof setTimeout>

  function enter() {
    if (!prefs.hoverPreviews || prefs.reduceMotion) return
    clearTimeout(previewTimer)
    previewTimer = setTimeout(() => (previewing = true), 450)
  }

  function leave() {
    clearTimeout(previewTimer)
    previewing = false
    armed = false
  }

  function activate() {
    if (selectable) onselect?.()
    else onopen()
  }

  $effect(() => () => clearTimeout(previewTimer))
</script>

<div class="group min-w-0">
  <div
    class="relative aspect-video cursor-pointer overflow-hidden rounded-card bg-panel ring-1 transition-shadow duration-fast ring-inset focus-visible:ring-2 focus-visible:ring-accent focus-visible:outline-none
      {selected ? 'ring-2 ring-accent' : 'ring-line hover:ring-line-strong'}"
    onclick={activate}
    onkeydown={(e) => e.key === 'Enter' && activate()}
    onmouseenter={enter}
    onmouseleave={leave}
    role="button"
    tabindex="0"
    aria-pressed={selectable ? selected : undefined}
    aria-label="{selectable ? 'Select' : 'Open'} {clip.game} clip from {date(clip.created_ms)}"
  >
    {#if clip.thumbnail}
      <img
        src={fileUrl(clip.thumbnail)}
        alt=""
        loading="lazy"
        draggable="false"
        class="size-full object-cover"
      />
    {:else}
      <div class="thumb-empty flex size-full items-end justify-start p-3 text-muted/50">
        <Clapperboard size={20} strokeWidth={1.6} />
      </div>
    {/if}

    {#if previewing}
      <!-- svelte-ignore a11y_media_has_caption -->
      <video
        src={videoUrl(clip.path)}
        class="absolute inset-0 size-full object-cover"
        muted
        autoplay
        loop
        playsinline
        preload="metadata"
        draggable="false"
      ></video>
    {/if}

    <span class="absolute right-2 bottom-2 rounded-chip bg-black/70 px-1.5 py-0.5 text-caption font-medium text-white tabular-nums">
      {duration(clip.duration)}
    </span>
    {#if clip.is_edit}
      <span class="absolute bottom-2 left-2 rounded-chip bg-black/70 p-1 text-fg-2" title="Edited clip">
        <Scissors size={12} />
      </span>
    {/if}

    {#if selectable}
      <span
        class="absolute top-2 left-2 flex size-6 items-center justify-center rounded-full border-2 transition-colors duration-fast
          {selected ? 'border-accent bg-accent text-on-accent' : 'border-white/70 bg-black/50 text-transparent'}"
        aria-hidden="true"
      >
        <Check size={14} strokeWidth={3} />
      </span>
    {:else}
      {#if onfavorite}
        <button
          class="btn-scrim absolute top-2 left-2 transition-opacity duration-fast
            {clip.favorite ? 'text-accent' : 'opacity-0 group-focus-within:opacity-100 group-hover:opacity-100'}"
          onclick={(e) => {
            e.stopPropagation()
            onfavorite()
          }}
          title={clip.favorite ? 'Remove from favourites' : 'Add to favourites'}
          aria-label={clip.favorite ? 'Remove from favourites' : 'Add to favourites'}
          aria-pressed={clip.favorite}
        >
          <Star size={16} fill={clip.favorite ? 'currentColor' : 'none'} />
        </button>
      {/if}
      <div class="absolute top-2 right-2 flex gap-1 opacity-0 transition-opacity duration-fast group-focus-within:opacity-100 group-hover:opacity-100">
        <button
          class="btn-scrim"
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
          class="btn-scrim {armed ? 'bg-danger text-white hover:bg-danger' : ''}"
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
    {/if}
  </div>
  <div class="mt-2 flex items-baseline justify-between gap-3 px-0.5">
    <span class="display-face truncate text-body font-semibold" title={clip.title ? fallback : undefined}>{title}</span>
    <span class="flex-none text-caption text-muted tabular-nums">{subtitle}</span>
  </div>
  {#if clip.labels.length}
    <div class="mt-1.5 flex flex-wrap gap-1 px-0.5">
      {#each clip.labels.slice(0, 3) as l (l)}
        <span class="chip">{l}</span>
      {/each}
      {#if clip.labels.length > 3}
        <span class="chip" title={clip.labels.slice(3).join(', ')}>+{clip.labels.length - 3}</span>
      {/if}
    </div>
  {/if}
</div>
