import { describe, expect, test } from 'claude-code/testing'

import {
  cwdOf,
  derive,
  findPrUrls,
  mergeArgv,
  mergeError,
  mergeQuestion,
  mergeableOf,
  methodsOf,
  pickMethod,
  refFromGhCommand,
  shellCode,
  stepIndex,
  touched,
} from './pr'
import type { GhPr } from './pr'

const base: GhPr = {
  number: 7,
  title: 'Add thing',
  url: 'https://github.com/acme/app/pull/7',
  state: 'OPEN',
  isDraft: false,
  reviewDecision: null,
  mergeStateStatus: 'CLEAN',
  autoMergeRequest: null,
  statusCheckRollup: [],
  mergedAt: null,
  additions: 1,
  deletions: 0,
  headRefName: 'feat',
  baseRefName: 'main',
  author: { login: 'me' },
}

const run = (name: string, status: string, conclusion = '') => ({ __typename: 'CheckRun', name, status, conclusion })

describe('detecting PRs', () => {
  test('finds and dedupes PR urls in gh output', async () => {
    const refs = findPrUrls('created https://github.com/acme/app/pull/7\nsee https://github.com/acme/app/pull/7/files')
    expect(refs).toEqual([{ url: 'https://github.com/acme/app/pull/7', repo: 'acme/app', number: 7 }])
  })

  test('ignores the "create a pull request" link git push prints', async () => {
    expect(findPrUrls('remote: https://github.com/acme/app/pull/new/my-branch')).toEqual([])
  })

  test('builds a ref from a -R flag and number', async () => {
    expect(refFromGhCommand('gh pr checks 42 -R acme/app')?.url).toBe('https://github.com/acme/app/pull/42')
    expect(refFromGhCommand('gh pr checks 42')).toBe(null)
  })

  test('reads the cd prefix', async () => {
    expect(cwdOf('cd ~/src/app && gh pr checks')).toBe('~/src/app')
    expect(cwdOf('gh pr checks')).toBe(undefined)
  })
})

