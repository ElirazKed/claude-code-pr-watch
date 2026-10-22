import { describe, expect, test } from 'claude-code/testing'

import { AGAIN_MS, KEEP_MS, NO_LOGINS, bestLogin, everyLogin, isDue, loginFor, loginsOf, notFoundAs, ownersBelowWrite, rankOf, tokenError, untriedBy } from './accounts'

const T = Date.parse('2026-10-07T12:00:00Z')

// `gh auth status` as gh prints it, with two logins, the second one active.
const STATUS = [
  'github.com',
  '  ✓ Logged in to github.com account octocat (keyring)',
  '  - Active account: false',
  '  - Git operations protocol: ssh',
  "  - Token: gho_************************************",
  '',
  '  ✓ Logged in to github.com account hubot (keyring)',
  '  - Active account: true',
  '  - Git operations protocol: ssh',
].join('\n')

describe("gh's logins", () => {
  test('the active one apart from the rest, whatever order gh lists them in', async () => {
    expect(loginsOf(STATUS)).toEqual({ active: 'hubot', others: ['octocat'] })
    expect(everyLogin({ active: 'hubot', others: ['octocat'] })).toEqual(['hubot', 'octocat'])
  })

  test('one whose token gh calls invalid is still a login', async () => {
    const text = `${STATUS}\n  X Failed to log in to github.com account monalisa (keyring)\n  - Active account: false\n  - The token in keyring is invalid.\n`
    expect(loginsOf(text)).toEqual({ active: 'hubot', others: ['octocat', 'monalisa'] })
  })

  test('a status that lists none (gh timed out, or nobody is logged in) is no reading', async () => {
    expect(loginsOf('')).toBe(null)
    expect(loginsOf('You are not logged into any GitHub hosts. To log in, run: gh auth login')).toBe(null)
    expect(everyLogin(NO_LOGINS)).toEqual([])
  })

  test('none marked active: the first stands for it', async () => {
    expect(loginsOf(STATUS.replace('Active account: true', 'Active account: false'))).toEqual({ active: 'octocat', others: ['hubot'] })
  })
})

describe('when gh is asked again', () => {
  test('never asked: now', async () => {
    expect(isDue(undefined, T)).toBe(true)
  })

  test('a good answer stands KEEP_MS, a failure AGAIN_MS', async () => {
    expect(isDue({ at: T, isOk: true }, T + KEEP_MS - 1)).toBe(false)
    expect(isDue({ at: T, isOk: true }, T + KEEP_MS)).toBe(true)
    expect(isDue({ at: T, isOk: false }, T + AGAIN_MS - 1)).toBe(false)
    expect(isDue({ at: T, isOk: false }, T + AGAIN_MS)).toBe(true)
  })
})

describe('which login asks', () => {
  const logins = { active: 'octocat', others: ['hubot'] }

  test("an owner's PRs go as the login found to see them, others as the active one", async () => {
    expect(loginFor({ acme: 'hubot' }, 'acme', logins)).toBe('hubot')
    expect(loginFor({ acme: 'hubot' }, 'globex', logins)).toBe('octocat')
  })

  test('a remembered login that has logged out since gives way to the active one', async () => {
    expect(loginFor({ acme: 'monalisa' }, 'acme', logins)).toBe('octocat')
  })

  test("with gh's logins unknown, the remembered one stands", async () => {
    expect(loginFor({ acme: 'hubot' }, 'acme', NO_LOGINS)).toBe('hubot')
    expect(loginFor({}, 'acme', NO_LOGINS)).toBe('')
  })

  test("an owner of a login's name goes as that login, before the active one", async () => {
    expect(loginFor({}, 'Hubot', logins)).toBe('hubot')
    // A login remembered for it still stands.
    expect(loginFor({ hubot: 'octocat' }, 'hubot', logins)).toBe('octocat')
  })

  test('an owner seen only to read, with nobody remembered for it, is asked as the others', async () => {
    const seen = [
      { owner: 'acme', permission: 'READ' },
      { owner: 'globex', permission: 'READ' },
      { owner: 'globex', permission: 'WRITE' },
      { owner: 'initech', permission: undefined },
      { owner: 'umbrella', permission: 'READ' },
    ]
    expect(ownersBelowWrite(seen, { umbrella: 'hubot' }, logins)).toEqual(['acme', 'initech'])
    expect(ownersBelowWrite(seen, {}, { active: 'octocat', others: [] })).toEqual([])
    expect(rankOf('ADMIN')).toBeGreaterThan(rankOf('WRITE'))
    expect(rankOf(null)).toBe(0)
  })

  test('an owner is kept as a login that sees all its PRs with more rights, else the first', async () => {
    const asked = { login: 'octocat', seen: 2, rank: rankOf('READ') }
    expect(bestLogin([asked, { login: 'hubot', seen: 2, rank: rankOf('ADMIN') }], 2)).toBe('hubot')
    // Seeing fewer of them, more rights are no reason to move.
    expect(bestLogin([asked, { login: 'hubot', seen: 1, rank: rankOf('ADMIN') }], 2)).toBe('octocat')
    expect(bestLogin([asked, { login: 'hubot', seen: 2, rank: rankOf('READ') }], 2)).toBe('octocat')
    expect(bestLogin([], 0)).toBe('')
  })

  test('a login is asked only about the missing PRs it has not answered for', async () => {
    const missing = [{ url: 'a' }, { url: 'b' }]
    const asked = new Map([['a', ['octocat']]])
    expect(untriedBy(missing, asked, 'octocat')).toEqual([{ url: 'b' }])
    expect(untriedBy(missing, asked, 'hubot')).toEqual(missing)
  })
})

describe('what the card says', () => {
  test('the logins asked, in the order they were', async () => {
    expect(notFoundAs(['octocat'])).toBe('Not found as octocat')
    expect(notFoundAs(['octocat', 'hubot'])).toBe('Not found as octocat or hubot')
    expect(notFoundAs(['octocat', 'hubot', 'monalisa', 'hubot'])).toBe('Not found as octocat, hubot or monalisa')
    expect(notFoundAs([])).toBe('Not found')
  })

  test("a token gh won't give names its login; no logins at all says gh auth status", async () => {
    expect(tokenError('hubot')).toBe("Couldn't get hubot's gh token")
    expect(tokenError('')).toBe("Couldn't read gh's logins (gh auth status)")
  })
})
