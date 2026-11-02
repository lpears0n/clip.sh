<!-- Empty and error states: one line, one action. -->
<script lang="ts">
  import type { Component, Snippet } from 'svelte'

  let {
    icon: Icon,
    title,
    body,
    error = false,
    children,
  }: {
    icon?: Component<{ size?: number; strokeWidth?: number }>
    title: string
    body?: Snippet | string
    error?: boolean
    children?: Snippet
  } = $props()
</script>

<div class="flex h-full flex-col items-center justify-center gap-2 py-16 text-center">
  {#if Icon}
    <span class="mb-2 flex size-12 items-center justify-center rounded-full {error ? 'bg-danger/15 text-danger' : 'bg-raised text-fg-2'}">
      <Icon size={22} strokeWidth={1.8} />
    </span>
  {/if}
  <p class="display-face text-page font-semibold">{title}</p>
  {#if typeof body === 'string'}
    <p class="max-w-md text-body text-muted">{body}</p>
  {:else if body}
    <p class="flex max-w-md items-center gap-2 text-body text-muted">{@render body()}</p>
  {/if}
  {#if children}
    <div class="mt-3 flex items-center gap-2">{@render children()}</div>
  {/if}
</div>
