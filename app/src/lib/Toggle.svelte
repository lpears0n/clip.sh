<script lang="ts">
  let {
    checked,
    onchange,
    label,
    disabled = false,
  }: { checked: boolean; onchange: (v: boolean) => void; label: string; disabled?: boolean } = $props()
</script>

<button
  role="switch"
  aria-checked={checked}
  aria-label={label}
  {disabled}
  class="relative h-5 w-9 flex-none rounded-full transition-colors duration-fast disabled:opacity-40
    {checked ? 'bg-accent' : 'bg-line-strong hover:bg-overlay'}"
  onclick={() => onchange(!checked)}
>
  <span
    class="absolute top-0.5 left-0.5 size-4 rounded-full bg-white transition-transform duration-fast
      {checked ? 'translate-x-4' : ''}"
  ></span>
</button>
