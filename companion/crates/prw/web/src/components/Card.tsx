import { useActions } from '../actions'
import { ago } from '../format'
import { icons } from '../icons'
import type { Row } from '../types'
import { Avatar } from './Avatar'
import { RepoChip } from './RepoChip'
import { ListButtons, NextBox, Track } from './Track'
import { transitionName } from './transition'

/** A PR on the Focus desk: who, what, where it is, and what's next. */
export function Card({ row }: { row: Row }) {
  const { selected, select, open, now, isWoke } = useActions()
  const isLoading = row.pill === 'loading'
  return (
    <article
      className={selected === row.url ? 'fc sel' : 'fc'}
      data-pr={row.url}
      tabIndex={0}
      style={{ viewTransitionName: transitionName(row.url) }}
      onClick={() => (isLoading ? select(row.url) : open(row.url))}
    >
      <div className="top">
        <Avatar login={row.author} size={30} isYours={row.isYours} />
        <span className="who">
          <b>
            {row.author || 'Reading…'}
            {row.isYours && <span className="you">you</span>}
          </b>
          <span>
            <RepoChip row={row} />
            <span className="ref">#{row.number}</span>
            {row.agent && (
              <>
                {' · '}
                <span className="agent">{row.agent}</span>
              </>
            )}
          </span>
        </span>
        <span className="grow" />
        <span className="mono">{ago(now, row.updatedAt)}</span>
        <ListButtons row={row} />
      </div>
      {isWoke(row.url) && <span className="woke">{icons.back} Back from Later: it changed</span>}
      {!isLoading ? (
        <h3>{row.title}</h3>
      ) : row.error ? (
        <h3 className="muted">Not read: {row.error}</h3>
      ) : (
        <div className="skeleton" />
      )}
      <Track row={row} />
      <NextBox row={row} />
    </article>
  )
}
