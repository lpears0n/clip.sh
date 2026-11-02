<!-- A row of mutually exclusive options. -->
<script lang="ts" generics="T extends string">
  let {
    options,
    value,
    onchange,
    label,
  }: {
    options: { value: T; label: string; disabled?: boolean; title?: string }[]
    value: T | null
    onchange: (v: T) => void
    label: string
  } = $props()
</script>

<div class="inline-flex rounded-ctl border border-line bg-canvas p-0.5" role="radiogroup" aria-label={label}>
  {#each options as o (o.value)}
    <button
      role="radio"
      aria-checked={value === o.value}
      disabled={o.disabled}
      title={o.title}
      class="h-8 rounded-chip px-3 text-secondary font-medium transition-colors duration-fast disabled:cursor-default disabled:opacity-35
        {value === o.value ? 'bg-overlay text-fg' : 'text-muted hover:text-fg'}"
      onclick={() => onchange(o.value)}
    >
      {o.label}
    </button>
  {/each}
</div>
