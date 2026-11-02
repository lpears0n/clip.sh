<!-- Placeholder for the clip grid while the library loads; same layout as the cards. -->
<script lang="ts">
  let { count = 8 }: { count?: number } = $props()
</script>

<div class="clip-grid grid gap-x-4 gap-y-5" aria-busy="true" aria-label="Loading clips">
  {#each { length: count } as _, i (i)}
    <div>
      <div class="skeleton aspect-video"></div>
      <div class="mt-3 flex justify-between">
        <div class="skeleton h-3.5 w-1/2 rounded-chip"></div>
        <div class="skeleton h-3 w-1/5 rounded-chip"></div>
      </div>
    </div>
  {/each}
</div>
