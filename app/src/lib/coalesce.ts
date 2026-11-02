/**
 * Run an async refresh at most once at a time, and at most once per `wait`
 * ms. Calls during a run are folded into one more run afterwards, so a
 * burst of events (save + watcher + usage) costs one fetch, not three.
 */
export function coalesce(run: () => Promise<void>, wait = 150): () => void {
  let timer: ReturnType<typeof setTimeout> | null = null
  let running = false
  let again = false

  const fire = async () => {
    timer = null
    if (running) {
      again = true
      return
    }
    running = true
    try {
      await run()
    } finally {
      running = false
      if (again) {
        again = false
        schedule()
      }
    }
  }

  const schedule = () => {
    if (!timer) timer = setTimeout(fire, wait)
  }

  return schedule
}
