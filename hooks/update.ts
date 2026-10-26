// Saying when a newer pr-watch is out. Claude Code tells no one of a third-party marketplace's
// new version, and a folder-loaded copy gets no updates at all, so the mod says it itself: the
// poller's round also reads this repo's plugin.json (batch.ts, at most every CHECK_MS, no call
// of its own), and each session compares that version with its own manifest's.
import { cells } from './review'

// How often a round also asks for the newest version.
export const CHECK_MS = 2 * 3_600_000

// The field a round adds beside its PRs: plugin.json on the default branch. The repo is
// public, so any gh account reads it. Renamed, gone or private, GitHub answers it with null
// and an error of its own, and the PRs beside it come through as ever.
export const LATEST_FIELD =
  'pw: repository(owner: "ElirazKed", name: "claude-code-pr-watch") { object(expression: "HEAD:.claude-plugin/plugin.json") { ... on Blob { text } } }'

type Version = readonly [number, number, number]

// Plain major.minor.patch only: anything else (a pre-release, a typo) is never compared.
function parse(text: string): Version | null {
  const m = text.trim().match(/^(\d+)\.(\d+)\.(\d+)$/)

  return m === null ? null : [Number(m[1]), Number(m[2]), Number(m[3])]
}

// A manifest's version, from its text; null when it isn't JSON or has no version to compare.
export function versionIn(manifest: unknown): string | null {
  if (typeof manifest !== 'string') return null
  try {
    const version = (JSON.parse(manifest) as { version?: unknown } | null)?.version

    return typeof version === 'string' && parse(version) !== null ? version.trim() : null
  } catch {
    return null
  }
}

// The newest version in a reply's `pw` field; null when there is none to read.
export function latestIn(field: unknown): string | null {
  return versionIn((field as { object?: { text?: unknown } | null } | null | undefined)?.object?.text)
}

// Whether `latest` is a later version than `own`; false when either won't parse.
export function isNewer(latest: string, own: string): boolean {
  const a = parse(latest)
  const b = parse(own)
  if (a === null || b === null) return false
  const i = a.findIndex((n, j) => n !== b[j])

  return i !== -1 && (a[i] ?? 0) > (b[i] ?? 0)
}

// How this copy updates, by where it was loaded from. An install from a marketplace sits in
// Claude Code's plugin cache (<config dir>/plugins/cache/<marketplace>/<plugin>/<version>/);
// anything else is a folder (--plugin-dir, CLAUDE_CODE_PLUGIN_DIRS), most likely a clone.
export function howToUpdate(root: string, home: string | null, name: string): string {
  const path = root.replace(/\\/g, '/')
  if (path.includes('/plugins/cache/')) {
    const market = path.match(/\/plugins\/cache\/([^/]+)\/[^/]+/)?.[1]

    return market === undefined ? `/plugin → Installed → ${name} → Update now` : `claude plugin update ${name}@${market}`
  }
  const base = home?.replace(/\\/g, '/').replace(/\/+$/, '') ?? ''
  const isHome = base !== '' && (path === base || path.startsWith(`${base}/`))

  return `git pull in ${isHome ? `~${path.slice(base.length)}` : path}`
}

// "↑ pr-watch 0.9.0 available": what's out, without how to get it (the band's short form).
export const headOf = (version: string) => `↑ pr-watch ${version} available`

export const noticeToast = (version: string, how: string) => `${headOf(version)} · ${how}`

// The footer's note: one line where it fits, else what's out above how to get it.
export function noticeLines(version: string, how: string, columns: number): string[] {
  const line = noticeToast(version, how)

  return cells(line) <= columns ? [line] : [headOf(version), how]
}

// Telling a mod user about prw, the companion app: a card in the pane (a line in the band)
// that stays until it is dismissed, on every session of the machine. companion.json in the
// cache keeps why it is gone (dismissed, or prw seen running), so it never comes back.
export const PRW_URL = 'https://elirazked.github.io/claude-code-pr-watch/prw.html'
export const PRW_PILL = '✦ NEW'
export const PRW_TEXT = "all your PRs in one terminal. Yours, your searches, your agents'. Review, reply, resolve and merge without opening GitHub."
export const PRW_LINE = 'New: prw, all your PRs in one terminal'

export type CompanionState = 'dismissed' | 'seen'

// What companion.json says, from its parsed text; null for none (or a file it can't read).
export function companionIn(file: unknown): CompanionState | null {
  const state = (file as { state?: unknown } | null | undefined)?.state

  return state === 'dismissed' || state === 'seen' ? state : null
}

// Whether prw has been running here: it writes its session files as `prw-<id>…json`.
export const isPrwIn = (names: readonly string[]) => names.some(name => name.startsWith('prw-') && name.endsWith('.json'))
