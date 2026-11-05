// The page: the rail of lists, the list picked, and the PR open beside it.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { flushSync } from 'react-dom'
import { ActionsContext, type Actions } from './actions'
import { useServer, type Trouble } from './api'
import { Card } from './components/Card'
import { Detail } from './components/Detail'
import { LaterRow } from './components/LaterRow'
import { Rail } from './components/Rail'
import { ThemePicker } from './components/ThemePicker'
import { Toasts, type ToastItem } from './components/Toasts'
import { Grouped, TriageRow } from './components/TriageRow'
import { aboutOf, BUCKETS, type Grouping, matches, OWN_LISTS, rowByUrl } from './format'
import { icons, laneIcon } from './icons'
import { useTheme } from './theme'
import type { Bucket, Change, List, Row, State } from './types'

const TOAST_MS = 4200
const WAKE_MS = 7000
/** How long a PR back from Later by itself says so on its card. */
const WOKE_MS = 5 * 60_000

const isReduced = () => matchMedia('(prefers-reduced-motion: reduce)').matches

/** A state change drawn as a move: each PR slides to where it lands. */
function asMove(draw: () => void) {
  if (document.startViewTransition && !isReduced()) {
    document.startViewTransition(() => flushSync(draw))
  } else {
    draw()
  }
}

const GROUPING_KEY = 'prw-grouping'

/** How the lists were last grouped, in this browser. */
function readGrouping(): Grouping {
  try {
    return localStorage.getItem(GROUPING_KEY) === 'repo' ? 'repo' : 'status'
  } catch {
    return 'status'
  }
}

function saveGrouping(grouping: Grouping) {
  try {
    localStorage.setItem(GROUPING_KEY, grouping)
  } catch {
    // Not kept: it lasts until a reload.
  }
}

const toastKey = (state: State) => (state.toast ? `${state.toast.text}|${state.toast.until}` : '')

export function App({ serverKey }: { serverKey: string }) {
  const { state, setState, trouble, change } = useServer(serverKey)
  if (!state) return <Waiting trouble={trouble} />
  return <Page state={state} setState={setState} trouble={trouble} change={change} />
}

