// Handing a PR's open review threads to Claude: where each one is, who said what, and the
// message that asks to address them. Pure, so the pane only fetches and delivers.
import type { TrackedPr } from '../types'
import { clip } from './fix'
import type { PrRef } from './pr'

// The newest threads read on a press, the comments read of each (the first and its replies),
// what one comment and one thread may take, and the characters every thread together may
// take: enough to act on, not a transcript of the whole review.
export const THREADS = 100
export const THREAD_COMMENTS = 10
export const COMMENT_CHARS = 1_000
export const THREAD_CHARS = 3_000
export const REVIEW_CHARS = 12_000

export type GhComment = { author: { login: string } | null; body: string; url: string }

// A review thread as GitHub has it. `line` and `startLine` are where it sits on the PR's code
// now (null once outdated, mostly); the `original` pair, where it was left.
export type GhThread = {
  isResolved: boolean
  isOutdated: boolean
  path: string
  line: number | null
  startLine: number | null
  originalLine: number | null
  originalStartLine: number | null
  comments: { totalCount?: number; nodes: GhComment[] }
}

// One PR's review threads with what they say. Asked only on a press, never by the poller,
// whose query only counts them; the alias shape is batch.ts's.
export function threadsQuery(ref: PrRef): string {
  const [owner = '', name = ''] = ref.repo.split('/')

  return `query {
  p0: repository(owner: ${JSON.stringify(owner)}, name: ${JSON.stringify(name)}) { pullRequest(number: ${ref.number}) {
    reviewThreads(last: ${THREADS}) { totalCount nodes {
      isResolved isOutdated path line startLine originalLine originalStartLine
      comments(first: ${THREAD_COMMENTS}) { totalCount nodes { author { login } body url } }
    } }
  } }
}`
}

type Reply = {
  data?: { p0?: { pullRequest?: { reviewThreads?: { totalCount?: number; nodes?: GhThread[] } | null } | null } | null } | null
  errors?: { message?: string }[]
}

const firstLine = (text: string) => text.trim().split('\n')[0] ?? ''

// The threads gh printed, and how many the PR has in all; or why there are none.
export function parseThreads(stdout: string, stderr: string): { threads: GhThread[]; total: number } | { error: string } {
  let reply: Reply
  try {
    reply = JSON.parse(stdout) as Reply
  } catch {
    return { error: firstLine(stderr) || 'gh api graphql failed' }
  }
  if (reply.data == null) return { error: reply.errors?.[0]?.message ?? (firstLine(stderr) || 'GitHub returned no data') }
  const threads = reply.data.p0?.pullRequest?.reviewThreads
  if (threads == null) return { error: "Not found, or this gh account can't see it" }
  const nodes = threads.nodes ?? []

  return { threads: nodes, total: threads.totalCount ?? nodes.length }
}

const lineOf = (thread: GhThread) => thread.line ?? thread.originalLine ?? 0

// The threads still to address, by file and line: a resolved one is done, an outdated one may
// not be.
export function openThreads(threads: readonly GhThread[]): GhThread[] {
  return threads
    .filter(thread => !thread.isResolved)
    .sort((a, b) => (a.path === b.path ? lineOf(a) - lineOf(b) : a.path < b.path ? -1 : 1))
}

const span = (start: number | null, end: number | null) => (end === null ? '' : start === null || start === end ? `${end}` : `${start}-${end}`)

// Where a thread is: "src/upload.ts:42", "src/upload.ts:40-42", "src/upload.ts (outdated; was
// line 42)", or "src/upload.ts (the whole file)" for a comment on the file.
export function spot(thread: GhThread): string {
  const now = span(thread.startLine, thread.line)
  if (!thread.isOutdated) return now === '' ? `${thread.path} (the whole file)` : `${thread.path}:${now}`
  if (now !== '') return `${thread.path}:${now} (outdated)`
  const was = span(thread.originalStartLine, thread.originalLine)

  return was === '' ? `${thread.path} (outdated)` : `${thread.path} (outdated; was line${was.includes('-') ? 's' : ''} ${was})`
}

// A comment as Claude needs it: without the HTML comments review apps keep their bookkeeping
// in, or runs of blank lines, and clipped.
export function cleanBody(body: string): string {
  const text = body
    .replace(/\r/g, '')
    .replace(/<!--[\s\S]*?-->/g, '')
    .replace(/\n[ \t]*\n(?:[ \t]*\n)+/g, '\n\n')
    .trim()

  return clip(text, COMMENT_CHARS) || '(no text)'
}