describe('lifecycle', () => {
  test('running CI is the CI step, warning', async () => {
    const d = derive({ ...base, statusCheckRollup: [run('build', 'IN_PROGRESS'), run('lint', 'COMPLETED', 'SUCCESS')] })
    expect(d.stage).toBe('checks')
    expect(d.headline).toBe('Running build · 1/2 passed')
    expect(d.isMoving).toBe(true)
    expect(stepIndex(d.stage)).toBe(1)
  })

  test('a failed check wins, but checks still running keep the card live', async () => {
    const d = derive({ ...base, statusCheckRollup: [run('build', 'COMPLETED', 'FAILURE'), run('e2e', 'QUEUED')] })
    expect(d.tone).toBe('error')
    expect(d.pill).toBe('✗ CI FAILING')
    expect(d.headline).toBe('Failing: build · 1 still running')
    expect(d.isMoving).toBe(true)
  })

  test('a failed check with nothing left running has settled', async () => {
    const d = derive({ ...base, statusCheckRollup: [run('build', 'COMPLETED', 'FAILURE'), run('lint', 'COMPLETED', 'SUCCESS')] })
    expect(d.pill).toBe('✗ CI FAILED')
    expect(d.headline).toBe('Failing: build')
    expect(d.isMoving).toBe(false)
  })

  test('names the running checks and counts the queued ones', async () => {
    const d = derive({
      ...base,
      statusCheckRollup: [
        run('build', 'IN_PROGRESS'),
        run('e2e', 'IN_PROGRESS'),
        run('lint', 'IN_PROGRESS'),
        run('deploy', 'QUEUED'),
        run('docs', 'WAITING'),
        run('types', 'COMPLETED', 'SUCCESS'),
      ],
    })
    expect(d.pill).toBe('● CI RUNNING')
    expect(d.headline).toBe('Running build, e2e +1 · 2 queued · 1/6 passed')
  })

  test('only queued checks say so', async () => {
    const d = derive({ ...base, statusCheckRollup: [run('build', 'QUEUED')] })
    expect(d.headline).toBe('1 queued · 0/1 passed')
  })

  test('conflicts outrank failing CI, which stays in the line', async () => {
    const d = derive({ ...base, mergeStateStatus: 'DIRTY', statusCheckRollup: [run('lint-docs', 'COMPLETED', 'FAILURE'), run('lint', 'COMPLETED', 'SUCCESS')] })
    expect(d.pill).toBe('⚠ CONFLICTS')
    expect(d.tone).toBe('error')
    expect(d.headline).toBe('Merge conflicts with base · failing: lint-docs')
    expect(d.stage).toBe('checks')
  })

  test('conflicts outrank running CI and pending review, and stay live while checks run', async () => {
    const d = derive({ ...base, mergeStateStatus: 'DIRTY', reviewDecision: 'REVIEW_REQUIRED', statusCheckRollup: [run('build', 'IN_PROGRESS')] })
    expect(d.pill).toBe('⚠ CONFLICTS')
    expect(d.headline).toBe('Merge conflicts with base · Running build')
    expect(d.isMoving).toBe(true)
  })

  test('mergeable CONFLICTING counts while mergeStateStatus is still being computed', async () => {
    const d = derive({ ...base, mergeStateStatus: 'UNKNOWN', mergeable: 'CONFLICTING', reviewDecision: 'REVIEW_REQUIRED' })
    expect(d.pill).toBe('⚠ CONFLICTS')
    expect(d.headline).toBe('Merge conflicts with base')
    expect(d.stage).toBe('ready')
  })

  test('a conflicted draft says so', async () => {
    expect(derive({ ...base, isDraft: true, mergeStateStatus: 'DIRTY' }).headline).toBe('Draft · merge conflicts')
  })

  test('a check re-run on the same commit counts by its latest run, as the PR page does', async () => {
    const at = (name: string, conclusion: string, startedAt: string) => ({ ...run(name, 'COMPLETED', conclusion), workflow: 'Docs Check', startedAt })
    const d = derive({
      ...base,
      statusCheckRollup: [
        at('lint-docs', 'FAILURE', '2026-10-07T13:55:50Z'),
        at('lint-docs', 'SUCCESS', '2026-10-07T14:07:48Z'),
        at('lint-docs', 'SUCCESS', '2026-10-07T14:00:32Z'),
        run('lint', 'COMPLETED', 'SUCCESS'),
      ],
    })
    expect(d.pill).not.toBe('✗ CI FAILED')
    expect(d.checks).toMatchObject({ passed: 2, failed: 0, total: 2 })
  })

  test('a queued re-run after a failure is what counts: CI is running again', async () => {
    const d = derive({
      ...base,
      statusCheckRollup: [
        { ...run('e2e', 'COMPLETED', 'FAILURE'), startedAt: '2026-10-07T13:00:00Z' },
        { ...run('e2e', 'QUEUED'), startedAt: null },
      ],
    })
    expect(d.pill).toBe('● CI RUNNING')
    expect(d.checks.total).toBe(1)
  })

  test("two workflows' same-named jobs are separate checks", async () => {
    const d = derive({
      ...base,
      statusCheckRollup: [
        { ...run('test', 'COMPLETED', 'SUCCESS'), workflow: 'Unit' },
        { ...run('test', 'COMPLETED', 'FAILURE'), workflow: 'E2E' },
      ],
    })
    expect(d.checks).toMatchObject({ passed: 1, failed: 1, total: 2 })
  })

  test('a status context counts by its latest report', async () => {
    const d = derive({
      ...base,
      statusCheckRollup: [
        { __typename: 'StatusContext', context: 'ci/jenkins', state: 'FAILURE', createdAt: '2026-10-07T10:00:00Z' },
        { __typename: 'StatusContext', context: 'ci/jenkins', state: 'SUCCESS', createdAt: '2026-10-07T11:00:00Z' },
      ],
    })
    expect(d.checks).toMatchObject({ passed: 1, failed: 0, total: 1 })
  })

  test('green CI waiting on approval', async () => {
    const d = derive({ ...base, reviewDecision: 'REVIEW_REQUIRED', statusCheckRollup: [run('build', 'COMPLETED', 'SUCCESS')] })
    expect(d.stage).toBe('review')
    expect(d.headline).toBe('Waiting for review')
  })

  test('approved and clean is ready', async () => {
    const d = derive({ ...base, reviewDecision: 'APPROVED' })
    expect(d.stage).toBe('ready')
    expect(d.tone).toBe('success')
  })

  test('status contexts count too', async () => {
    const d = derive({ ...base, statusCheckRollup: [{ __typename: 'StatusContext', context: 'ci/jenkins', state: 'PENDING' }] })
    expect(d.headline).toBe('Running ci/jenkins · 0/1 passed')
  })

  test('merged completes every step', async () => {
    const d = derive({ ...base, state: 'MERGED' })
    expect(d.headline).toBe('Merged')
    expect(stepIndex(d.stage)).toBe(4)
  })
})

