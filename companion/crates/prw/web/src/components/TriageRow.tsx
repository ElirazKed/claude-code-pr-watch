import { useActions } from '../actions'
import { ago, BUCKETS, byRepo, type Grouping } from '../format'
import type { Check, Row } from '../types'
import { Avatar } from './Avatar'
import { RepoChip } from './RepoChip'
import { ActionButton, ListButtons } from './Track'
import { transitionName } from './transition'

/** A list's PRs in its groups: by what each needs, most pressing first, or by repo. */
export function Grouped({
  rows,
  isFocusMarked,
  by = 'status',
}: {
  rows: Row[]
  isFocusMarked: boolean
  by?: Grouping
}) {
  return (
    <>
      <div className="cols colhead">
        <span />
        <span>Pull request</span>
        <span>Review</span>
        <span>Checks</span>
        <span style={{ textAlign: 'right' }}>Age</span>
        <span style={{ textAlign: 'right' }}>Next · Focus · Later</span>
      </div>
      {by === 'repo'
        ? byRepo(rows).map((group) => (
            <section key={group.repo} className="bucket">
              <h2>
                <RepoChip row={group.rows[0]} isLarge />
                <span className="badge">{group.rows.length}</span>
                <span className="hint">{needsOf(group.rows)}</span>
              </h2>
              {group.rows.map((row) => (
                <TriageRow
                  key={row.url}
                  row={row}
                  isFocusMarked={isFocusMarked}
                  isRepoShown={false}
                />
              ))}
            </section>
          ))
        : BUCKETS.map((bucket) => {
            const inIt = rows.filter((r) => r.bucket === bucket.id)
            if (inIt.length === 0) return null
            return (
              <section key={bucket.id} className="bucket">
                <h2>
                  <span className="badge" style={{ color: `var(--${bucket.tone})` }}>
                    {inIt.length}
                  </span>
                  {bucket.label}
                  <span className="hint">{bucket.hint}</span>
                </h2>
                {inIt.map((row) => (
                  <TriageRow key={row.url} row={row} isFocusMarked={isFocusMarked} />
                ))}
              </section>
            )
          })}
    </>
  )
}

/** What a repo's PRs need, as many of each: "1 needs you · 2 failing". */
function needsOf(rows: Row[]): string {
  return BUCKETS.map(
    (bucket) => [bucket, rows.filter((r) => r.bucket === bucket.id).length] as const,
  )
    .filter(([, n]) => n > 0)
    .map(([bucket, n]) => `${n} ${bucket.label.toLowerCase()}`)
    .join(' · ')
}

/** One PR in a triage list: its repo, who, what, the review and checks, and its next step's
    button. */
export function TriageRow({
  row,
  isFocusMarked,
  isRepoShown = true,
}: {
  row: Row
  isFocusMarked: boolean
  isRepoShown?: boolean
}) {
  const { selected, select, open, now } = useActions()
  const isLoading = row.pill === 'loading'
  const lead = row.next.actions.find((a) => a.kind === 'link' && a.isPrimary)
  return (
    <div
      className={selected === row.url ? 'row cols sel' : 'row cols'}
      data-pr={row.url}
      tabIndex={0}
      style={{ viewTransitionName: transitionName(row.url) }}
      onClick={() => (isLoading ? select(row.url) : open(row.url))}
    >
      <Progress row={row} />
      <Avatar login={row.author} size={28} isYours={row.isYours} />
      <div style={{ minWidth: 0 }}>
        <div className="line1">
          {isRepoShown && <RepoChip row={row} />}
          {!isLoading ? (
            <span className="title">{row.title}</span>
          ) : row.error ? (
            <span className="title muted">Not read</span>
          ) : (
            <span className="skeleton" />
          )}
        </div>
        <div className="line2">
          {row.author && <span className="au">{row.author}</span>}
          <span className="mono">#{row.number}</span>
          {isLoading ? (
            !row.error && <span>reading from GitHub…</span>
          ) : (
            <span className="branch">{row.branch}</span>
          )}
          {isFocusMarked && row.isFocus && <span className="pill p-yellow">★ in Focus</span>}
          {row.agent && <span className="agent">{row.agent}</span>}
          {row.error && <span className="pill p-red">{row.error}</span>}
        </div>
      </div>
      <div className="rv">{!isLoading && <ReviewPill row={row} />}</div>
      <ChecksCell row={row} />
      <div className="age">{ago(now, row.updatedAt)}</div>
      <div className="acts">
        {lead && (
          <span className="lead">
            <ActionButton action={lead} row={row} />
          </span>
        )}
        <ListButtons row={row} />
      </div>
    </div>
  )
}

const count = (checks: Check[], state: Check['state']) =>
  checks.filter((c) => c.state === state).length

/** The row's own background, filled as far as its checks got. */
function Progress({ row }: { row: Row }) {
  if (row.pill === 'loading') return null
  if (row.pill === 'merged') {
    return (
      <span className="prog">
        <i className="done" style={{ width: '100%' }} />
      </span>
    )
  }
  const total = row.checks.length || 1
  return (
    <span className="prog">
      {(['pass', 'fail', 'skip', 'run'] as const).map((state) => {
        const width = (count(row.checks, state) / total) * 100
        return width > 0 ? <i key={state} className={state} style={{ width: `${width}%` }} /> : null
      })}
    </span>
  )
}

function ReviewPill({ row }: { row: Row }) {
  const review = row.stations?.review
  if (!review) return null
  const tone = { done: 'p-green', blocked: 'p-red', now: 'p-dim', todo: 'p-dim', merged: 'p-lav' }[
    review.stop
  ]
  const isYourTurn = row.bucket === 'you' && review.text === 'yours to give'
  return <span className={`pill ${isYourTurn ? 'p-red' : tone}`}>{review.text}</span>
}

function ChecksCell({ row }: { row: Row }) {
  if (row.pill === 'loading') {
    return (
      <div className="ck">
        <small style={{ color: 'var(--faint)' }}>{row.error ? '' : 'reading…'}</small>
      </div>
    )
  }
  const { checks } = row
  const parts = [
    ['fail', 'failed', 'red'],
    ['run', 'running', 'yellow'],
    ['skip', 'skipped', 'blue'],
    ['queue', 'queued', 'faint'],
  ] as const
  const said = parts
    .map(([state, word, colour]) => {
      const n = count(checks, state)
      return n > 0 ? (
        <span key={state} style={{ color: `var(--${colour})` }}>
          {n} {word}
        </span>
      ) : null
    })
    .filter((part) => part !== null)
  return (
    <div className="ck">
      <b>
        {count(checks, 'pass')}
        <span style={{ color: 'var(--faint)' }}> / {checks.length}</span>
      </b>
      <small>
        {said.length > 0 ? (
          said.flatMap((part, i) => (i > 0 ? [' · ', part] : [part]))
        ) : (
          <span style={{ color: checks.length ? 'var(--green)' : 'var(--faint)' }}>
            {checks.length ? 'all passed' : 'no checks'}
          </span>
        )}
      </small>
    </div>
  )
}
