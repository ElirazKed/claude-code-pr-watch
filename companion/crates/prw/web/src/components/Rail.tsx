import { useActions } from '../actions'
import { aboutOf, ago, OWN_LISTS } from '../format'
import { icons, laneIcon } from '../icons'
import type { List, State } from '../types'

/** Your three lists, then the rest prw watches, then who is polling. */
export function Rail({
  state,
  view,
  onView,
  children,
}: {
  state: State
  view: string
  onView: (id: string) => void
  /** Below the poll: the theme picker. */
  children?: React.ReactNode
}) {
  const own = OWN_LISTS.map((id) => state.lists.find((l) => l.id === id)).filter(
    (l): l is List => l !== undefined,
  )
  const also = state.lists.filter((l) => l.kind === 'mine' || l.kind === 'search')
  return (
    <aside className="rail" aria-label="Lists">
      <div className="brand">
        {icons.logo}
        <b>prw</b>
        <span className="tag">web</span>
      </div>
      <div>
        <div className="lbl-sm">Your lists</div>
        <nav className="lists">
          {own.map((list, i) => (
            <button
              key={list.id}
              type="button"
              className={`lane ${list.kind}`}
              aria-current={view === list.id}
              title={`${list.label} (${i + 1})`}
              onClick={() => onView(list.id)}
            >
              {laneIcon(list.kind)}
              <b>{list.label}</b>
              <span className={list.kind === 'inbox' && list.rows.length ? 'n hot' : 'n'}>
                {list.rows.length}
              </span>
              <small>{aboutOf(list).sub}</small>
            </button>
          ))}
        </nav>
      </div>
      <div>
        <div className="lbl-sm">Also watching</div>
        <nav className="also">
          {also.map((list) => (
            <button
              key={list.id}
              type="button"
              aria-current={view === list.id}
              onClick={() => onView(list.id)}
            >
              <span>{list.label}</span>
              <span className="n">{list.rows.length}</span>
              <small>{list.error ? `failed: ${list.error}` : aboutOf(list).sub}</small>
            </button>
          ))}
          {!also.some((l) => l.kind === 'search') && (
            <p className="tip">
              Saved searches, each a list of its own, go in <code>~/.config/prw/config.toml</code>.
            </p>
          )}
        </nav>
      </div>
      <PollBox state={state} />
      {children}
    </aside>
  )
}

const POLL_TONE = { mod: 'green', prw: 'green', nobody: 'yellow', stale: 'red', waiting: 'faint' }

export function PollBox({ state }: { state: State }) {
  const { now } = useActions()
  const { poll } = state
  const age = poll.fetchedAt ? ago(now, poll.fetchedAt) : ''
  return (
    <div className="poll">
      <div className="said">
        <span className="dot" style={{ background: `var(--${POLL_TONE[poll.by]})` }} />
        <span>
          {poll.words[0].toUpperCase() + poll.words.slice(1)}
          {age && (
            <>
              {' · '}
              <span className="mono">{age === 'now' ? 'just now' : `${age} ago`}</span>
            </>
          )}
        </span>
      </div>
      <div className="row2">
        <span>{state.logins.length ? state.logins.join(', ') : 'no gh login yet'}</span>
        <span className="mono">{poll.live} live</span>
      </div>
      {poll.isReadFailing && (
        <div className="bad">results.json didn't read: showing the last copy</div>
      )}
      {poll.error && <div className="bad">{poll.error}</div>}
      {poll.problems.map((problem) => (
        <div key={problem} className="bad">
          config.toml: {problem}
        </div>
      ))}
      {state.newer && (
        <div className="newer">
          prw {state.newer} is out (this is {state.version})
        </div>
      )}
    </div>
  )
}
