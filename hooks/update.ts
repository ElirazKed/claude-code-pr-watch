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
  const isHome = home !== null && home !== '' && (path === home || path.startsWith(`${home}/`))

  return `git pull in ${isHome ? `~${path.slice(home.length)}` : path}`
}

const headOf = (version: string) => `↑ pr-watch ${version} available`

// The footer's note: one line where it fits, else what's out above how to get it.
export function noticeLines(version: string, how: string, columns: number): string[] {
  const line = `${headOf(version)} · ${how}`

  return cells(line) <= columns ? [line] : [headOf(version), how]
}

export const noticeToast = (version: string, how: string) => `${headOf(version)} · ${how}`
