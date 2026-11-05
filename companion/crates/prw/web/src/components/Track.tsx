import { useActions } from '../actions'
import { toneOf } from '../format'
import { icons, nextIcon } from '../icons'
import type { Action, Row, Station } from '../types'

/** Checks, review, merge: where the PR is on its way in. */
export function Track({ row, isBig = false }: { row: Row; isBig?: boolean }) {
  if (!row.stations) return <div className="skeleton" style={{ width: '80%' }} />
  const { checks, review, merge } = row.stations
  return (
    <div className={isBig ? 'track big' : 'track'}>
      <Stop name="Checks" station={checks}>
        {row.checks.length > 0 && (
          <span className="seg">
            {row.checks.map((check, i) => (
              <i key={i} className={check.state} title={`${check.name}: ${check.state}`} />
            ))}
          </span>
        )}
      </Stop>
      <Stop name="Review" station={review} />
      <Stop name="Merge" station={merge} />
    </div>
  )
}

function Stop({
  name,
  station,
  children,
}: {
  name: string
  station: Station
  children?: React.ReactNode
}) {
  return (
    <div className={`st ${station.stop}`}>
      <span className="node" />
      <span className="k">{name}</span>
      <span className="v">{station.text}</span>
      {children}
    </div>
  )
}

/** What to do next, in a sentence, and the buttons that do it. */
export function NextBox({ row }: { row: Row }) {
  const { next } = row
  return (
    <div className={`next t-${toneOf(row)}`}>
      {nextIcon[next.icon]}
      <div className="nx">
        <b>{next.title}</b>
        <span>{next.text}</span>
      </div>
      {next.actions.length > 0 && (
        <div className="nacts">
          {next.actions.map((action) => (
            <ActionButton key={action.label} action={action} row={row} />
          ))}
        </div>
      )}
    </div>
  )
}

export function ActionButton({ action, row }: { action: Action; row: Row }) {
  const { pin } = useActions()
  if (action.kind === 'unpin') {
    return (
      <button
        type="button"
        className="btn sm"
        onClick={(e) => {
          e.stopPropagation()
          pin(row)
        }}
      >
        {action.label}
      </button>
    )
  }
  return (
    <a
      className={action.isPrimary ? 'btn sm primary' : 'btn sm'}
      href={action.url}
      target="_blank"
      rel="noreferrer"
      onClick={(e) => e.stopPropagation()}
    >
      {action.label} <span aria-hidden="true">↗</span>
    </a>
  )
}

/** Pin and put aside, or bring back for a PR in Later. */
export function ListButtons({ row }: { row: Row }) {
  const { pin, aside } = useActions()
  if (row.pill === 'loading' && !row.later) return null
  const stop = (fn: () => void) => (e: React.MouseEvent) => {
    e.stopPropagation()
    fn()
  }
  if (row.later) {
    return (
      <button
        type="button"
        className="ib zz"
        title="Bring back (z)"
        aria-label="Bring back from Later"
        onClick={stop(() => aside(row))}
      >
        {icons.back}
      </button>
    )
  }
  const pinLabel = row.isFocus ? 'Unpin from Focus' : 'Pin to Focus'
  return (
    <>
      <button
        type="button"
        className={row.isFocus ? 'ib pin on' : 'ib pin'}
        title={`${pinLabel} (p)`}
        aria-label={pinLabel}
        aria-pressed={row.isFocus}
        onClick={stop(() => pin(row))}
      >
        {row.isFocus ? icons.starOn : icons.star}
      </button>
      <button
        type="button"
        className="ib zz"
        title="Put aside until it changes (z)"
        aria-label="Put aside in Later"
        onClick={stop(() => aside(row))}
      >
        {icons.moon}
      </button>
    </>
  )
}
