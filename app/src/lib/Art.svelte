<!--
  A Steam image from disk. A load can fail: Steam swaps the files when a
  game's art changes, and the first paint at start-up comes from the cache.
  A failed image shows `fallback` until the next library listing arrives,
  then tries again with a fresh request.
-->
<script module lang="ts">
  let listing = $state(0)

  /** A new library listing arrived: failed images try again. */
  export function retryFailedArt() {
    listing++
  }
</script>

<script lang="ts">
  import type { Snippet } from 'svelte'
  import { fileUrl } from './api'

  let {
    path,
    alt = '',
    class: cls = '',
    fallback,
  }: { path: string; alt?: string; class?: string; fallback?: Snippet } = $props()

  let failed = $state<{ path: string; listing: number; count: number } | null>(null)
  const failure = $derived(failed?.path === path ? failed : null)
  // The asset protocol reads only the path, so the query just makes a retry
  // a new request. It changes per failure, not per listing: loaded art
  // isn't fetched again.
  const src = $derived(fileUrl(path) + (failure ? `?retry=${failure.count}` : ''))
</script>

{#if failure?.listing === listing}
  {@render fallback?.()}
{:else}
  <img
    {src}
    {alt}
    draggable="false"
    class={cls}
    onerror={() => (failed = { path, listing, count: (failure?.count ?? 0) + 1 })}
  />
{/if}
