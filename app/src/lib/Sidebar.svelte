<!-- Labelled navigation with the storage summary at the foot. Collapses to icons (Ctrl+B). -->
<script lang="ts">
  import { Clapperboard, Gamepad2, HardDrive, Settings as Gear } from '@lucide/svelte'
  import type { Component } from 'svelte'
  import type { Usage } from './api'
  import { bytes, storageFraction } from './format'

  export type NavId = 'games' | 'clips' | 'settings'

  let {
    active,
    collapsed,
    usage,
    onnavigate,
    onstorage,
  }: {
    active: NavId
    collapsed: boolean
    usage: Usage | null
    onnavigate: (id: NavId) => void
    onstorage: () => void
  } = $props()

  const items: { id: NavId; label: string; icon: Component<{ size?: number; strokeWidth?: number }>; key: string }[] = [
    { id: 'games', label: 'Games', icon: Gamepad2, key: '1' },
    { id: 'clips', label: 'All clips', icon: Clapperboard, key: '2' },
    { id: 'settings', label: 'Settings', icon: Gear, key: '3' },
  ]

  const fraction = $derived(usage ? storageFraction(usage) : 0)
  const full = $derived(fraction >= 0.9)
  const storageLine = $derived(
    usage ? (usage.limit_bytes != null ? `${bytes(usage.clips_bytes)} of ${bytes(usage.limit_bytes)}` : `${bytes(usage.clips_bytes)} · ${bytes(usage.disk_free)} free`) : '',
  )
</script>

<aside
  class="flex flex-none flex-col border-r border-line bg-panel transition-[width] duration-normal {collapsed ? 'w-16' : 'w-(--spacing-sidebar)'}"
  aria-label="Navigation"
>
  <div class="flex h-14 flex-none items-center {collapsed ? 'justify-center' : 'px-5'}">
    <span class="display-face text-title font-bold">
      {#if collapsed}c<span class="text-accent">.</span>{:else}clip<span class="text-accent">.sh</span>{/if}
    </span>
  </div>

  <nav class="flex flex-col gap-1 px-3">
    {#each items as item (item.id)}
      {@const Icon = item.icon}
      {@const current = active === item.id}
      <button
        class="group flex h-9 items-center gap-3 rounded-ctl text-body font-medium transition-colors duration-fast
          {collapsed ? 'justify-center' : 'px-3'}
          {current ? 'bg-raised text-fg' : 'text-fg-2 hover:bg-raised hover:text-fg'}"
        aria-current={current ? 'page' : undefined}
        title={collapsed ? `${item.label} (${item.key})` : undefined}
        onclick={() => onnavigate(item.id)}
      >
        <Icon size={18} strokeWidth={current ? 2.1 : 1.9} />
        {#if !collapsed}
          <span class="flex-1 truncate text-left">{item.label}</span>
          <kbd class="h-4 min-w-4 px-1 opacity-0 transition-opacity duration-fast group-hover:opacity-100">{item.key}</kbd>
        {/if}
      </button>
    {/each}
  </nav>

  <div class="flex-1"></div>

  <button
    class="group mx-3 mb-3 rounded-ctl p-3 text-left transition-colors duration-fast hover:bg-raised {collapsed ? 'flex justify-center' : ''}"
    onclick={onstorage}
    aria-label="Storage: {usage ? bytes(usage.clips_bytes) : 'unknown'} used. Open storage settings"
    title={collapsed ? storageLine : 'Storage settings'}
  >
    {#if collapsed}
      <HardDrive size={18} class={full ? 'text-danger' : 'text-fg-2'} />
    {:else}
      <div class="flex items-center gap-2 text-secondary text-fg-2">
        <HardDrive size={14} />
        <span class="flex-1">Storage</span>
        {#if usage}<span class="tabular-nums">{Math.round(fraction * 100)}%</span>{/if}
      </div>
      <div class="mt-2 h-1 overflow-hidden rounded-full bg-line-strong">
        <div
          class="h-full rounded-full transition-[width] duration-normal {full ? 'bg-danger' : 'bg-accent'}"
          style:width="{Math.max(fraction * 100, usage?.clips_bytes ? 1 : 0)}%"
        ></div>
      </div>
      <div class="mt-1.5 truncate text-caption text-muted tabular-nums">{storageLine}</div>
    {/if}
  </button>
</aside>
