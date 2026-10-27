// Commenting on a PR from its card: the gh call that posts it, what GitHub answers, and how a
// refusal reads on the card. Pure, so the pane only delivers.
import { tokenError } from './accounts'
import type { PrRef } from './pr'

// Only text with something in it posts: an empty or blank field does nothing.
export const isPostable = (text: string) => text.trim() !== ''

// A comment on the PR's conversation (an issue comment, not a review). The text goes as `-f`,
// gh's raw field, which is never read as a file or a flag: a text starting with `@` or `-`, or
// holding newlines, posts as typed. Not on stdin: a guard on mods may refuse a call with one.
export const commentArgv = (pr: Pick<PrRef, 'repo' | 'number'>, text: string) => [
  'api',
  '--method',
  'POST',
  `repos/${pr.repo}/issues/${pr.number}/comments`,
  '-f',
  `body=${text}`,
]

// The new comment's link, from GitHub's reply (null: the reply named none).
export function commentUrl(stdout: string): string | null {
  try {
    const url = (JSON.parse(stdout) as { html_url?: unknown } | null)?.html_url

    return typeof url === 'string' && url.startsWith('https://') ? url : null
  } catch {
    return null
  }
}

// gh's first line, in plain words: "gh: Unable to create comment because issue is locked.
// (HTTP 403)" is "Couldn't comment: Unable to create comment because issue is locked."
export function commentError(stderr: string, login: string): string {
  const text = (stderr.trim().split('\n')[0] ?? '').replace(/^gh:\s*/, '')
  const who = login === '' ? 'this account' : login
  // gh gave no token for the login (ghArgv's words): said as is.
  if (login !== '' && text === tokenError(login)) return text
  if (/HTTP 404/.test(text)) return `Couldn't comment: GitHub has no such PR, or ${who} can't see it`
  if (/HTTP 401/.test(text)) return `Couldn't comment: GitHub refused ${who}'s token; gh auth login renews it`
  const said = text.replace(/\s*\(HTTP \d{3}\)$/, '')

  return `Couldn't comment: ${said || 'gh api failed'}`
}

// The empty field's words: which PR, and which account will post.
export const commentPlaceholder = (pr: Pick<PrRef, 'number'>, login: string) => (login === '' ? `Comment on #${pr.number}` : `Comment on #${pr.number} as ${login}`)
