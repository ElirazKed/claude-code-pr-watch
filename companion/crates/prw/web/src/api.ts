// Talking to `prw web`: the key from the link, the state polled each second, and changes to
// the lists.

import { useCallback, useEffect, useRef, useState } from 'react'
import type { Change, State } from './types'

const KEY = 'prw-key'
const POLL_MS = 1000

/**
 * The key `prw web` printed in the page's link (`/#k=…`). Kept for this tab only, and taken
 * out of the address bar so it isn't bookmarked or shared with the URL.
 */
export function takeKey(): string | null {
  const fromLink = new URLSearchParams(location.hash.slice(1)).get('k')
  if (fromLink) {
    try {
      sessionStorage.setItem(KEY, fromLink)
    } catch {
      // A tab that can't keep it still has it until a reload.
    }
    history.replaceState(null, '', location.pathname + location.search)
    return fromLink
  }
  try {
    return sessionStorage.getItem(KEY)
  } catch {
    return null
  }
}

/** Why the state isn't coming: prw web stopped, or this tab's key is another run's. */
export type Trouble = 'gone' | 'key' | null

class Refused extends Error {
  constructor(readonly status: number) {
    super(`prw web answered ${status}`)
  }
}

async function ask(key: string, path: string, init?: RequestInit): Promise<State> {
  const response = await fetch(path, {
    ...init,
    headers: { 'X-Prw-Key': key, ...(init?.body ? { 'Content-Type': 'application/json' } : {}) },
  })
  if (!response.ok) throw new Refused(response.status)
  return (await response.json()) as State
}

/** The state as last polled, and a way to change the lists (the answer is the new state). */
export function useServer(key: string) {
  const [state, setState] = useState<State | null>(null)
  const [trouble, setTrouble] = useState<Trouble>(null)
  // A change's answer is newer than a poll sent before it: that poll's answer is dropped.
  const changes = useRef(0)

  useEffect(() => {
    let isLive = true
    let timer: ReturnType<typeof setTimeout>
    const poll = async () => {
      const sent = changes.current
      try {
        const next = await ask(key, '/api/state')
        if (isLive && sent === changes.current) {
          setState(next)
          setTrouble(null)
        }
      } catch (e) {
        if (isLive) setTrouble(e instanceof Refused && e.status === 401 ? 'key' : 'gone')
      }
      if (isLive) timer = setTimeout(poll, POLL_MS)
    }
    void poll()
    return () => {
      isLive = false
      clearTimeout(timer)
    }
  }, [key])

  const change = useCallback(
    async (body: Change): Promise<State | null> => {
      changes.current += 1
      try {
        return await ask(key, '/api/lists', { method: 'POST', body: JSON.stringify(body) })
      } catch (e) {
        setTrouble(e instanceof Refused && e.status === 401 ? 'key' : 'gone')
        return null
      }
    },
    [key],
  )

  return { state, setState, trouble, change }
}
