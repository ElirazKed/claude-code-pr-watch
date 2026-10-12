import { describe, expect, test } from 'claude-code/testing'

import { howToUpdate, isNewer, latestIn, noticeLines, noticeToast, versionIn } from './update'

const manifest = (version: unknown) => JSON.stringify({ name: 'pr-watch', version })

describe('versions', () => {
  test('a manifest says its version; anything not plain major.minor.patch says none', async () => {
    expect(versionIn(manifest('0.9.0'))).toBe('0.9.0')
    expect(versionIn(manifest('12.0.31'))).toBe('12.0.31')
    for (const odd of ['0.9', '0.9.0-beta.1', 'v0.9.0', 'latest', '', 9]) expect(versionIn(manifest(odd))).toBe(null)
    expect(versionIn(JSON.stringify({ name: 'pr-watch' }))).toBe(null)
    expect(versionIn('not json')).toBe(null)
    expect(versionIn('null')).toBe(null)
    expect(versionIn(undefined)).toBe(null)
  })

  test('newer by major, then minor, then patch, as numbers', async () => {
    expect(isNewer('0.9.0', '0.8.0')).toBe(true)
    expect(isNewer('0.8.1', '0.8.0')).toBe(true)
    expect(isNewer('1.0.0', '0.12.9')).toBe(true)
    // 10 is more than 9, though "10" sorts before "9".
    expect(isNewer('0.10.0', '0.9.0')).toBe(true)
  })

  test('the same or an older version is not newer', async () => {
    expect(isNewer('0.8.0', '0.8.0')).toBe(false)
    expect(isNewer('0.7.9', '0.8.0')).toBe(false)
    expect(isNewer('0.9.0', '1.0.0')).toBe(false)
  })

  test("a version that won't parse is never newer, nor older", async () => {
    expect(isNewer('0.9.0-beta', '0.8.0')).toBe(false)
    expect(isNewer('0.9.0', 'dev')).toBe(false)
    expect(isNewer('', '')).toBe(false)
  })

  test("the reply's pw field: the manifest's version, or none", async () => {
    expect(latestIn({ object: { text: manifest('0.9.0') } })).toBe('0.9.0')
    // The repo renamed, gone or private (null), the file moved (object null), or garbled.
    expect(latestIn(null)).toBe(null)
    expect(latestIn(undefined)).toBe(null)
    expect(latestIn({ object: null })).toBe(null)
    expect(latestIn({ object: { text: '{' } })).toBe(null)
  })
})

describe('how to update', () => {
  test('a marketplace install, from its place in the plugin cache', async () => {
    const root = '/home/me/.claude/plugins/cache/claude-code-pr-watch/pr-watch/0.8.0'
    expect(howToUpdate(root, '/home/me', 'pr-watch')).toBe('claude plugin update pr-watch@claude-code-pr-watch')
    // Another config dir has a cache of its own.
    expect(howToUpdate('/home/me/.claude-work/plugins/cache/acme-tools/pr-watch/0.8.0', '/home/me', 'pr-watch')).toBe(
      'claude plugin update pr-watch@acme-tools',
    )
  })

  test("in the cache where the marketplace can't be read off the path, the /plugin menu", async () => {
    expect(howToUpdate('/home/me/.claude/plugins/cache/', '/home/me', 'pr-watch')).toBe('/plugin → Installed → pr-watch → Update now')
  })

  test('a folder: git pull there, home shortened to ~', async () => {
    expect(howToUpdate('/home/me/.config/claude-mods/pr-watch', '/home/me', 'pr-watch')).toBe('git pull in ~/.config/claude-mods/pr-watch')
    expect(howToUpdate('/home/meg/pr-watch', '/home/me', 'pr-watch')).toBe('git pull in /home/meg/pr-watch')
    expect(howToUpdate('/opt/mods/pr-watch', null, 'pr-watch')).toBe('git pull in /opt/mods/pr-watch')
  })
})

describe('the notice', () => {
  test('one line where it fits, else what is out above how to get it', async () => {
    const how = 'git pull in ~/src/pr-watch'
    expect(noticeLines('0.9.0', how, 80)).toEqual(['↑ pr-watch 0.9.0 available · git pull in ~/src/pr-watch'])
    expect(noticeLines('0.9.0', how, 50)).toEqual(['↑ pr-watch 0.9.0 available', 'git pull in ~/src/pr-watch'])
    // The longest how there is still fits a 50-column pane on its own line.
    const update = howToUpdate('/home/me/.claude/plugins/cache/claude-code-pr-watch/pr-watch/0.8.0', '/home/me', 'pr-watch')
    expect(noticeLines('0.9.0', update, 50)[1]?.length).toBeLessThanOrEqual(50)
  })

  test('the toast says it on one line', async () => {
    expect(noticeToast('0.9.0', 'git pull in ~/src/pr-watch')).toBe('↑ pr-watch 0.9.0 available · git pull in ~/src/pr-watch')
  })
})
