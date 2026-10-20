// What results.json keeps of each PR, and what a failed fetch and a round do to it.
import type { GhPr } from './pr'

// `at`: when `pr` was fetched, or, with no `pr` (never read), when the fetch last failed.
// `failedSince`: the first failed fetch in a row, while the last data still stands; `error`:
// what the card says, once it says anything. `isUnreachable`: never read, and no gh account
// could see it (not a failure that passes). Sessions of an older version spread an entry,
// so a field they don't know is no harm to them.
export type Entry = { at: number; pr?: GhPr; error?: string; failedSince?: number; isUnreachable?: true }

// What a PR no logged-in gh account could see said before the card named the logins asked
// (accounts.ts's notFoundAs): an older version's entry may still say it.
export const NOT_FOUND = 'Not found, or no gh account can see it'

// A PR no account can see is asked about again only this often. A fetch that failed on the
// way (a 502, a cut connection) is no such miss: the next round asks again.
export const UNREACHABLE_MS = 300_000

// Whether a PR was never read because no account could see it: its card says who was asked,
// and offers Try again. An older version's entry has no flag, only the words.
export function isMiss(entry: Entry | undefined): boolean {
  return entry !== undefined && entry.pr === undefined && (entry.isUnreachable === true || entry.error === NOT_FOUND)
}

// Whether a round leaves this PR out: a miss (isMiss) UNREACHABLE_MS ago at most.
export function isUnreachable(entry: Entry | undefined, at: number): boolean {
  return entry !== undefined && isMiss(entry) && at - entry.at < UNREACHABLE_MS
}

// How long fetches may fail before a card that has data says so. GitHub's gateway drops a
// query now and then (a 502 or 504) and the next round gets through; until then the card
// keeps its last data, and its "checked 1m ago" says how old that is. Two minutes is three
// idle rounds, or a few more busy ones, and the card then reads "checked 2m ago" beside it.
export const QUIET_MS = 120_000

// A PR's entry after a fetch at `at` failed with `error`. One never read says so at once, as
// before; one with data keeps it and its time, and shows the error once failing QUIET_MS.
// An older version's entry has an error and no failedSince: its `at` is when it failed.
// `isMiss`: no gh account could see it, which a round then skips a while (isUnreachable).
export function failedEntry(entry: Entry | undefined, error: string, at: number, isMiss = false): Entry {
  if (entry?.pr === undefined) return isMiss ? { at, error, isUnreachable: true } : { at, error }
  const { error: was, failedSince, isUnreachable: _, ...kept } = entry
  const since = failedSince ?? (was !== undefined ? entry.at : at)

  return at - since >= QUIET_MS ? { ...kept, failedSince: since, error } : { ...kept, failedSince: since }
}

// How long a round keeps a PR no live session watches, after its last fetch: a session
// back from a restart within the hour still finds its PRs.
export const KEEP_MS = 3_600_000

// What a round keeps: every PR a live session watches, however old (a merged one is never
// asked again, so its entry is what says it merged), and the rest until KEEP_MS old.
export function keptEntries(prs: Readonly<Record<string, Entry>>, live: ReadonlySet<string>, at: number): Record<string, Entry> {
  return Object.fromEntries(Object.entries(prs).filter(([url, entry]) => live.has(url) || at - entry.at < KEEP_MS))
}
