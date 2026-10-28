<script lang="ts">
  import { type GameEntry, fileUrl } from './api'
  import { ago, playtime } from './format'
  import GameCover from './GameCover.svelte'

  let {
    games,
    loading,
    hotkey,
    onopen,
  }: {
    games: GameEntry[]
    loading: boolean
    hotkey: string
    onopen: (g: GameEntry) => void
  } = $props()

  const hero = $derived(games[0] ?? null)
  const heroImage = $derived(hero ? (hero.art.hero ?? hero.fallback_image) : null)
  const session = $derived(hero?.sessions[0])

  const clipCount = (n: number) => `${n} clip${n === 1 ? '' : 's'}`

  // Only the parts we know, joined, so a missing value never leaves a stray "·".
  const heroLine = $derived(
    hero
      ? [
          hero.playing
            ? `Playing now${session ? ` for ${playtime(Date.now() - session.start_ms)}` : ''}`
            : hero.last_played_ms
              ? `Last played ${ago(hero.last_played_ms)}`
              : null,
          hero.total_play_ms ? `${playtime(hero.total_play_ms)} total` : null,
          clipCount(hero.clip_count),
        ]
          .filter(Boolean)
          .join(' · ')
      : '',
  )

  function meta(g: GameEntry): string {
    const clips = clipCount(g.clip_count)
    if (g.playing) return `Playing now · ${clips}`
    return g.last_played_ms ? `${ago(g.last_played_ms)} · ${clips}` : clips
  }
</script>

{#if hero}
  <div class="mx-auto max-w-[1400px] space-y-9">
    <button
      class="group relative block h-[clamp(220px,36vh,380px)] w-full overflow-hidden rounded-[var(--radius-card)] bg-panel text-left"
      onclick={() => onopen(hero)}
    >
      {#if heroImage}
        <img
          src={fileUrl(heroImage)}
          alt=""
          draggable="false"
          class="absolute inset-0 size-full object-cover object-[center_30%] transition-transform duration-700 group-hover:scale-[1.015]
            {hero.art.hero ? '' : 'brightness-75'}"
        />
      {/if}
      <!-- Legibility for the text over the art. -->
      <div class="absolute inset-0 bg-gradient-to-t from-black/85 via-black/25 to-transparent"></div>
      <div class="absolute inset-x-8 bottom-7 flex items-end justify-between gap-6">
        <div class="min-w-0">
          {#if hero.art.logo}
            <img src={fileUrl(hero.art.logo)} alt={hero.name} draggable="false" class="max-h-24 max-w-[380px] object-contain object-left-bottom drop-shadow-lg" />
          {:else}
            <h1 class="display-face text-[34px] leading-none font-bold">{hero.name}</h1>
          {/if}
          <p class="mt-3 flex items-center gap-2 text-[14px] text-zinc-200">
            {#if hero.playing}<span class="size-2 rounded-full bg-danger"></span>{/if}
            {heroLine}
          </p>
        </div>
        <span class="btn flex-none border-white/15 bg-black/40 backdrop-blur-sm">View clips</span>
      </div>
    </button>

    <section>
      <h2 class="section-title mb-4">Recently played</h2>
      <div class="grid grid-cols-[repeat(auto-fill,minmax(150px,1fr))] gap-x-5 gap-y-6">
        {#each games as g (g.name)}
          <button class="group min-w-0 text-left" onclick={() => onopen(g)}>
            <div class="transition-transform duration-200 group-hover:-translate-y-1">
              <GameCover game={g} />
            </div>
            <div class="display-face mt-2.5 truncate text-[14px] font-semibold">{g.name}</div>
            <div class="mt-0.5 flex items-center gap-1.5 truncate text-[12px] text-muted">
              {#if g.playing}<span class="size-1.5 flex-none rounded-full bg-danger"></span>{/if}
              {meta(g)}
            </div>
          </button>
        {/each}
      </div>
    </section>
  </div>
{:else if !loading}
  <div class="flex h-full flex-col items-center justify-center text-center">
    <h1 class="display-face text-[28px] font-semibold">Play something.</h1>
    <p class="mt-3 max-w-[440px] text-[15px] text-muted">
      Games you play show up here. Press <kbd class="align-[1px]">{hotkey}</kbd> while playing to keep the last moments.
    </p>
  </div>
{/if}
