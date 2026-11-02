<script lang="ts">
  import { hotkeyFromEvent } from './format'

  let {
    value,
    onbind,
  }: { value: string; onbind: (keys: string) => Promise<void> } = $props()

  let listening = $state(false)
  let error = $state<string | null>(null)

  async function onkeydown(e: KeyboardEvent) {
    if (!listening) return
    e.preventDefault()
    e.stopPropagation()
    if (e.code === 'Escape') {
      listening = false
      return
    }
    const keys = hotkeyFromEvent(e)
    if (!keys) return
    listening = false
    try {
      await onbind(keys)
      error = null
    } catch (err) {
      error = String(err)
    }
  }
</script>

<svelte:window onkeydowncapture={onkeydown} />

<div class="flex flex-col items-end gap-1.5">
  <button
    class="field flex min-w-48 items-center justify-center gap-1 {listening ? 'border-accent text-accent' : ''}"
    onclick={() => {
      listening = !listening
      error = null
    }}
    onblur={() => (listening = false)}
  >
    {#if listening}
      <span class="text-secondary font-medium">Press a key combination…</span>
    {:else}
      {#each value.split('+') as k, i (i)}
        {#if i > 0}<span class="text-muted">+</span>{/if}
        <kbd>{k}</kbd>
      {/each}
    {/if}
  </button>
  {#if error}
    <span class="text-caption text-danger">{error}</span>
  {/if}
</div>
