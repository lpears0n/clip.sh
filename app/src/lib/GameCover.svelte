<!-- Portrait cover: Steam's capsule art, or a frame from the newest clip with the title set over it. -->
<script lang="ts">
  import { type GameEntry, fileUrl } from './api'

  let { game }: { game: GameEntry } = $props()
</script>

<div class="relative aspect-[2/3] overflow-hidden rounded-[var(--radius-ctl)] bg-panel">
  {#if game.art.capsule}
    <img src={fileUrl(game.art.capsule)} alt="" draggable="false" class="size-full object-cover" />
  {:else}
    {#if game.fallback_image}
      <img src={fileUrl(game.fallback_image)} alt="" draggable="false" class="size-full scale-110 object-cover blur-[2px] brightness-50" />
    {/if}
    <span class="display-face absolute inset-x-3 bottom-3 text-[17px] leading-tight font-bold break-words">{game.name}</span>
  {/if}
</div>
