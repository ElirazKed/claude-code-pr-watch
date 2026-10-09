import { describe, expect, test } from 'claude-code/testing'

import {
  COMMENT_CHARS,
  REVIEW_CHARS,
  THREAD_CHARS,
  addressHeading,
  addressPrompt,
  cleanBody,
  openThreads,
  parseThreads,
  redraft,
  spot,
  threadsQuery,
} from './address'
import type { GhThread } from './address'
import { fixHeading } from './fix'

const PR = {
  repo: 'acme/widgets',
  number: 12,
  title: 'Retry uploads with exponential backoff',
  url: 'https://github.com/acme/widgets/pull/12',
  branch: 'feat/retry',
  base: 'main',
  author: 'octocat',
}

let ids = 100
// A thread on `path` at `line`, its comments given as [login, body]: open, current, single-line.
const thread = (path: string, line: number | null, comments: [string, string][], extra: Partial<GhThread> = {}): GhThread => ({
  isResolved: false,
  isOutdated: false,
  path,
  line,
  startLine: null,
  originalLine: line,
  originalStartLine: null,
  comments: {
    totalCount: comments.length,
    nodes: comments.map(([login, body]) => ({ author: { login }, body, url: `${PR.url}#discussion_r${(ids += 1)}` })),
  },
  ...extra,
})

describe('which threads go', () => {
  test('resolved threads are done; the rest go by file and line', async () => {
    const threads = [
      thread('src/upload.ts', 40, [['alice', 'b']]),
      thread('src/api.ts', 9, [['bob', 'a']]),
      thread('src/upload.ts', 7, [['alice', 'done']], { isResolved: true }),
      thread('src/upload.ts', 3, [['carol', 'c']]),
    ]
    expect(openThreads(threads).map(t => `${t.path}:${t.line}`)).toEqual(['src/api.ts:9', 'src/upload.ts:3', 'src/upload.ts:40'])
  })

  test('every thread resolved: nothing to hand over', async () => {
    expect(openThreads([thread('src/upload.ts', 3, [['alice', 'x']], { isResolved: true })])).toEqual([])
  })
})

describe('where a thread is', () => {
  test('a line, a range, and a comment on the whole file', async () => {
    expect(spot(thread('src/upload.ts', 42, []))).toBe('src/upload.ts:42')
    expect(spot(thread('src/upload.ts', 42, [], { startLine: 40 }))).toBe('src/upload.ts:40-42')
    expect(spot(thread('src/upload.ts', 42, [], { startLine: 42 }))).toBe('src/upload.ts:42')
    expect(spot(thread('README.md', null, [], { originalLine: null }))).toBe('README.md (the whole file)')
  })

  test('outdated: where it was left, or where GitHub still places it', async () => {
    expect(spot(thread('src/upload.ts', null, [], { isOutdated: true, originalLine: 7 }))).toBe('src/upload.ts (outdated; was line 7)')
    expect(spot(thread('src/upload.ts', null, [], { isOutdated: true, originalStartLine: 5, originalLine: 7 }))).toBe(
      'src/upload.ts (outdated; was lines 5-7)',
    )
    expect(spot(thread('src/upload.ts', 12, [], { isOutdated: true }))).toBe('src/upload.ts:12 (outdated)')
    expect(spot(thread('src/upload.ts', null, [], { isOutdated: true, originalLine: null }))).toBe('src/upload.ts (outdated)')
  })
})

describe('a comment', () => {
  test("loses review apps' HTML comments and runs of blank lines", async () => {
    expect(cleanBody('<!-- bot:meta {"id":1} -->\r\nUse the config value.\n\n\n  \nThanks!\n')).toBe('Use the config value.\n\nThanks!')
  })

  test('is clipped when long, and says so', async () => {
    const body = cleanBody('x'.repeat(5_000))
    expect(body).toHaveLength(COMMENT_CHARS)
    expect(body.endsWith('…')).toBe(true)
  })

  test('with no words left still says something', async () => {
    expect(cleanBody('<!-- only metadata -->')).toBe('(no text)')
  })
})

