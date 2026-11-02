<!-- A settings row: title and description on the left, control on the right. -->
<script lang="ts">
  import type { Snippet } from 'svelte'

  let {
    title,
    description,
    disabled = false,
    children,
  }: { title: string; description?: string; disabled?: boolean; children: Snippet } = $props()
</script>

<div class="flex min-h-15 items-center gap-6 px-4 py-3 {disabled ? 'opacity-45' : ''}">
  <div class="min-w-0 flex-1">
    <div class="text-body font-medium text-fg">{title}</div>
    {#if description}
      <div class="mt-0.5 text-secondary text-muted">{description}</div>
    {/if}
  </div>
  <div class="flex flex-none items-center gap-2">
    {@render children()}
  </div>
</div>
