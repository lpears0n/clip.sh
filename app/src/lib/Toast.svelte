<!-- Bottom-right notice, optionally with one action (e.g. Undo). -->
<script lang="ts" module>
  export interface ToastData {
    text: string
    error?: boolean
    action?: { label: string; run: () => void }
  }
</script>

<script lang="ts">
  import { X } from '@lucide/svelte'

  let { data, onclose }: { data: ToastData; onclose: () => void } = $props()
</script>

<div
  class="toast fixed right-5 bottom-5 z-40 flex items-center gap-3 rounded-[var(--radius-card)] border bg-raised py-2 pr-2 pl-4 text-[14px] font-medium shadow-xl shadow-black/40
    {data.error ? 'border-danger/50 text-danger' : 'border-line'}"
  role="status"
  aria-live="polite"
>
  <span class="max-w-[420px] truncate">{data.text}</span>
  {#if data.action}
    <button class="rounded-[var(--radius-ctl)] px-2.5 py-1 font-semibold text-accent hover:bg-white/[0.06]" onclick={data.action.run}>
      {data.action.label}
    </button>
  {/if}
  <button class="btn-icon size-7 text-muted" onclick={onclose} aria-label="Dismiss">
    <X size={14} />
  </button>
</div>

<style>
  .toast {
    animation: rise 180ms ease-out;
  }

  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(8px);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
</style>
