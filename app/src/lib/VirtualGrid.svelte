<!--
  Windowed grid: only the rows near the viewport exist in the DOM. Row
  height is measured from what is rendered, so cards with label chips don't
  throw the maths off. Below `threshold` items it renders a plain grid.
-->
<script lang="ts" generics="T">
  import type { Snippet } from 'svelte'

  let {
    items,
    key,
    threshold = 120,
    overscan = 2,
    children,
  }: {
    items: T[]
    key: (item: T) => string
    /** Item count above which windowing switches on. */
    threshold?: number
    /** Extra rows rendered above and below the viewport. */
    overscan?: number
    children: Snippet<[T]>
  } = $props()

  let grid: HTMLDivElement | undefined = $state()
  let columns = $state(1)
  let rowHeight = $state(260)
  let viewTop = $state(0)
  let viewHeight = $state(800)
  /** Offset of the grid's top from the scroll container's content top. */
  let gridTop = $state(0)

  const windowed = $derived(items.length > threshold)
  const rows = $derived(Math.ceil(items.length / columns))
  const firstRow = $derived(windowed ? Math.max(0, Math.floor((viewTop - gridTop) / rowHeight) - overscan) : 0)
  const lastRow = $derived(
    windowed ? Math.min(rows, Math.ceil((viewTop - gridTop + viewHeight) / rowHeight) + overscan) : rows,
  )
  const shown = $derived(items.slice(firstRow * columns, lastRow * columns))
  const padTop = $derived(windowed ? firstRow * rowHeight : 0)
  const padBottom = $derived(windowed ? Math.max(0, rows - lastRow) * rowHeight : 0)

  function scrollParent(el: HTMLElement): HTMLElement {
    let p = el.parentElement
    while (p) {
      const o = getComputedStyle(p).overflowY
      if (o === 'auto' || o === 'scroll') return p
      p = p.parentElement
    }
    return document.documentElement
  }

  function measure(scroller: HTMLElement) {
    if (!grid) return
    viewTop = scroller.scrollTop
    viewHeight = scroller.clientHeight
    // The grid's border box; its padding (the spacers) lies inside it.
    gridTop = grid.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop
    const style = getComputedStyle(grid)
    const cols = style.gridTemplateColumns.split(' ').filter((c) => c !== '0px').length
    if (cols > 0) columns = cols
    // Row pitch from the rendered cards themselves, never from the grid's
    // own height, which includes the spacers and would feed back on itself.
    const cards = grid.children
    if (cards.length > columns) {
      const first = cards[0].getBoundingClientRect().top
      const lastRowStart = cards[Math.floor((cards.length - 1) / columns) * columns].getBoundingClientRect().top
      const renderedRows = Math.ceil(cards.length / columns)
      const h = (lastRowStart - first) / (renderedRows - 1)
      if (h > 40 && Math.abs(h - rowHeight) > 1) rowHeight = h
    } else if (cards.length > 0) {
      const h = cards[0].getBoundingClientRect().height + (parseFloat(style.rowGap) || 0)
      if (h > 40 && Math.abs(h - rowHeight) > 1) rowHeight = h
    }
  }

  $effect(() => {
    if (!grid) return
    const scroller = scrollParent(grid)
    let raf = 0
    const onScroll = () => {
      if (raf) return
      raf = requestAnimationFrame(() => {
        raf = 0
        measure(scroller)
      })
    }
    scroller.addEventListener('scroll', onScroll, { passive: true })
    const ro = new ResizeObserver(() => measure(scroller))
    ro.observe(grid)
    ro.observe(scroller)
    measure(scroller)
    return () => {
      scroller.removeEventListener('scroll', onScroll)
      ro.disconnect()
      cancelAnimationFrame(raf)
    }
  })
</script>

<div bind:this={grid} class="clip-grid grid gap-x-4 gap-y-5" style:padding-top="{padTop}px" style:padding-bottom="{padBottom}px">
  {#each shown as item (key(item))}
    {@render children(item)}
  {/each}
</div>
