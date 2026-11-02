<script lang="ts">
  import { Check, CircleAlert, FolderOpen, Keyboard, MonitorPlay } from '@lucide/svelte'
  import { type GameEntry, type Settings, type Status, fileUrl } from './api'
  import { ago, playtime } from './format'
  import GameCover from './GameCover.svelte'
  import SkeletonGrid from './SkeletonGrid.svelte'
  import type { Category } from './SettingsView.svelte'

  let {
    games,
    loading,
    hotkey,
    status,
    settings,
    now,
    onopen,
    onchoosescreen,
    onsettings,
  }: {
    games: GameEntry[]
    loading: boolean
    hotkey: string
    status: Status | null
    settings: Settings | null
    /** Wall clock from the app's local tick, for "playing for". */
    now: number
    onopen: (g: GameEntry) => void
    onchoosescreen: () => void
    onsettings: (c: Category) => void
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
            ? `Playing now${session ? ` for ${playtime(now - session.start_ms)}` : ''}`
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

  // First-run checklist: capture, hotkey, where clips go.
  const capture = $derived.by(() => {
    if (!status) return { ok: false, text: 'Starting the recorder…' }
    if (status.needs_screen) return { ok: false, text: 'Choose which screen to record.', action: 'screen' as const }
    if (status.state === 'unavailable') return { ok: false, text: status.error ?? 'Capture unavailable.', action: 'recording' as const }
    return {
      ok: true,
      text: status.backend === 'kwin' ? 'Game windows are captured directly, no prompts.' : 'Capturing the screen you chose.',
    }
  })
  const folder = $derived(settings?.clips_dir.replace(/^\/home\/[^/]+/, '~') ?? '…')
</script>

{#if hero}
  <div class="mx-auto max-w-(--container-library) space-y-8">
    <button
      class="group relative block h-(--height-hero) w-full overflow-hidden rounded-surface bg-panel text-left"
      onclick={() => onopen(hero)}
    >
      {#if heroImage}
        <img
          src={fileUrl(heroImage)}
          alt=""
          draggable="false"
          class="absolute inset-0 size-full object-cover object-[center_30%] {hero.art.hero ? '' : 'brightness-75'}"
        />
      {/if}
      <!-- Legibility for the text over the art. -->
      <div class="absolute inset-0 bg-gradient-to-t from-black/85 via-black/25 to-transparent"></div>
      <div class="absolute inset-x-8 bottom-7 flex items-end justify-between gap-6">
        <div class="min-w-0">
          {#if hero.art.logo}
            <img src={fileUrl(hero.art.logo)} alt={hero.name} draggable="false" class="max-h-24 max-w-96 object-contain object-left-bottom" />
          {:else}
            <h1 class="display-face text-display leading-none font-bold">{hero.name}</h1>
          {/if}
          <p class="mt-3 flex items-center gap-2 text-body text-fg-2">
            {#if hero.playing}<span class="size-2 rounded-full bg-danger"></span>{/if}
            {heroLine}
          </p>
        </div>
        <span class="btn flex-none border-white/15 bg-black/60 hover:bg-black/80">View clips</span>
      </div>
    </button>

    <section>
      <h2 class="section-title mb-4">Recently played</h2>
      <div class="grid grid-cols-[repeat(auto-fill,minmax(150px,1fr))] gap-x-5 gap-y-6">
        {#each games as g (g.name)}
          <button class="group min-w-0 text-left" onclick={() => onopen(g)}>
            <div class="transition-transform duration-normal group-hover:-translate-y-1">
              <GameCover game={g} />
            </div>
            <div class="display-face mt-2 truncate text-body font-semibold">{g.name}</div>
            <div class="mt-0.5 flex items-center gap-1.5 truncate text-caption text-muted">
              {#if g.playing}<span class="size-1.5 flex-none rounded-full bg-danger"></span>{/if}
              {meta(g)}
            </div>
          </button>
        {/each}
      </div>
    </section>
  </div>
{:else if loading}
  <div class="mx-auto max-w-(--container-library) space-y-8">
    <div class="skeleton h-(--height-hero) rounded-surface"></div>
    <SkeletonGrid count={4} />
  </div>
{:else}
  <div class="mx-auto flex h-full max-w-2xl flex-col justify-center">
    <h1 class="display-face text-display font-semibold">Play something</h1>
    <p class="mt-2 text-body text-muted">
      Games you play show up here. clip.sh keeps the last {settings ? settings.replay_seconds : 30} seconds in memory while
      you play, so a moment is never lost.
    </p>

    <ol class="card mt-8 divide-y divide-line">
      <li class="flex items-center gap-4 px-5 py-4">
        <span class="flex size-9 flex-none items-center justify-center rounded-full {capture.ok ? 'bg-ok/15 text-ok' : 'bg-accent/15 text-accent'}">
          {#if capture.ok}<Check size={18} />{:else}<CircleAlert size={18} />{/if}
        </span>
        <div class="min-w-0 flex-1">
          <div class="flex items-center gap-2 text-body font-medium"><MonitorPlay size={15} class="text-muted" /> Capture</div>
          <div class="mt-0.5 truncate text-secondary text-muted" title={capture.text}>{capture.text}</div>
        </div>
        {#if capture.action === 'screen'}
          <button class="btn h-8 border-accent/60 text-accent" onclick={onchoosescreen}>Choose screen</button>
        {:else if capture.action === 'recording'}
          <button class="btn h-8" onclick={() => onsettings('Recording')}>Recording settings</button>
        {/if}
      </li>
      <li class="flex items-center gap-4 px-5 py-4">
        <span class="flex size-9 flex-none items-center justify-center rounded-full bg-ok/15 text-ok"><Check size={18} /></span>
        <div class="min-w-0 flex-1">
          <div class="flex items-center gap-2 text-body font-medium"><Keyboard size={15} class="text-muted" /> Save a clip</div>
          <div class="mt-0.5 text-secondary text-muted">Press <kbd class="align-[1px]">{hotkey}</kbd> in any game. Change it under Hotkeys.</div>
        </div>
        <button class="btn h-8" onclick={() => onsettings('Hotkeys')}>Change</button>
      </li>
      <li class="flex items-center gap-4 px-5 py-4">
        <span class="flex size-9 flex-none items-center justify-center rounded-full bg-ok/15 text-ok"><Check size={18} /></span>
        <div class="min-w-0 flex-1">
          <div class="flex items-center gap-2 text-body font-medium"><FolderOpen size={15} class="text-muted" /> Clips folder</div>
          <div class="mt-0.5 truncate text-secondary text-muted" title={settings?.clips_dir}>{folder}</div>
        </div>
        <button class="btn h-8" onclick={() => onsettings('Storage')}>Change</button>
      </li>
    </ol>
    <p class="mt-4 text-secondary text-muted">
      You can also drop any video onto this window to trim it.
    </p>
  </div>
{/if}
