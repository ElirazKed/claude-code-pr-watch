import { useActions } from '../actions'
import { ago } from '../format'
import { icons } from '../icons'
import type { Row } from '../types'
import { Avatar } from './Avatar'
import { RepoChip } from './RepoChip'
import { transitionName } from './transition'

/** A PR put aside: when, from where, how it was then, and what brings it back. */
export function LaterRow({ row }: { row: Row }) {
  const { selected, select, open, aside, now } = useActions()
  const later = row.later
  if (!later) return null
  const isLoading = row.pill === 'loading'
  return (
    <div
      className={selected === row.url ? 'lr sel' : 'lr'}
      data-pr={row.url}
      tabIndex={0}
      style={{ viewTransitionName: transitionName(row.url) }}
      onClick={() => (isLoading ? select(row.url) : open(row.url))}
    >
      <Avatar login={row.author} size={28} isYours={row.isYours} />
      <div style={{ minWidth: 0 }}>
        <div className="line1">
          <RepoChip row={row} />
          {isLoading ? <span className="skeleton" /> : <span className="title">{row.title}</span>}
        </div>
        <div className="line2">
          {row.author && <span className="au">{row.author}</span>}
          <span className="mono">#{row.number}</span>
          <span className="aside">
            {icons.moon} put aside {ago(now, later.at)} ago, from{' '}
            {later.from === 'focus' ? 'Focus' : 'the Inbox'}
          </span>
        </div>
      </div>
      <div className="acts">
        <button
          type="button"
          className="btn sm"
          onClick={(e) => {
            e.stopPropagation()
            aside(row)
          }}
        >
          {icons.back} Bring back
        </button>
      </div>
      <dl className="then">
        <dt>Then</dt>
        <dd>{pillWords[later.pill] ?? later.pill}</dd>
        <dt>Now</dt>
        <dd>{row.headline}</dd>
        <dt>Wakes</dt>
        <dd>
          on a push, a review, a comment, CI finishing, or a merge
          {later.from === 'focus' ? ', back into Focus' : ''}
        </dd>
      </dl>
    </div>
  )
}

/** How a pill reads in a sentence. */
const pillWords: Record<string, string> = {
  merged: 'merged',
  closed: 'closed',
  draft: 'a draft',
  conflicts: 'conflicts with its base',
  ciFailing: 'a check failing',
  ciFailed: 'checks failed',
  ciRunning: 'checks running',
  changes: 'changes asked for',
  comments: 'review threads open',
  inReview: 'in review',
  behind: 'behind its base',
  blocked: 'blocked',
  checking: 'GitHub checking it',
  autoMerging: 'auto-merging',
  ready: 'ready to merge',
  loading: 'not read yet',
}
