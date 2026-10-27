import { describe, expect, test } from 'claude-code/testing'

import { commentArgv, commentError, commentPlaceholder, commentUrl, isPostable } from './comment'

const PR = { repo: 'acme/app', number: 21 }

describe('what posts', () => {
  test('text with something in it; an empty or blank field does not', async () => {
    expect(isPostable('LGTM')).toBe(true)
    expect(isPostable('  /retest\n')).toBe(true)
    expect(isPostable('')).toBe(false)
    expect(isPostable(' \n\t ')).toBe(false)
  })
})

describe('the call', () => {
  test("a POST to the PR's conversation, the text as gh's raw field", async () => {
    expect(commentArgv(PR, 'LGTM')).toEqual(['api', '--method', 'POST', 'repos/acme/app/issues/21/comments', '-f', 'body=LGTM'])
  })

  test('the body is the text whole, in one argument after body=: an `@` or `-` first, newlines and quotes kept', async () => {
    for (const text of ['@/etc/passwd', '--method=DELETE', '-F body=x', '@octocat take a look', 'first line\nsecond line\n', 'say "hi" \\ bye', ' LGTM ']) {
      const argv = commentArgv(PR, text)
      expect(argv.slice(-2)).toEqual(['-f', `body=${text}`])
      expect(argv.slice(0, -1)).toEqual(commentArgv(PR, '').slice(0, -1))
    }
  })
})

describe("GitHub's reply", () => {
  test("the new comment's link", async () => {
    const reply = JSON.stringify({ id: 1, html_url: 'https://github.com/acme/app/pull/21#issuecomment-1', body: 'LGTM' })
    expect(commentUrl(reply)).toBe('https://github.com/acme/app/pull/21#issuecomment-1')
  })

  test('no link in a reply that names none, or will not parse', async () => {
    expect(commentUrl('{}')).toBe(null)
    expect(commentUrl('null')).toBe(null)
    expect(commentUrl('{"html_url": 7}')).toBe(null)
    expect(commentUrl('<html>Bad Gateway</html>')).toBe(null)
    expect(commentUrl('')).toBe(null)
  })
})

describe('a refusal on the card', () => {
  test("GitHub's own words, without gh's prefix and the status", async () => {
    const stderr = 'gh: Unable to create comment because issue is locked. (HTTP 403)\n'
    expect(commentError(stderr, 'me')).toBe("Couldn't comment: Unable to create comment because issue is locked.")
  })

  test('the first line only', async () => {
    expect(commentError('gh: Validation Failed (HTTP 422)\n{"message":"Validation Failed"}\n', 'me')).toBe("Couldn't comment: Validation Failed")
  })

  test('not found names the account, which may be the one that cannot see it', async () => {
    expect(commentError('gh: Not Found (HTTP 404)\n', 'hubot')).toBe("Couldn't comment: GitHub has no such PR, or hubot can't see it")
  })

  test('a token GitHub refuses says how to renew it', async () => {
    expect(commentError('gh: Bad credentials (HTTP 401)\n', 'me')).toBe("Couldn't comment: GitHub refused me's token; gh auth login renews it")
  })

  test('a cut connection, or nothing said', async () => {
    expect(commentError('error connecting to api.github.com\n', 'me')).toBe("Couldn't comment: error connecting to api.github.com")
    expect(commentError('', 'me')).toBe("Couldn't comment: gh api failed")
  })
})

describe('the empty field', () => {
  test('says which PR, and which account will post', async () => {
    expect(commentPlaceholder(PR, 'hubot')).toBe('Comment on #21 as hubot')
    expect(commentPlaceholder(PR, '')).toBe('Comment on #21')
  })
})
