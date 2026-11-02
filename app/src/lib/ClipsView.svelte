<script lang="ts">
  import { ArrowDownUp, CheckSquare, Search, Trash2, X } from '@lucide/svelte'
  import type { Clip } from './api'
  import ClipCard from './ClipCard.svelte'
  import { bytes, date } from './format'

  let {
    clips,
    loading,
    hotkey,
    onopen,
    ondelete,
    ondeletemany,
    onshare,
  }: {
    clips: Clip[]
    loading: boolean
    hotkey: string
    onopen: (c: Clip) => void
    ondelete: (c: Clip) => void
    ondeletemany: (cs: Clip[]) => void
    onshare: (c: Clip) => void
  } = $props()

  type Sort = 'newest' | 'oldest' | 'longest' | 'largest'
  const SORTS: { value: Sort; label: string }[] = [
    { value: 'newest', label: 'Newest first' },
    { value: 'oldest', label: 'Oldest first' },
    { value: 'longest', label: 'Longest' },
    { value: 'largest', label: 'Largest file' },
  ]

  let game = $state('')
  let query = $state('')
  let sort = $state<Sort>('newest')
  let search: HTMLInputElement | undefined = $state()

  // Selection mode for bulk actions.
  let selecting = $state(false)
  let selected = $state<Set<string>>(new Set())

  const games = $derived([...new Set(clips.map((c) => c.game))].sort((a, b) => a.localeCompare(b)))

  const shown = $derived.by(() => {
    const q = query.trim().toLowerCase()
    let list = clips.filter((c) => (!game || c.game === game))
    if (q) {
      list = list.filter((c) => `${c.game} ${c.name} ${date(c.created_ms)}`.toLowerCase().includes(q))
    }
    const by: Record<Sort, (a: Clip, b: Clip) => number> = {
      newest: (a, b) => b.created_ms - a.created_ms,
      oldest: (a, b) => a.created_ms - b.created_ms,
      longest: (a, b) => (b.duration ?? 0) - (a.duration ?? 0),
      largest: (a, b) => b.size_bytes - a.size_bytes,
    }
    return [...list].sort(by[sort])
  })
  const total = $derived(shown.reduce((n, c) => n + c.size_bytes, 0))
  const selectedClips = $derived(shown.filter((c) => selected.has(c.path)))
  const selectedBytes = $derived(selectedClips.reduce((n, c) => n + c.size_bytes, 0))

  $effect(() => {
    if (game && !games.includes(game)) game = ''
  })

  // Drop selections for clips that disappeared.
  $effect(() => {
    const paths = new Set(clips.map((c) => c.path))
    if ([...selected].some((p) => !paths.has(p))) {
      selected = new Set([...selected].filter((p) => paths.has(p)))
    }
  })

  function toggle(c: Clip) {
    const next = new Set(selected)
    if (next.has(c.path)) next.delete(c.path)
    else next.add(c.path)
    selected = next
  }

  function selectAll() {
    selected = selected.size === shown.length ? new Set() : new Set(shown.map((c) => c.path))
  }

  function stopSelecting() {
    selecting = false
    selected = new Set()
  }

  function deleteSelected() {
    const cs = selectedClips
    stopSelecting()
    ondeletemany(cs)
  }

  function onkeydown(e: KeyboardEvent) {
    const typing = e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement
    if ((e.ctrlKey || e.metaKey) && e.key === 'f') {
      e.preventDefault()
      search?.focus()
      search?.select()
    } else if (e.key === 'Escape' && !typing) {
      if (selecting) stopSelecting()
      else if (query) query = ''
    } else if (e.key === 'Escape' && e.target === search) {
      query = ''
      search?.blur()
    } else if (selecting && !typing && (e.ctrlKey || e.metaKey) && e.key === 'a') {
      e.preventDefault()
      selectAll()
    } else if (selecting && !typing && e.key === 'Delete' && selected.size) {
      deleteSelected()
    }
  }
</script>

<svelte:window {onkeydown} />

