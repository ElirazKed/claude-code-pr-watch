import { describe, expect, test } from 'claude-code/testing'

import { folderName, isTemp, placeOf, sessionFile, tempPath, urlsIn } from './protocol'

const URL = 'https://github.com/acme/app/pull/7'

describe('session files', () => {
  test('a session says who wrote it, which agent, and where it runs', async () => {
    expect(sessionFile([URL], placeOf('/work/app/hooks', '/work/app'))).toEqual({
      urls: [URL],
      writer: 'mod',
      agent: 'claude',
      name: 'app',
      cwd: '/work/app/hooks',
    })
  })

  test('before the session start says where, the file has no place', async () => {
    expect(sessionFile([URL], null)).toEqual({ urls: [URL], writer: 'mod', agent: 'claude' })
  })

  test("a folder's name is its path's last segment", async () => {
    expect(folderName('/work/app')).toBe('app')
    expect(folderName('/work/app/')).toBe('app')
    expect(folderName('C:\\work\\app')).toBe('app')
    expect(folderName('/')).toBe('/')
    expect(placeOf('/work/app')).toEqual({ cwd: '/work/app', name: 'app' })
  })

  test("an older mod's file, a newer one's and the companion's all list their urls", async () => {
    expect(urlsIn({ urls: [URL] })).toEqual([URL])
    expect(urlsIn({ urls: [URL], writer: 'mod', agent: 'claude', name: 'app', cwd: '/work/app' })).toEqual([URL])
    expect(urlsIn({ urls: [URL], writer: 'prw', agent: 'codex', future: { field: 1 } })).toEqual([URL])
  })

  test('a file cut off mid-write, or one shaped otherwise, lists none', async () => {
    for (const file of [null, undefined, 'urls', [URL], {}, { urls: URL }, { urls: null }]) expect(urlsIn(file)).toEqual([])
    expect(urlsIn({ urls: [URL, 7, null, { url: URL }] })).toEqual([URL])
  })
})

describe('whole-file writes', () => {
  test('the temp file sits beside the file, named by its writer', async () => {
    expect(tempPath('/home/me/.cache/pr-watch/results.json', 'lz3-ab12')).toBe('/home/me/.cache/pr-watch/results.json.lz3-ab12.tmp')
  })

  test('a temp file is never read as a session file or the results', async () => {
    expect(isTemp('results.json.lz3-ab12.tmp')).toBe(true)
    expect(isTemp('results.json')).toBe(false)
    expect(isTemp('lz3-ab12.json')).toBe(false)
  })
})