type PrFacts = Pick<TrackedPr, 'repo' | 'number' | 'title' | 'url' | 'branch' | 'base' | 'author'>

// The draft's first words, by which a press finds an earlier draft of its own in the prompt box.
export const addressHeading = (pr: Pick<PrFacts, 'repo' | 'number'>) => `Review comments to address on ${pr.repo}#${pr.number}:`

// One thread, numbered: where it is and its link, then each comment under it by its author,
// a body's later lines (not its blank ones) indented further so a reply can't pass for one.
function threadBlock(thread: GhThread, n: number, author: string): string {
  const comments = thread.comments.nodes
  const head = `${n}. ${spot(thread)} · ${comments[0]?.url ?? 'no link'}`
  const said = comments.map(c => {
    const login = c.author?.login ?? 'ghost'
    const who = login === author ? `@${login} (PR author)` : `@${login}`

    return `   ${who}: ${cleanBody(c.body).replace(/\n(?!\n)/g, '\n     ')}`
  })
  const unread = (thread.comments.totalCount ?? comments.length) - comments.length
  if (unread > 0) said.push(`   (${unread} more ${unread === 1 ? 'reply' : 'replies'}, not read)`)

  return clip([head, ...said].join('\n'), THREAD_CHARS)
}

// The message for Claude: the PR, each open thread (outdated ones marked), and the ask. It
// never asks for a push or a merge, nor for a word on GitHub: replies and resolving stay the
// person's call.
export function addressPrompt(pr: PrFacts, threads: readonly GhThread[], total = threads.length): string {
  const open = openThreads(threads)
  const branch = pr.branch || 'its head branch'
  const outdated = open.filter(thread => thread.isOutdated).length
  const blocks: string[] = []
  let size = 0
  for (const thread of open) {
    const block = threadBlock(thread, blocks.length + 1, pr.author)
    if (blocks.length > 0 && size + block.length > REVIEW_CHARS) break
    blocks.push(block)
    size += block.length + 2
  }
  const count = `${open.length} unresolved review thread${open.length === 1 ? '' : 's'}`
  const stale = outdated === 0 ? '' : ` (${outdated} outdated: the code changed since, so check ${outdated === 1 ? 'it' : 'each'} still applies)`
  const out = [
    `${addressHeading(pr)} ${pr.title || 'untitled'}`,
    `${pr.url} · ${branch} → ${pr.base || 'its base'} · files: ${pr.url}/files`,
    '',
    `${count}, by file${stale}:`,
    '',
    blocks.join('\n\n'),
  ]
  const unshown = open.length - blocks.length
  if (unshown > 0) out.push('', `${unshown} more unresolved thread${unshown === 1 ? '' : 's'} didn't fit here; they're on the PR's files tab.`)
  if (total > threads.length) out.push('', `Only the newest ${threads.length} of ${total} threads were read; older ones may be open too.`)
  out.push(
    '',
    `Address each on this branch (${branch}): change the code where a comment is right, and where you'd disagree or a ` +
      `comment needs my call, say so instead. If ${pr.repo} or ${branch} isn't checked out here, say so first.`,
    "Don't reply to or resolve any thread on GitHub: if one needs a reply, draft it here for me to read first.",
  )

  return out.join('\n')
}

// Where a hand-off's draft starts, past the first: fixHeading's and addressHeading's first words.
const NEXT_DRAFT = /\n\n(?:CI failed|Review comments to address) on [\w.-]+\/[\w.-]+#\d+:/

// The prompt box with a hand-off drafted in: over an earlier draft of the same one (keeping the
// person's text before it and any other hand-off's draft after it), into an empty box, or
// after whatever is there.
export function redraft(box: string, heading: string, text: string): { text: string; mode: 'append' | 'replace' } {
  const at = box.trim() === '' || box.startsWith(heading) ? 0 : box.indexOf(`\n\n${heading}`)
  if (at === -1) return { text: `\n\n${text}`, mode: 'append' }
  const start = at === 0 ? 0 : at + 2
  const next = box.slice(start + heading.length).search(NEXT_DRAFT)
  const after = next === -1 ? '' : box.slice(start + heading.length + next)

  return { text: `${box.slice(0, start)}${text}${after}`, mode: 'replace' }
}
