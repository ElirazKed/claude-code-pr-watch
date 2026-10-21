// The files under ~/.cache/pr-watch/ that every session shares, and that other readers (the
// `prw` companion) share too: what a session file says, and how a reader takes what it finds.
// companion/PROTOCOL.md is the spec; this is the mod's side of it.

// Who wrote a session file: this mod, or the companion. A file with no `writer` is an older
// mod's, which wrote `{ urls }` alone.
export type Writer = 'mod' | 'prw'

// sessions/<id>.json. Every field but `urls` is optional: older mods write none of them, and a
// reader takes a file without them as it always did.
export type SessionFile = {
  urls: string[]
  writer?: Writer
  // The agent the session runs (`claude` here; `codex` and others through the companion).
  agent?: string
  // What a list of sessions calls this one: its project folder's name.
  name?: string
  // The directory the session runs in, absolute.
  cwd?: string
}

// Where the session runs, as its file says: `cwd`, and `name` from its project root (a `cd`
// into a subfolder doesn't rename the session).
export type Place = { cwd: string; name: string }

// The last segment of a path: `/work/app/` is `app`; the root is itself.
export function folderName(path: string): string {
  const parts = path.split(/[\\/]+/).filter(Boolean)

  return parts.at(-1) ?? path
}

export function placeOf(cwd: string, root: string = cwd): Place {
  return { cwd, name: folderName(root) }
}

// This session's file: its PRs, and, once the session start said where it runs, where.
export function sessionFile(urls: readonly string[], place: Place | null): SessionFile {
  return { urls: [...urls], writer: 'mod', agent: 'claude', ...(place !== null && { name: place.name, cwd: place.cwd }) }
}

// The PR urls a session file lists. Whatever another writer put there, a reader takes only
// strings from an array; anything else (a file cut off mid-write, which reads as null) lists
// none, and that session's PRs come back with its next write.
export function urlsIn(file: unknown): string[] {
  const urls = typeof file === 'object' && file !== null ? (file as { urls?: unknown }).urls : undefined

  return Array.isArray(urls) ? urls.filter((url): url is string => typeof url === 'string') : []
}

// Where a whole-file write goes first, beside the file it replaces (the same filesystem, so
// the move is a rename): named by its writer, so two writers never share one, and a writer's
// next write takes over one it left behind.
export const tempPath = (path: string, owner: string) => `${path}.${owner}.tmp`

// A temp file `tempPath` names, by its file name: never a session file or results.json.
export const isTemp = (name: string) => name.endsWith('.tmp')
