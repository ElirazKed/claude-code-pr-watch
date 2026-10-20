// Which gh login asks about a PR. Every call made as a login carries that login's own token,
// the active one's too: gh's active account is whichever was switched to last, in any
// terminal, so it says nothing about who a call runs as.

// gh's logins on github.com: the active one, and the rest.
export type Logins = { active: string; others: string[] }

export const NO_LOGINS: Logins = { active: '', others: [] }

// The logins `gh auth status` lists, or null where it lists none (gh timed out, or nobody is
// logged in): a reading worth nothing, so never kept. Where none is marked active, the first
// stands for it.
export function loginsOf(text: string): Logins | null {
  const all = [...text.matchAll(/account (\S+)[^\n]*\n\s*-\s*Active account: (true|false)/g)].map(m => ({ login: m[1] ?? '', isActive: m[2] === 'true' }))
  const named = all.filter(m => m.login !== '')
  if (named.length === 0) return null
  const active = (named.find(m => m.isActive) ?? named[0])?.login ?? ''

  return { active, others: [...new Set(named.map(m => m.login).filter(login => login !== active))] }
}

// Every login, the active one first.
export function everyLogin(logins: Logins): string[] {
  return [...new Set([logins.active, ...logins.others])].filter(login => login !== '')
}

// The login an owner's PRs are polled as: the one found to see them, while it is still logged
// in (or nothing says it isn't), else the active one.
export function loginFor(accounts: Readonly<Record<string, string>>, owner: string, logins: Logins): string {
  const mapped = accounts[owner]
  const all = everyLogin(logins)
  if (mapped !== undefined && (all.length === 0 || all.includes(mapped))) return mapped

  return logins.active
}

// A login's token as gh gave it, or what the card says where it wouldn't.
export type Token = { token: string } | { error: string }

export const tokenError = (login: string) => (login === '' ? "Couldn't read gh's logins (gh auth status)" : `Couldn't get ${login}'s gh token`)

// How long a reading of gh's logins, or a login's token, is kept: a login added, switched to
// or logged in again shows within this. A reading that failed is asked again AGAIN_MS on, so a
// keyring locked after sleep isn't asked on every call.
export const KEEP_MS = 300_000
export const AGAIN_MS = 5_000

// Whether to ask gh again: never asked, a failure AGAIN_MS old, or a good answer KEEP_MS old.
export function isDue(last: { at: number; isOk: boolean } | undefined, at: number): boolean {
  if (last === undefined) return true

  return at - last.at >= (last.isOk ? KEEP_MS : AGAIN_MS)
}

// Which PRs a login is still to be asked about: the missing ones it hasn't answered for.
export function untriedBy<T extends { url: string }>(missing: readonly T[], asked: ReadonlyMap<string, readonly string[]>, login: string): T[] {
  return missing.filter(ref => !(asked.get(ref.url) ?? []).includes(login))
}

// What a PR no login could see says: the logins that answered. "Not found as octocat or hubot".
export function notFoundAs(asked: readonly string[]): string {
  const names = [...new Set(asked)].filter(login => login !== '')
  if (names.length === 0) return 'Not found'
  if (names.length === 1) return `Not found as ${names[0]}`

  return `Not found as ${names.slice(0, -1).join(', ')} or ${names[names.length - 1]}`
}
