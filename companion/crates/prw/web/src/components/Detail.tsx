import { useActions } from '../actions'
import { ago, plural } from '../format'
import { checkIcon, icons } from '../icons'
import type { Row } from '../types'
import { Avatar } from './Avatar'
import { RepoChip } from './RepoChip'
import { NextBox, Track } from './Track'

const VERDICT = {
  approved: ['approved', 'p-green'],
  changes: ['changes asked', 'p-red'],
  commented: ['commented', 'p-dim'],
  requested: ['review asked', 'p-dim'],
} as const

/** The PR open: where it is, what's next, its checks and reviewers. */
export function Detail({ row, onClose }: { row: Row; onClose: () => void }) {
  const { now } = useActions()
  return (
    <section className="detail" aria-label={`#${row.number}`}>
      <div className="dh">
        <div className="crumbs">
          <RepoChip row={row} isFull />
          <span>#{row.number}</span>
          {row.branch && (
            <>
              <span>·</span>
              <span>
                {row.branch} → {row.base}
              </span>
            </>
          )}
          <button type="button" className="x" aria-label="Close" onClick={onClose}>
            ×
          </button>
        </div>
        <h2>{row.title}</h2>
        <div className="facts">
          <span className="byline">
            <Avatar login={row.author} size={26} isYours={row.isYours} />
            <b>{row.author}</b>
          </span>
          <span>
            <span style={{ color: 'var(--green)' }}>+{row.additions}</span>{' '}
            <span style={{ color: 'var(--red)' }}>−{row.deletions}</span>
          </span>
          {row.agent && <span className="agent">{row.agent}</span>}
          {row.login && (
            <span>
              as <b style={{ color: 'var(--fg)' }}>{row.login}</b>
            </span>
          )}
          {row.updatedAt != null && <span>updated {ago(now, row.updatedAt)} ago</span>}
          <a
            className="ib"
            href={row.url}
            target="_blank"
            rel="noreferrer"
            title="Open on GitHub (o)"
            aria-label="Open on GitHub"
          >
            {icons.gh}
          </a>
        </div>
        <Place row={row} />
        <Track row={row} isBig />
        <NextBox row={row} />
      </div>
      <div className="dbody">
        <div className="wf">
          <div className="wh">
            <b>Checks</b>
            <span className="sub">
              {row.checks.length
                ? `${row.checks.filter((c) => c.state === 'pass').length} of ${row.checks.length} passed`
                : 'none reported'}
            </span>
            <span className="grow" />
            <a className="btn sm" href={`${row.url}/checks`} target="_blank" rel="noreferrer">
              On GitHub ↗
            </a>
          </div>
          {row.checks.map((check, i) => (
            <div key={i} className="job">
              <span className={`state ${check.state}`}>{checkIcon[check.state]}</span>
              <span className="nm">{check.name}</span>
              <span className="d">{check.state === 'skip' ? 'skipped' : ''}</span>
            </div>
          ))}
        </div>
        <div className="wf">
          <div className="wh">
            <b>Review</b>
            <span className="sub">
              {row.unresolved > 0 ? `${plural(row.unresolved, 'thread')} open` : row.headline}
            </span>
            <span className="grow" />
            <a className="btn sm" href={`${row.url}/files`} target="_blank" rel="noreferrer">
              Files ↗
            </a>
          </div>
          {row.reviewers.length === 0 && <div className="job muted">No reviewers yet.</div>}
          {row.reviewers.map((reviewer) => {
            const [words, tone] = VERDICT[reviewer.verdict]
            return (
              <div key={reviewer.login} className="job">
                <Avatar login={reviewer.login} size={18} />
                <span className="nm">{reviewer.login}</span>
                <span className={`pill ${tone}`}>{words}</span>
              </div>
            )
          })}
        </div>
        <p className="later-note">
          The conversation, check logs and files open here in a later version. Until then,{' '}
          <a href={row.url} target="_blank" rel="noreferrer">
            #{row.number} on GitHub
          </a>{' '}
          has them.
        </p>
      </div>
    </section>
  )
}

/** Which of your lists it is in, and the buttons that move it. */
function Place({ row }: { row: Row }) {
  const { pin, aside } = useActions()
  const where = row.isFocus ? (
    <span className="where" style={{ color: 'var(--yellow)' }}>
      {icons.starOn} In Focus
    </span>
  ) : row.later ? (
    <span className="where" style={{ color: 'var(--lav)' }}>
      {icons.moon} In Later, back when it changes
    </span>
  ) : (
    <span className="where">{icons.tray} Not sorted yet</span>
  )
  return (
    <div className="place">
      {where}
      <span className="grow" />
      {row.later ? (
        <button type="button" className="btn sm" onClick={() => aside(row)}>
          {icons.back} Bring back
        </button>
      ) : (
        <>
          <button type="button" className="btn sm" onClick={() => pin(row)}>
            {row.isFocus ? 'Unpin' : <>{icons.star} Pin to Focus</>}
          </button>
          <button type="button" className="btn sm" onClick={() => aside(row)}>
            {icons.moon} Later
          </button>
        </>
      )}
    </div>
  )
}