describe('PRs Claude only read: offered', () => {
  const url = 'https://github.com/acme/app/pull/9'
  const read = (tool: string, input: Record<string, unknown>, output = '') => touched(tool, input, output)

  test('a PR link Claude fetched', async () => {
    expect(read('WebFetch', { url, prompt: 'status?' }).read.map(r => r.url)).toEqual([url])
  })

  test('a GitHub MCP read by owner/repo/number', async () => {
    const t = read('mcp__github__get_pull_request', { owner: 'acme', repo: 'app', pull_number: 9 })
    expect([t.read.map(r => r.url), t.acted]).toEqual([[url], []])
  })

  test('gh pr view / diff, gh api GETs', async () => {
    expect(read('Bash', { command: `gh pr view ${url}` }).read.map(r => r.url)).toEqual([url])
    expect(read('Bash', { command: 'gh pr diff 9 -R acme/app' }).read.map(r => r.url)).toEqual([url])
    expect(read('Bash', { command: 'gh api repos/acme/app/pulls/9' }).read.map(r => r.url)).toEqual([url])
    expect(read('Bash', { command: 'gh pr view --json title' }).resolve).toBe('read')
  })

  test('gh pr list resolves nothing, and its --limit is not a PR number', async () => {
    expect(read('Bash', { command: 'gh pr list --limit 30 -R acme/app' })).toEqual({ acted: [], read: [], resolve: null })
  })

  test('not PR links that merely sit in a file Claude read', async () => {
    expect(read('Read', { file_path: '/repo/CHANGELOG.md' }, `fixed in ${url}`).read).toEqual([])
    expect(read('Bash', { command: 'cat CHANGELOG.md' }, `fixed in ${url}`).read).toEqual([])
  })
})

describe('PRs Claude acted on: watched', () => {
  const url = 'https://github.com/acme/app/pull/9'
  const acted = (input: Record<string, unknown>, output = '', tool = 'Bash') => touched(tool, input, output)

  test('gh pr create behind an env prefix and a heredoc PR body (as Claude really runs it)', async () => {
    const command =
      "S=/tmp/s && cat > $S/body.md <<'EOF'\nFollows https://github.com/acme/app/pull/4; run gh pr view later\nEOF\n" +
      'GH_TOKEN=$(gh auth token --user me) gh pr create --repo acme/app --title "feat: x" --body-file $S/body.md'
    expect(acted({ command }, `${url}\n`)).toEqual({ acted: [{ url, repo: 'acme/app', number: 9 }], read: [], resolve: null })
  })

  test('git push with -c options, then gh pr create', async () => {
    const command =
      'T=$(gh auth token) && git -c credential.helper= -c credential.helper="!f() { echo password=$T; }; f" push -q -u origin feat/x 2>&1 | tail -2 && GH_TOKEN=$T gh pr create --fill'
    const out = `remote:      https://github.com/acme/app/pull/new/feat/x\n${url}`
    expect(acted({ command }, out).acted.map(r => r.url)).toEqual([url])
  })

  test('a plain git push asks gh which PR the branch has', async () => {
    expect(acted({ command: 'cd ~/src/app && git push' })).toEqual({ acted: [], read: [], resolve: 'acted' })
  })

  test('merge, comment, review, checks', async () => {
    expect(acted({ command: `gh pr merge ${url} --squash` }).acted.map(r => r.url)).toEqual([url])
    expect(acted({ command: 'gh pr comment 9 -R acme/app -b "LGTM, see gh pr create docs"' }).acted.map(r => r.url)).toEqual([url])
    expect(acted({ command: 'gh pr checks --watch' }).resolve).toBe('acted')
    expect(acted({ command: 'gh api -X PATCH repos/acme/app/pulls/9 -f title=x' }).acted.map(r => r.url)).toEqual([url])
  })

  test('GitHub MCP writes; a new PR is the first link in the reply', async () => {
    const reply = `{"html_url":"${url}","body":"follows https://github.com/acme/app/pull/4"}`
    expect(acted({ owner: 'acme', repo: 'app', title: 'x' }, reply, 'mcp__github__create_pull_request').acted.map(r => r.url)).toEqual([url])
    expect(acted({ owner: 'acme', repo: 'app', pullNumber: 9 }, '', 'mcp__github__merge_pull_request').acted.map(r => r.url)).toEqual([url])
  })

  test('not a gh pr create, merge or git push that is only text in a heredoc or a message', async () => {
    for (const command of ["git commit -m 'then gh pr create and git push'", "cat <<'EOF'\ngh pr merge 9\ngit push\nEOF"]) {
      expect(acted({ command }, url)).toEqual({ acted: [], read: [], resolve: null })
    }
  })

  test('shellCode keeps the code around a heredoc', async () => {
    expect(shellCode("cat > f <<'EOF' && echo hi\nbody\nEOF\ngh pr create")).toBe("cat > f   && echo hi\ngh pr create")
  })
})

