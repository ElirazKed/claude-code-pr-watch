// The page's icons: 24-unit strokes in the text's colour.

import type { ReactNode } from 'react'
import type { CheckState, ListKind, Icon as NextIcon } from './types'

/** An icon's drawing, as an element (not a component: the icons are values). */
function svg(children: ReactNode, className = 'ic') {
  return (
    <svg
      className={className}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {children}
    </svg>
  )
}

const STAR = 'm12 3.6 2.6 5.3 5.8.8-4.2 4.1 1 5.8L12 16.9l-5.2 2.7 1-5.8-4.2-4.1 5.8-.8z'

export const icons = {
  star: svg(
    <>
      <path d={STAR} />
    </>,
  ),
  starOn: svg(
    <>
      <path fill="currentColor" d={STAR} />
    </>,
  ),
  tray: svg(
    <>
      <path d="M4 13.5 6.6 6h10.8l2.6 7.5V18a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 18z" />
      <path d="M4 13.5h4.5l1.2 2.2h4.6l1.2-2.2H20" />
    </>,
  ),
  moon: svg(
    <>
      <path d="M19.5 14.6A7.8 7.8 0 1 1 9.4 4.5a6.2 6.2 0 0 0 10.1 10.1z" />
    </>,
  ),
  back: svg(
    <>
      <path d="M9 14 4 9l5-5" />
      <path d="M4 9h10.5a5.5 5.5 0 0 1 0 11H11" />
    </>,
  ),
  x: svg(
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="m9 9 6 6M15 9l-6 6" />
    </>,
  ),
  ok: svg(
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="m8 12.5 2.6 2.6L16 9.6" />
    </>,
  ),
  run: svg(
    <>
      <circle cx="12" cy="12" r="9" opacity=".3" />
      <path d="M12 3a9 9 0 0 1 9 9" />
    </>,
    'ic spin',
  ),
  queue: svg(
    <>
      <circle cx="12" cy="12" r="9" strokeDasharray="3 3" />
    </>,
  ),
  skip: svg(
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M6 18 18 6" />
    </>,
  ),
  merged: svg(
    <>
      <circle cx="7" cy="6" r="2.4" />
      <circle cx="7" cy="18" r="2.4" />
      <circle cx="17" cy="12" r="2.4" />
      <path d="M7 8.4v7.2M7 8.4c0 2.6 3 3.6 7.6 3.6" />
    </>,
  ),
  alert: svg(
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 7.5v5.5M12 16.5v.2" />
    </>,
  ),
  clock: svg(
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 7.5V12l3 2" />
    </>,
  ),
  conflict: svg(
    <>
      <path d="M6 3v12M18 9v12" />
      <circle cx="6" cy="18" r="2.5" />
      <circle cx="18" cy="6" r="2.5" />
      <path d="m10 10 4 4M14 10l-4 4" />
    </>,
  ),
  eye: svg(
    <>
      <path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z" />
      <circle cx="12" cy="12" r="2.8" />
    </>,
  ),
  thread: svg(
    <>
      <path d="M4 5.5h16v10H9l-5 4z" />
    </>,
  ),
  draft: svg(
    <>
      <path d="M4 20h4L19 9l-4-4L4 16z" />
    </>,
  ),
  search: svg(
    <>
      <circle cx="11" cy="11" r="7" />
      <path d="m20 20-3.5-3.5" />
    </>,
  ),
  gh: svg(
    <>
      <path d="M14 4h6v6M20 4l-9 9M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5" />
    </>,
  ),
  logo: svg(
    <>
      <circle cx="6" cy="6" r="2.5" />
      <circle cx="6" cy="18" r="2.5" />
      <circle cx="18" cy="18" r="2.5" />
      <path d="M6 8.5v7M18 15.5V10a3 3 0 0 0-3-3h-4" />
      <path d="m13 5-2 2 2 2" />
    </>,
    'ic logo',
  ),
} satisfies Record<string, ReactNode>

/** The icon a next step leads with. */
export const nextIcon: Record<NextIcon, ReactNode> = {
  reading: icons.queue,
  merged: icons.merged,
  closed: icons.x,
  conflict: icons.conflict,
  alert: icons.alert,
  thread: icons.thread,
  eye: icons.eye,
  failed: icons.x,
  running: icons.run,
  ready: icons.ok,
  waiting: icons.clock,
  draft: icons.draft,
}

/** A check's icon, by where it stands. */
export const checkIcon: Record<CheckState, ReactNode> = {
  pass: icons.ok,
  fail: icons.x,
  run: icons.run,
  queue: icons.queue,
  skip: icons.skip,
}

/** A list's icon in the rail and its heading: your own lists only. */
export function laneIcon(kind: ListKind): ReactNode {
  return { focus: icons.star, inbox: icons.tray, later: icons.moon, mine: null, search: null }[kind]
}