function Page({
  state,
  setState,
  trouble,
  change,
}: {
  state: State
  setState: (state: State) => void
  trouble: Trouble
  change: (body: Change) => Promise<State | null>
}) {
  const [picked, setPicked] = useState<string | null>(null)
  const [selected, setSelected] = useState<string | null>(null)
  const [opened, setOpened] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const [need, setNeed] = useState<Bucket | null>(null)
  const [grouping, setGrouping] = useState<Grouping>(readGrouping)
  const [toasts, setToasts] = useState<ToastItem[]>([])
  const [woke, setWoke] = useState<ReadonlyMap<string, number>>(new Map())
  const [theme, pickTheme] = useTheme()
  const filter = useRef<HTMLInputElement>(null)
  const toastIds = useRef(0)
  /** The model's toasts already shown (or said by a change of ours, with its undo). */
  const seen = useRef(new Set<string>([toastKey(state)]))
  /** The PRs we moved: one leaving Later is ours, not a wake. */
  const moved = useRef(new Set<string>())
  const wasLater = useRef(
    new Set(state.lists.find((l) => l.id === 'later')?.rows.map((r) => r.url)),
  )

  const byId = (id: string) => state.lists.find((l) => l.id === id)
  const focus = byId('focus')
  const view = picked && byId(picked) ? picked : focus?.rows.length ? 'focus' : 'inbox'
  const list = byId(view) ?? state.lists[0]
  const openRow = rowByUrl(state.lists, opened)

  const say = useCallback((toast: Omit<ToastItem, 'id'>) => {
    const id = ++toastIds.current
    setToasts((all) => [...all, { ...toast, id }])
    setTimeout(
      () => setToasts((all) => all.filter((t) => t.id !== id)),
      toast.isWake ? WAKE_MS : TOAST_MS,
    )
  }, [])

  // The model's own toasts (a PR back from Later, say), each once; and the PRs that left Later
  // by themselves, marked on their cards.
  useEffect(() => {
    const key = toastKey(state)
    if (state.toast && !seen.current.has(key)) {
      seen.current.add(key)
      say({
        text: state.toast.text,
        isWake: state.toast.text.startsWith('↩'),
        isBad: state.toast.tone === 'bad',
      })
    }
    const later = new Set(state.lists.find((l) => l.id === 'later')?.rows.map((r) => r.url))
    const back = [...wasLater.current].filter((url) => !later.has(url) && !moved.current.has(url))
    wasLater.current = later
    moved.current.clear()
    if (back.length) {
      setWoke((all) => {
        const next = new Map(all)
        back.forEach((url) => next.set(url, state.now))
        return next
      })
    }
  }, [state, say])

  const move = useCallback(
    (first: Change, firstUndo?: Change) => {
      const run = async (body: Change, undo?: Change): Promise<void> => {
        moved.current.add(body.url)
        const next = await change(body)
        if (!next) {
          say({ text: "prw web didn't answer: nothing changed", isBad: true })
          return
        }
        seen.current.add(toastKey(next))
        asMove(() => setState(next))
        if (next.toast) {
          say({ text: next.toast.text, undo: undo ? () => void run(undo) : undefined })
        }
      }
      return run(first, firstUndo)
    },
    [change, say, setState],
  )

  const actions: Actions = useMemo(
    () => ({
      selected,
      select: setSelected,
      open: (url) => {
        setSelected(url)
        setOpened(url)
      },
      pin: (row: Row) =>
        void move(
          { url: row.url, focus: !row.isFocus },
          row.later ? { url: row.url, later: true } : { url: row.url, focus: row.isFocus },
        ),
      aside: (row: Row) =>
        void move({ url: row.url, later: !row.later }, { url: row.url, later: row.later !== null }),
      now: state.now,
      isWoke: (url) => state.now - (woke.get(url) ?? -Infinity) < WOKE_MS,
    }),
    [selected, move, state.now, woke],
  )

  const groupBy = useCallback((by: Grouping) => {
    setGrouping(by)
    saveGrouping(by)
  }, [])

  const go = useCallback((id: string) => {
    setPicked(id)
    setNeed(null)
    setSelected(null)
    scrollTo({ top: 0 })
  }, [])

  // The keys, as the TUI's where they can be.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const active = document.activeElement
      const isTyping = active instanceof HTMLInputElement || active instanceof HTMLTextAreaElement
      if (e.key === 'Escape') {
        if (isTyping) (active as HTMLElement).blur()
        else setOpened(null)
        return
      }
      if (isTyping || e.metaKey || e.ctrlKey || e.altKey) return
      const urls = [...document.querySelectorAll<HTMLElement>('#main [data-pr]')].map(
        (el) => el.dataset.pr ?? '',
      )
      const current = rowByUrl(
        state.lists,
        opened && !urls.includes(selected ?? '') ? opened : selected,
      )
      if (e.key === '/') {
        e.preventDefault()
        filter.current?.focus()
      } else if (e.key === 'j' || e.key === 'k' || e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        if (!urls.length) return
        e.preventDefault()
        const at = urls.indexOf(selected ?? '')
        const step = e.key === 'j' || e.key === 'ArrowDown' ? 1 : -1
        const url = urls[Math.max(0, Math.min(urls.length - 1, at < 0 ? 0 : at + step))]
        setSelected(url)
        if (opened && rowByUrl(state.lists, url)?.pill !== 'loading') setOpened(url)
        document
          .querySelector(`#main [data-pr="${CSS.escape(url)}"]`)
          ?.scrollIntoView({ block: 'nearest' })
      } else if (e.key === 'Enter' && selected) {
        if (rowByUrl(state.lists, selected)?.pill !== 'loading') setOpened(selected)
      } else if (e.key === 'g') {
        groupBy(grouping === 'repo' ? 'status' : 'repo')
      } else if (['1', '2', '3'].includes(e.key)) {
        go(OWN_LISTS[Number(e.key) - 1])
      } else if (current && e.key === 'p') {
        actions.pin(current)
      } else if (current && e.key === 'z') {
        actions.aside(current)
      } else if (current && e.key === 'o') {
        window.open(current.url, '_blank', 'noopener,noreferrer')
      }
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [state.lists, opened, selected, actions, go, grouping, groupBy])

  const rows = list.rows.filter(
    (r) =>
      matches(r, query) &&
      (!need || list.kind === 'focus' || list.kind === 'later' || r.bucket === need),
  )
  const about = aboutOf(list)
  return (
    <ActionsContext.Provider value={actions}>
      {trouble && <TroubleBar trouble={trouble} />}
      <div className={openRow ? 'app open' : 'app'}>
        <Rail state={state} view={view} onView={go}>
          <ThemePicker theme={theme} onPick={pickTheme} />
        </Rail>
        <main className="main" id="main">
          <MobileLanes state={state} view={view} onView={go} />
          <div className="head">
            <div className="title-block">
              <h1>
                <span className={`h-ic ${list.kind}`}>{laneIcon(list.kind)}</span>
                {list.label} <span className="n">{list.rows.length}</span>
              </h1>
              <p>{about.about}</p>
            </div>
            <label className="search">
              {icons.search}
              <input
                ref={filter}
                value={query}
                placeholder="Filter by title, repo, branch, agent"
                aria-label="Filter"
                onChange={(e) => setQuery(e.target.value)}
              />
              <kbd>/</kbd>
            </label>
          </div>
          {list.kind !== 'focus' && list.kind !== 'later' && (
            <div className="listbar">
              <Tally rows={list.rows} need={need} onNeed={setNeed} />
              <GroupSwitch by={grouping} onBy={groupBy} />
            </div>
          )}
          {list.error && <div className="banner bad">The search failed: {list.error}</div>}
          <Body
            list={list}
            rows={rows}
            state={state}
            query={query}
            grouping={grouping}
            onView={go}
          />
          <ThemePicker theme={theme} onPick={pickTheme} className="appearance appearance-m" />
        </main>
        {openRow && (
          <>
            <div className="scrim" onClick={() => setOpened(null)} />
            <Detail row={openRow} onClose={() => setOpened(null)} />
          </>
        )}
      </div>
      <Hints />
      <Toasts
        toasts={toasts}
        onDismiss={(id) => setToasts((all) => all.filter((t) => t.id !== id))}
      />
    </ActionsContext.Provider>
  )
}

