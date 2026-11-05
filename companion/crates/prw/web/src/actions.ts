// What a PR on the page can be asked to do, handed down to every list.

import { createContext, useContext } from 'react'
import type { Row } from './types'

export interface Actions {
  /** The PR the keys act on. */
  selected: string | null
  select: (url: string) => void
  /** Opens a PR's detail. */
  open: (url: string) => void
  /** Pins a PR to Focus, or unpins it. */
  pin: (row: Row) => void
  /** Puts a PR aside in Later, or brings it back. */
  aside: (row: Row) => void
  /** The model's clock (ms). */
  now: number
  /** Whether a PR came back from Later by itself a moment ago. */
  isWoke: (url: string) => boolean
}

export const ActionsContext = createContext<Actions | null>(null)

export function useActions(): Actions {
  const actions = useContext(ActionsContext)
  if (!actions) throw new Error('useActions outside ActionsContext')
  return actions
}