{#if clips.length}
  <div class="mx-auto max-w-[1600px] space-y-5">
    <div class="flex flex-wrap items-center gap-3">
      <h2 class="display-face text-[24px] font-semibold">Clips</h2>
      <span class="text-[13px] text-muted tabular-nums">{shown.length} · {bytes(total)}</span>
      <div class="flex-1"></div>

      <label class="field flex w-[260px] items-center gap-2 px-2.5">
        <Search size={15} class="flex-none text-muted" />
        <input
          bind:this={search}
          bind:value={query}
          type="search"
          placeholder="Search clips… (Ctrl+F)"
          class="min-w-0 flex-1 bg-transparent text-[14px] outline-none placeholder:text-muted"
          aria-label="Search clips"
        />
        {#if query}
          <button class="btn-icon size-6 text-muted" onclick={() => (query = '')} aria-label="Clear search">
            <X size={14} />
          </button>
        {/if}
      </label>

      {#if games.length > 1}
        <select class="field w-[200px]" bind:value={game} aria-label="Filter by game">
          <option value="">All games</option>
          {#each games as g (g)}
            <option value={g}>{g}</option>
          {/each}
        </select>
      {/if}

      <label class="field flex items-center gap-2 pr-0 pl-2.5">
        <ArrowDownUp size={15} class="flex-none text-muted" />
        <select class="h-full bg-transparent pr-7 text-[14px] outline-none" bind:value={sort} aria-label="Sort clips">
          {#each SORTS as s (s.value)}
            <option value={s.value}>{s.label}</option>
          {/each}
        </select>
      </label>

      <button
        class="btn {selecting ? 'border-accent/60 text-accent' : ''}"
        onclick={() => (selecting ? stopSelecting() : (selecting = true))}
        title={selecting ? 'Done selecting (Esc)' : 'Select clips'}
      >
        <CheckSquare size={16} />
        {selecting ? 'Done' : 'Select'}
      </button>
    </div>

    {#if shown.length}
      <div class="clip-grid grid gap-x-4 gap-y-5">
        {#each shown as clip (clip.path)}
          <ClipCard
            {clip}
            selectable={selecting}
            selected={selected.has(clip.path)}
            onopen={() => onopen(clip)}
            ondelete={() => ondelete(clip)}
            onshare={() => onshare(clip)}
            onselect={() => toggle(clip)}
          />
        {/each}
      </div>
    {:else}
      <div class="flex flex-col items-center gap-2 py-20 text-center">
        <p class="display-face text-[18px] font-semibold">Nothing matches.</p>
        <p class="text-[14px] text-muted">Try another search or clear the game filter.</p>
      </div>
    {/if}
  </div>

  {#if selecting}
    <div
      class="fixed bottom-6 left-1/2 z-40 flex -translate-x-1/2 items-center gap-3 rounded-full border border-line bg-raised py-2 pr-2 pl-5 shadow-2xl shadow-black/60"
      role="toolbar"
      aria-label="Selection actions"
    >
      <span class="text-[14px] font-medium tabular-nums">
        {selected.size} selected
        {#if selected.size}<span class="text-muted"> · {bytes(selectedBytes)}</span>{/if}
      </span>
      <button class="btn h-8 rounded-full px-3 text-[13px]" onclick={selectAll}>
        {selected.size === shown.length ? 'Select none' : 'Select all'}
      </button>
      <button
        class="btn-primary h-8 rounded-full bg-danger px-3 text-[13px] text-white hover:bg-danger/90"
        disabled={!selected.size}
        onclick={deleteSelected}
      >
        <Trash2 size={15} />
        Delete
      </button>
      <button class="btn-icon rounded-full" onclick={stopSelecting} aria-label="Cancel selection" title="Cancel (Esc)">
        <X size={16} />
      </button>
    </div>
  {/if}
{:else if !loading}
  <div class="flex h-full flex-col items-center justify-center gap-3 text-center">
    <p class="display-face text-[22px] font-semibold">No clips yet.</p>
    <p class="flex items-center gap-2 text-[14px] text-muted">Press <kbd>{hotkey}</kbd> in a game to save one.</p>
  </div>
{/if}