describe('the message for Claude', () => {
  test('names the PR, lists each open thread with its replies, marks outdated ones, and asks', async () => {
    const text = addressPrompt(PR, [
      thread('src/upload.ts', 42, [
        ['alice', 'Retry count should come from config,\nnot a literal.'],
        ['octocat', 'Agreed, moving it.'],
      ], { startLine: 40 }),
      thread('src/upload.ts', null, [['bob', 'Is this still needed?']], { isOutdated: true, originalLine: 7 }),
      thread('src/api.ts', 9, [['carol', 'Already fixed.']], { isResolved: true }),
    ])

    expect(text).toContain('Review comments to address on acme/widgets#12: Retry uploads with exponential backoff')
    expect(text).toContain('https://github.com/acme/widgets/pull/12 · feat/retry → main · files: https://github.com/acme/widgets/pull/12/files')
    expect(text).toContain('2 unresolved review threads, by file (1 outdated: the code changed since, so check it still applies):')
    expect(text).toMatch(/1\. src\/upload\.ts \(outdated; was line 7\) · https:\/\/github\.com\/acme\/widgets\/pull\/12#discussion_r\d+\n   @bob: Is this still needed\?/)
    expect(text).toMatch(/2\. src\/upload\.ts:40-42 · https:\/\/github\.com\/acme\/widgets\/pull\/12#discussion_r\d+/)
    // A body's later lines sit deeper than the next comment's author.
    expect(text).toContain('   @alice: Retry count should come from config,\n     not a literal.\n   @octocat (PR author): Agreed, moving it.')
    expect(text).not.toContain('Already fixed')
    expect(text).toContain("Address each on this branch (feat/retry): change the code where a comment is right, and where you'd disagree")
    expect(text).toContain("If acme/widgets or feat/retry isn't checked out here, say so first.")
    expect(text).toContain("Don't reply to or resolve any thread on GitHub: if one needs a reply, draft it here for me to read first.")
    expect(text).not.toMatch(/\bpush\b|\bmerge\b/i)
  })

  test('replies past those read are counted, not lost silently', async () => {
    const t = thread('src/upload.ts', 3, [['alice', 'one']])
    const text = addressPrompt(PR, [{ ...t, comments: { ...t.comments, totalCount: 13 } }])
    expect(text).toContain('   (12 more replies, not read)')
  })

  test("a body's blank lines stay blank", async () => {
    const text = addressPrompt(PR, [thread('src/upload.ts', 3, [['alice', 'First.\n\nSecond.']])])
    expect(text).toContain('   @alice: First.\n\n     Second.')
  })

  test('one long thread is clipped, and every thread together stays inside the cap', async () => {
    const long = Array.from({ length: 10 }, (_, i): [string, string] => [`reviewer${i}`, 'y'.repeat(900)])
    const threads = Array.from({ length: 20 }, (_, i) => thread(`src/file${String(i).padStart(2, '0')}.ts`, i + 1, long))
    const text = addressPrompt(PR, threads)
    const blocks = [...text.matchAll(/^\d+\. [\s\S]*?(?=\n\n)/gm)].map(m => m[0])

    expect(blocks.length).toBeGreaterThan(0)
    expect(blocks.length).toBeLessThan(20)
    for (const block of blocks) expect(block.length).toBeLessThanOrEqual(THREAD_CHARS)
    expect(blocks.reduce((n, block) => n + block.length, 0)).toBeLessThanOrEqual(REVIEW_CHARS)
    expect(text).toContain(`${20 - blocks.length} more unresolved threads didn't fit here; they're on the PR's files tab.`)
  })

  test('more threads on the PR than were read says so', async () => {
    const text = addressPrompt(PR, [thread('src/upload.ts', 3, [['alice', 'x']])], 140)
    expect(text).toContain('Only the newest 1 of 140 threads were read; older ones may be open too.')
  })

  test('a deleted account still has a name', async () => {
    const t = thread('src/upload.ts', 3, [['alice', 'x']])
    const nodes = t.comments.nodes.map(c => ({ ...c, author: null }))
    expect(addressPrompt(PR, [{ ...t, comments: { nodes } }])).toContain('   @ghost: x')
  })
})

describe('asking GitHub', () => {
  test('the threads query is the batch alias shape, for this PR alone, with what each comment said', async () => {
    const query = threadsQuery({ url: PR.url, repo: 'acme/widgets', number: 12 })
    expect(query).toContain('p0: repository(owner: "acme", name: "widgets") { pullRequest(number: 12)')
    expect(query).toContain('reviewThreads(last: 100) { totalCount nodes {')
    expect(query).toContain('isResolved isOutdated path line startLine originalLine originalStartLine')
    expect(query).toContain('comments(first: 10) { totalCount nodes { author { login } body url } }')
  })

  test("the reply's threads and total; or why there are none", async () => {
    const nodes = [thread('src/upload.ts', 3, [['alice', 'x']])]
    const ok = JSON.stringify({ data: { p0: { pullRequest: { reviewThreads: { totalCount: 4, nodes } } } } })
    expect(parseThreads(ok, '')).toEqual({ threads: nodes, total: 4 })
    expect(parseThreads(JSON.stringify({ data: { p0: null } }), '')).toEqual({ error: "Not found, or this gh account can't see it" })
    expect(parseThreads(JSON.stringify({ errors: [{ message: 'API rate limit exceeded' }] }), '')).toEqual({ error: 'API rate limit exceeded' })
    expect(parseThreads('', 'HTTP 401: Bad credentials\n')).toEqual({ error: 'HTTP 401: Bad credentials' })
  })
})

describe('drafting into the prompt box', () => {
  const fix = `${fixHeading(PR)} Retry uploads\nthe logs`
  const address = `${addressHeading(PR)} Retry uploads\nthe threads`

  test('an empty box takes the draft; text the person typed keeps it after', async () => {
    expect(redraft('', addressHeading(PR), address)).toEqual({ text: address, mode: 'replace' })
    expect(redraft('Look at this first.', addressHeading(PR), address)).toEqual({ text: `\n\n${address}`, mode: 'append' })
  })

  test("a second press replaces its own draft, and keeps the person's text and the other hand-off's draft", async () => {
    const box = `Look at this first.\n\n${address}\n\n${fix}`
    const again = `${addressHeading(PR)} Retry uploads\nnewer threads`
    expect(redraft(box, addressHeading(PR), again)).toEqual({ text: `Look at this first.\n\n${again}\n\n${fix}`, mode: 'replace' })
    expect(redraft(`${fix}\n\n${address}`, fixHeading(PR), `${fixHeading(PR)} newer logs`)).toEqual({
      text: `${fixHeading(PR)} newer logs\n\n${address}`,
      mode: 'replace',
    })
  })
})