describe('merging', () => {
  const repo = { squashMergeAllowed: true, rebaseMergeAllowed: true, mergeCommitAllowed: true, autoMergeAllowed: true, viewerPermission: 'WRITE' }
  const open: GhPr = { ...base, repository: { ...repo, viewerDefaultMergeMethod: 'SQUASH' }, viewerCanEnableAutoMerge: true }

  test("the viewer's default method comes first while the repo allows it", async () => {
    expect(methodsOf({ ...repo, viewerDefaultMergeMethod: 'REBASE' })).toEqual(['rebase', 'squash', 'merge'])
  })

  test('a default the repo no longer allows falls back to squash, rebase, merge', async () => {
    expect(methodsOf({ ...repo, mergeCommitAllowed: false, viewerDefaultMergeMethod: 'MERGE' })).toEqual(['squash', 'rebase'])
  })

  test('a rebase-only repo offers rebase', async () => {
    const only = { rebaseMergeAllowed: true, squashMergeAllowed: false, mergeCommitAllowed: false, viewerDefaultMergeMethod: 'MERGE' }
    expect(methodsOf(only)).toEqual(['rebase'])
    expect(pickMethod(methodsOf(only), 'squash')).toBe('rebase')
  })

  test('clean, unstable or with hooks: merge now, if the viewer may write', async () => {
    for (const status of ['CLEAN', 'HAS_HOOKS', 'UNSTABLE']) expect(mergeableOf({ ...open, mergeStateStatus: status }).canMerge).toBe(true)
    expect(mergeableOf({ ...open, repository: { ...repo, viewerPermission: 'READ' } }).canMerge).toBe(false)
    expect(mergeableOf({ ...open, mergeStateStatus: 'BLOCKED' }).canMerge).toBe(false)
  })

  test('blocked or waiting on checks: auto-merge, if the repo allows it', async () => {
    const blocked: GhPr = { ...open, mergeStateStatus: 'BLOCKED' }
    expect(mergeableOf(blocked).canAuto).toBe(true)
    expect(mergeableOf({ ...open, mergeStateStatus: 'UNKNOWN', statusCheckRollup: [run('build', 'IN_PROGRESS')] }).canAuto).toBe(true)
    expect(mergeableOf({ ...blocked, repository: { ...repo, autoMergeAllowed: false } }).canAuto).toBe(false)
    expect(mergeableOf({ ...blocked, viewerCanEnableAutoMerge: false }).canAuto).toBe(false)
    expect(mergeableOf({ ...open, mergeStateStatus: 'BEHIND' }).canAuto).toBe(false)
  })

  test('drafts, conflicts and finished PRs offer nothing', async () => {
    for (const pr of [
      { ...open, isDraft: true },
      { ...open, mergeStateStatus: 'DIRTY' },
      { ...open, mergeStateStatus: 'BLOCKED', mergeable: 'CONFLICTING' },
      { ...open, state: 'MERGED' as const },
    ]) {
      const m = mergeableOf(pr)
      expect([m.canMerge, m.canAuto]).toEqual([false, false])
    }
  })

  test('auto-merge on: its method, and whether the viewer may turn it off', async () => {
    const m = mergeableOf({ ...open, autoMergeRequest: { mergeMethod: 'REBASE' }, viewerCanDisableAutoMerge: true })
    expect([m.isAuto, m.autoMethod, m.canCancelAuto, m.canMerge]).toEqual([true, 'rebase', true, false])
  })

  test('gh pr merge argv', async () => {
    const url = 'https://github.com/acme/app/pull/7'
    expect(mergeArgv(url, { action: 'merge', method: 'rebase' })).toEqual(['pr', 'merge', url, '--rebase'])
    expect(mergeArgv(url, { action: 'auto', method: 'squash' })).toEqual(['pr', 'merge', url, '--auto', '--squash'])
    expect(mergeArgv(url, { action: 'cancel-auto' })).toEqual(['pr', 'merge', url, '--disable-auto'])
  })

  test('the question names the method, the PR and its base', async () => {
    expect(mergeQuestion('squash', { number: 12, base: 'main' }, false)).toBe('Squash-merge #12 into main?')
    expect(mergeQuestion('merge', { number: 12, base: 'main' }, true)).toBe('Merge #12 into main once checks and review pass?')
  })

  test("gh's refusal in plain words, and whether it refused the method", async () => {
    const method = mergeError('GraphQL: Merge method squash merging is not allowed on this repository (mergePullRequest)\n')
    expect(method).toEqual({ message: 'Merge method squash merging is not allowed on this repository', isMethodRefused: true })
    const state = mergeError(
      'X Pull request acme/app#7 is not mergeable: the base branch policy prohibits the merge.\n' +
        'To have the pull request merged after all the requirements have been met, add the `--auto` flag.\n',
    )
    expect(state).toEqual({ message: 'Pull request acme/app#7 is not mergeable: the base branch policy prohibits the merge.', isMethodRefused: false })
  })
})
