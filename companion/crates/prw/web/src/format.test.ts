import { describe, expect, it } from 'vitest'
import { ago, byRepo, matches, plural } from './format'
import type { Row } from './types'

describe('ago', () => {
  const now = 10 * 86_400_000
  it('counts minutes, then hours, then days', () => {
    expect(ago(now, now - 30_000)).toBe('now')
    expect(ago(now, now - 4 * 60_000)).toBe('4m')
    expect(ago(now, now - 3 * 3_600_000)).toBe('3h')
    expect(ago(now, now - 2 * 86_400_000)).toBe('2d')
  })
  it('says nothing for a time not known, and now for one ahead', () => {
    expect(ago(now, null)).toBe('')
    expect(ago(now, now + 60_000)).toBe('now')
  })
})

describe('plural', () => {
  it('adds an s but to one', () => {
    expect(plural(1, 'check')).toBe('1 check')
    expect(plural(3, 'check')).toBe('3 checks')
  })
})

describe('matches', () => {
  const row = {
    title: 'Retry uploads',
    repo: 'acme/app',
    branch: 'feat/upload-retry',
    author: 'octocat',
    number: 412,
    agent: 'claude',
  } as Row
  it('finds a row by any of its names', () => {
    for (const q of ['retry', 'ACME/app', '#412', 'octo', 'claude', '  ']) {
      expect(matches(row, q)).toBe(true)
    }
    expect(matches(row, 'billing')).toBe(false)
  })
})

describe('byRepo', () => {
  const at = (repo: string, number: number) => ({ repo, number }) as Row
  it('goes A to Z by name, namesakes together by owner, each keeping its rows in order', () => {
    const rows = [at('zed/web', 1), at('acme/app', 2), at('acme/web', 3), at('acme/app', 4)]
    expect(byRepo(rows).map((g) => [g.repo, g.rows.map((r) => r.number)])).toEqual([
      ['acme/app', [2, 4]],
      ['acme/web', [3]],
      ['zed/web', [1]],
    ])
  })
  it('puts a name before a longer one it starts', () => {
    expect(byRepo([at('a/webapp', 1), at('z/web', 2)]).map((g) => g.repo)).toEqual([
      'z/web',
      'a/webapp',
    ])
  })
})
