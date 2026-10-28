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

<div class="inline-flex rounded-[var(--radius-ctl)] border border-line bg-canvas p-0.5" role="radiogroup" aria-label={label}>
  {#each options as o (o.value)}
    <button
      role="radio"
      aria-checked={value === o.value}
      disabled={o.disabled}
      title={o.title}
      class="h-8 rounded-[4px] px-3 text-[13px] font-medium transition-colors disabled:cursor-default disabled:opacity-35
        {value === o.value ? 'bg-zinc-700 text-white shadow-sm' : 'text-muted hover:text-zinc-100'}"
      onclick={() => onchange(o.value)}
    >
      {o.label}
    </button>
  {/each}
</div>
