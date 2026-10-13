import { describe, expect, test } from 'claude-code/testing'

import { addressHeading, redraft } from './address'
import { fixHeading } from './fix'
import { resolveHeading, resolvePrompt } from './resolve'

const PR = {
  repo: 'acme/app',
  number: 21,
  title: 'Retry uploads with exponential backoff',
  url: 'https://github.com/acme/app/pull/21',
  branch: 'feat/retry',
  base: 'main',
}

describe('the ask to resolve a conflict', () => {
  test('starts with its heading and the title, then the link and branch → base', async () => {
    const lines = resolvePrompt(PR).split('\n')

    expect(resolveHeading(PR)).toBe('Merge conflicts on acme/app#21:')
    expect(lines[0]).toBe('Merge conflicts on acme/app#21: Retry uploads with exponential backoff')
    expect(lines[1]).toBe('https://github.com/acme/app/pull/21 · feat/retry → main')
    expect(resolvePrompt(PR)).toContain("GitHub says feat/retry can't be merged cleanly into main.")
  })

  test('asks to merge the base in, not rebase, and to ask first where the branch works by rebase', async () => {
    const text = resolvePrompt(PR)

    expect(text).toContain('fetch, then merge origin/main into it, so no history is rewritten and no force-push is needed')
    expect(text).toContain('If the branch or repo clearly works by rebase instead, say so and ask me first.')
    // A fork's clone has the fork as origin, whose base may lag the PR's.
    expect(text).toContain("If origin isn't acme/app (a fork's clone), merge main from the remote that is.")
    expect(text).not.toMatch(/rebase (?:it|the branch|onto)/i)
  })

  test('asks to keep both sides, to ask where intent is unclear, to run the checks and commit, and never to push', async () => {
    const text = resolvePrompt(PR)

    expect(text).toContain('keeping what both sides meant')
    expect(text).toContain('ask me rather than pick one')
    expect(text).toContain("run the project's checks and tests, and commit the merge")
    expect(text).toContain("Don't push: that stays my call.")
    expect(text).not.toMatch(/\bgit push\b|force-push it|merge the PR/i)
    expect(text).toContain("If acme/app or feat/retry isn't checked out here, say so first.")
  })

  test('a PR not read yet still reads as a sentence', async () => {
    const text = resolvePrompt({ ...PR, title: '', branch: '', base: '' })

    expect(text.split('\n')[0]).toBe('Merge conflicts on acme/app#21: untitled')
    expect(text).toContain('its head branch → its base')
    expect(text).toContain('merge the base from origin into it')
  })
})

describe('drafting beside the other hand-offs', () => {
  const fix = `${fixHeading(PR)} Retry uploads\nthe logs`
  const address = `${addressHeading(PR)} Retry uploads\nthe threads`
  const resolve = resolvePrompt(PR)

  test("a second press replaces its own draft, and keeps the person's text and the others' drafts", async () => {
    const again = `${resolveHeading(PR)} Retry uploads\nnewer`
    expect(redraft(`Look at this first.\n\n${resolve}\n\n${fix}`, resolveHeading(PR), again)).toEqual({
      text: `Look at this first.\n\n${again}\n\n${fix}`,
      mode: 'replace',
    })
    // Another hand-off's draft ends where a resolve draft starts.
    expect(redraft(`${address}\n\n${resolve}`, addressHeading(PR), `${addressHeading(PR)} newer threads`)).toEqual({
      text: `${addressHeading(PR)} newer threads\n\n${resolve}`,
      mode: 'replace',
    })
  })
})