function Body({
  list,
  rows,
  state,
  query,
  grouping,
  onView,
}: {
  list: List
  rows: Row[]
  state: State
  query: string
  grouping: Grouping
  onView: (id: string) => void
}) {
  if (list.kind === 'focus') {
    const inbox =
      state.lists.find((l) => l.id === 'inbox')?.rows.filter((r) => matches(r, query)) ?? []
    return (
      <>
        {rows.length ? (
          <div className="desk">
            {rows.map((row) => (
              <Card key={row.url} row={row} />
            ))}
          </div>
        ) : (
          <Empty list={list} state={state} query={query} />
        )}
        {inbox.length > 0 && (
          <section className="bucket">
            <h2>
              <span className="badge" style={{ color: 'var(--accent)' }}>
                {inbox.length}
              </span>
              Inbox
              <span className="hint">waiting to be sorted</span>
              <a
                href="#inbox"
                onClick={(e) => {
                  e.preventDefault()
                  onView('inbox')
                }}
              >
                Sort the Inbox →
              </a>
            </h2>
            {inbox.slice(0, 3).map((row) => (
              <TriageRow key={row.url} row={row} isFocusMarked={false} />
            ))}
          </section>
        )}
      </>
    )
  }
  if (!rows.length) return <Empty list={list} state={state} query={query} />
  if (list.kind === 'later') {
    return (
      <div className="shelf">
        {rows.map((row) => (
          <LaterRow key={row.url} row={row} />
        ))}
      </div>
    )
  }
  return <Grouped rows={rows} isFocusMarked by={grouping} />
}

/** Group a list by what each PR needs, or by repo (g). */
function GroupSwitch({ by, onBy }: { by: Grouping; onBy: (by: Grouping) => void }) {
  return (
    <span className="group-by">
      <span className="lbl-sm">Group</span>
      <span className="modes" role="group" aria-label="Group by">
        {(
          [
            ['status', 'By status'],
            ['repo', 'By repo'],
          ] as const
        ).map(([id, label]) => (
          <button key={id} type="button" aria-pressed={by === id} onClick={() => onBy(id)}>
            {label}
          </button>
        ))}
      </span>
    </span>
  )
}

