<!-- Icon-only rail button with a tooltip to its right. -->
<script lang="ts">
  import type { Component } from 'svelte'

  let {
    icon: Icon,
    label,
    active,
    onclick,
  }: {
    icon: Component<{ size?: number; strokeWidth?: number }>
    label: string
    active: boolean
    onclick: () => void
  } = $props()
</script>

<button
  class="group relative flex size-11 items-center justify-center rounded-[var(--radius-card)] transition-colors
    {active ? 'bg-raised text-white' : 'text-muted hover:bg-white/[0.04] hover:text-zinc-100'}"
  aria-label={label}
  aria-current={active ? 'page' : undefined}
  {onclick}
>
  <Icon size={21} strokeWidth={active ? 2.1 : 1.85} />
  <span
    class="pointer-events-none absolute left-full z-30 ml-3 rounded-[var(--radius-ctl)] border border-line bg-raised px-2.5 py-1 text-[13px] font-medium whitespace-nowrap text-zinc-100 opacity-0 shadow-lg shadow-black/40 transition-opacity delay-150 group-hover:opacity-100"
  >
    {label}
  </span>
</button>