function Tally({
  rows,
  need,
  onNeed,
}: {
  rows: Row[]
  need: Bucket | null
  onNeed: (need: Bucket | null) => void
}) {
  return (
    <div className="tally">
      {BUCKETS.map((bucket) => {
        const n = rows.filter((r) => r.bucket === bucket.id && r.pill !== 'loading').length
        if (!n) return null
        return (
          <button
            key={bucket.id}
            type="button"
            aria-pressed={need === bucket.id}
            onClick={() => onNeed(need === bucket.id ? null : bucket.id)}
          >
            <span className="dot" style={{ background: `var(--${bucket.tone})` }} />
            <b>{n}</b>
            {bucket.label}
          </button>
        )
      })}
    </div>
  )
}

/** What an empty list says: on a first run, how PRs get here. */
function Empty({ list, state, query }: { list: List; state: State; query: string }) {
  const [title, text] = (() => {
    if (query.trim() && list.rows.length)
      return ['Nothing matches', `No PR here matches “${query.trim()}”.`]
    switch (list.kind) {
      case 'focus':
        return ['Nothing pinned', 'Pin a PR with ★ or p, and it waits here with its next step.']
      case 'later':
        return ['Nothing put aside', '⏾ or z on a PR hides it here until something changes on it.']
      case 'inbox':
        if (state.poll.by === 'waiting') {
          return [
            'Waiting for the first poll',
            'prw polls GitHub itself in a moment (unless --no-poll), or a Claude session with the pr-watch mod does.',
          ]
        }
        if (state.poll.live === 0) {
          return [
            'Nothing watched yet',
            "The PRs your agents watch land here: Claude Code's with the pr-watch mod, and any other agent's through prw watch <PR URL>.",
          ]
        }
        return [
          'Inbox zero',
          'Everything your agents picked up is in Focus or Later. New PRs land here.',
        ]
      default:
        return ['Nothing here', 'No open PRs in this list.']
    }
  })()
  return (
    <div className="empty">
      <b>{title}</b>
      <span>{text}</span>
    </div>
  )
}

/** Below 760px the rail goes: your lists as a segmented control, the rest in a select. */
function MobileLanes({
  state,
  view,
  onView,
}: {
  state: State
  view: string
  onView: (id: string) => void
}) {
  const own = OWN_LISTS.map((id) => state.lists.find((l) => l.id === id)).filter(
    (l): l is List => !!l,
  )
  const also = state.lists.filter((l) => l.kind === 'mine' || l.kind === 'search')
  return (
    <div className="lanes-m">
      <span className="segm">
        {own.map((list) => (
          <button
            key={list.id}
            type="button"
            aria-pressed={view === list.id}
            onClick={() => onView(list.id)}
          >
            {laneIcon(list.kind)}
            {list.label} <span className="n">{list.rows.length}</span>
          </button>
        ))}
      </span>
      {also.length > 0 && (
        <select
          aria-label="Other lists"
          value={also.some((l) => l.id === view) ? view : ''}
          onChange={(e) => e.target.value && onView(e.target.value)}
        >
          <option value="">More lists…</option>
          {also.map((list) => (
            <option key={list.id} value={list.id}>
              {list.label} ({list.rows.length})
            </option>
          ))}
        </select>
      )}
    </div>
  )
}

function Hints() {
  const keys: [string[], string][] = [
    [['j', 'k'], 'move'],
    [['⏎'], 'open'],
    [['p'], 'focus'],
    [['z'], 'later'],
    [['o'], 'GitHub'],
    [['/'], 'filter'],
    [['g'], 'group'],
    [['1', '2', '3'], 'lists'],
    [['esc'], 'close'],
  ]
  return (
    <div className="hints" aria-hidden="true">
      {keys.map(([ks, what]) => (
        <span key={what}>
          {ks.map((k) => (
            <kbd key={k}>{k}</kbd>
          ))}{' '}
          {what}
        </span>
      ))}
    </div>
  )
}

function TroubleBar({ trouble }: { trouble: Exclude<Trouble, null> }) {
  return (
    <div className="trouble" role="alert">
      {trouble === 'key'
        ? "This tab's key is an earlier prw web's: open the link the running one printed."
        : 'prw web stopped answering. Start it again (prw web), and open the link it prints.'}
    </div>
  )
}

function Waiting({ trouble }: { trouble: Trouble }) {
  return (
    <div className="lone">{trouble ? <TroubleBar trouble={trouble} /> : <p>Reading prw…</p>}</div>
  )
}
