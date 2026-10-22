# The shared files

Every pr-watch session on a machine shares one cache directory. Whoever holds the lease polls GitHub for every live session's PRs, and the rest read the results. This file describes that cache as pr-watch 0.14.1 writes and reads it, for any other program that takes part, such as the planned `prw` companion.

In code: the mod's side lives in `hooks/register.tsx` (the "One poller for the whole machine" section), `hooks/protocol.ts` (session files, temp paths), `hooks/cache.ts` (entries) and `hooks/rate.ts` (rates). Symbols named below are from those files.

## The directory

`$HOME/.cache/pr-watch/` (`root()`; `/tmp/.cache/pr-watch/` where `HOME` is unset).

| Path | What it is | Written by | How often |
|---|---|---|---|
| `sessions/<id>.json` | One session's watch list; its mtime is its heartbeat | That session | Every tick (10 s) |
| `lease.json` | Whose turn it is to poll | The session taking or giving back the turn | Around each round |
| `results.json` | What GitHub said about every watched PR | The lease holder; any session fetching a PR at once | Each round, each watch / Refresh / merge / comment |
| `results.json.<owner>.tmp` | A `results.json` on its way in | Its owner | Just before each `results.json` write |
| `notice.json`, `errors.log` | The mod's own: the update toast it showed, failed calls | The mod | Not part of the protocol: don't write them |
| `companion/watches/<hash>.json` | One PR one agent's session watches through `prw watch` or `prw mcp` | `prw` | When watched; deleted when unwatched |
| `prw-errors.log` | `prw`'s own failed calls, in the mod's `errors.log` format (`logLine`, last `LOG_LINES`) | `prw` | Not part of the protocol |

All times are milliseconds since the epoch (`$.clock.now()`), unless a field says ISO.

## Compatibility rules

- **Unknown fields are ignored.** Every reader takes the fields it knows and skips the rest.
- **New fields are optional.** A file written by an older mod lacks them, and a reader treats a missing field as absent, never as an error. There is no version field.
- **`results.json` is read, merged and written, never replaced blindly.** A writer re-reads it just before writing, keeps the top-level fields it doesn't know, and merges its PRs into `prs` (`publish`).
- **A file that doesn't parse is absent, not fatal.** The rules for each file are below.

## `sessions/<id>.json`

```ts
type SessionFile = {
  urls: string[]          // PR URLs this session watches: https://github.com/<owner>/<repo>/pull/<n>
  writer?: 'mod' | 'prw'  // who wrote it; absent: an older mod (before 0.14.1)
  agent?: string          // the agent the session runs: 'claude' for the mod; 'codex', … through prw
  name?: string           // what a list of sessions calls it: the project root's folder name
  cwd?: string            // the directory the session runs in, absolute
  kind?: 'searches'       // what the file is where it isn't a session's; absent: a session
}
```

```json
{"urls":["https://github.com/acme/app/pull/12"],"writer":"mod","agent":"claude","name":"app","cwd":"/work/app/hooks"}
```

- **`<id>`:** the writer's own id. The mod uses `INSTANCE`, `<base-36 time>-<up to 8 random base-36 characters>`, which lasts as long as the loaded module (a `/clear` keeps it). `prw` uses `prw-<id>`, new each run. Only names ending in `.json` are session files.
- **`prw`'s own file:** `{"urls": [...], "writer": "prw", "name": "My PRs"}`, with no `agent`: every open PR each gh login authored (one search per login, as that login, a minute apart), so whoever polls fetches them like any session's. It is written every 10 s while `prw` runs, and, unlike the mod's, removed when `prw` quits (giving back a lease it still holds).
- **`prw`'s saved searches:** `sessions/prw-<id>-searches.json`, `{"urls": [...], "writer": "prw", "name": "Saved searches", "kind": "searches"}`: what the searches in `prw`'s `config.toml` found (at most 30 each, searched with the "My PRs" search, in the same call), so whoever polls fetches them too. Written and removed with `prw`'s own file; absent while they find nothing.
- **`kind`:** a file with any `kind` (one a reader doesn't know too) is polled like a session's, but is no session: it lists nothing among the sessions, and doesn't count as one live. The mod polls every file's URLs and reads no `kind`.
- **Other agents' sessions:** `sessions/prw-w-<hash>.json`, one per agent's session (`hash`: FNV-1a 64 of the agent, the session name and, where the folder named it, the folder: two projects in folders of one name are two sessions), `{"urls": [...], "writer": "prw", "agent": "codex", "name": "billing", "cwd": "..."}`: the watches in `companion/watches/` of that agent and session. Any `prw` TUI writes every one each tick, and a `prw mcp` writes its own; they write the same text, through a temp file in `companion/` renamed into `sessions/`. One whose last watch goes is removed.
- **When it is written:** the mod writes its file on each 10 s tick (`$.clock.every(10_000)` → `tick` → `announce`), and only while it watches an open PR or one not read yet (`isActive`). A session that watches nothing writes nothing and soon stops counting as live.
- **`name` and `cwd`:** these come from the session start (`session.start`'s `cwd`, and `$.session.root()` for `name`). They follow `classic.CwdChanged`, so a `cd`, `/cd` or worktree move shows on the next tick. `name` is the project root's last path segment (`placeOf`, `folderName`), so a `cd` into a subfolder moves `cwd` but doesn't rename the session. Until the start has said where the session runs, the file has neither field.
- **Liveness:** a file is live while its mtime is less than `ALIVE_MS` (60 000 ms) old. A dead session's PRs are no longer polled. The mod's files are never deleted on exit. Ones older than `PRUNE_MS` (24 h) are removed by whichever session runs a round, at most every `PRUNE_EVERY_MS` (1 h).
- **Atomicity: none.** The file is written in place (`writeJson`) every tick, since a rename would cost an exec every tick. **A reader may catch it torn or empty.** Treat a file that doesn't parse as listing no URLs this time (`urlsIn`) and read it again next tick, or keep the last good parse of that file. Take only strings from an array `urls`, whatever else is there.

## `lease.json`

```ts
type Lease = { owner: string; until: number }   // owner: a session file's <id>
```

A lease, not a lock: plain files have no atomic create.

1. **Due:** a round is due once the clock reaches `results.json`'s `nextAt`. Before that, nobody polls.
2. **Take** (`takeLease`): read `lease.json`. If another owner holds it and `until` is in the future, it isn't your turn. Otherwise write `{owner: <you>, until: now + LEASE_MS}` (`LEASE_MS` = 30 000 ms), sleep 150 ms, then read it again. It is your turn only if `owner` is still you. Two takers racing past this cost one duplicate round, never a missed one.
3. **Poll inside it:** every call of the round must end before `until − 1 s` (`publish`'s `until`). A call starts only with at least `MIN_CALL_MS` (5 s) left (`callBudget`). Whatever has no time left waits for the next round.
4. **Give back** (`releaseLease`): write `{owner: <you>, until: 0}` when the round ends, even if it failed.

**Atomicity: none.** The file is written in place. A torn or empty `lease.json` reads as no lease held. At worst, two sessions both take the turn, and the re-read settles it or costs one duplicate round.

## `results.json`

```ts
type Shared = {                       // register.tsx
  fetchedAt: number                   // when the last round ran
  nextAt: number                      // when the next round is due
  prs: Record<string, Entry>          // by PR URL
  accounts: Record<string, string>    // repo owner → the gh login that sees its repos (a login, never a token)
  rate: RateLimit | null              // legacy: what versions before per-login rates read (legacyRate)
  rates: Record<string, LoginRates>   // by gh login (rate.ts)
  latest: { version: string | null; checkedAt: number }  // pr-watch's newest version, when last asked
  heldUntil?: number                  // a secondary rate limit holds everyone off GitHub until then
}

type Entry = {                        // cache.ts
  at: number                          // when `pr` was fetched; with no `pr`, when the fetch last failed
  pr?: GhPr                           // the PR as the batched GraphQL query returned it (below)
  error?: string                      // what the card says, once it says anything
  failedSince?: number                // first failure in a row while `pr` still stands
  isUnreachable?: true                // never read: no logged-in gh account could see it
}

type LoginRates = { graphql?: Bucket; core?: Bucket; askedAt?: number }   // askedAt: last /rate_limit ask
type Bucket = { limit: number; remaining: number; resetAt: number; seenAt: number }
type RateLimit = { remaining: number; resetAt: string /* ISO */; limit?: number }
```

- **`pr` is raw:** the `GhPr` type (`hooks/pr.ts`) as `parseReply` (`hooks/batch.ts`) stores it. Checks are flattened into `statusCheckRollup` with each run's `workflow`, the repository's merge settings sit under `repository`, and the review connections come along. There is no derived card in the file. Each reader works out the status itself, and `companion/fixtures/` pins what it must work out.
- **Who writes it:** the lease holder at the end of a round (`publish` with `live`), and any session fetching PRs at once outside the lease (`fetchNow`: a new watch, Refresh, after a merge or a comment). Either way, the writer reads the file, asks GitHub, reads it again and merges. Each fetched PR replaces its entry, each failure goes through `failedEntry`, the rates fold in by the later sighting (`withSeen`), and `accounts` merges.
- **The hold:** while `heldUntil` is in the future, nobody asks GitHub anything, rounds and at-once fetches alike. It is set from the `retry-after` of a secondary rate limit, or 60 s (`LIMIT_WAIT_MS`) where none is named, and only ever moved later.
- **A round also:** drops entries no live session watches once they are `KEEP_MS` (1 h) old (`keptEntries`), and sets `fetchedAt`. It sets `nextAt` to `fetchedAt` plus the active interval (`poll_active_seconds`, default 20 s) while any PR moves (`derive(pr).isMoving`), else the idle one (`poll_idle_seconds`, default 60 s). `nextAt` is pushed later to a rate reset when every login the round needs is low (`roundWaitsUntil`), and never earlier than `heldUntil`.
- **What a round asks:** every live session's URLs plus its own (`liveUrls`), minus finished PRs (`state` not `OPEN`) and PRs no account could see in the last `UNREACHABLE_MS` (5 min, `isUnreachable`). The least recently asked go first.
- **Atomicity: whole-file.** Since 0.14.1 the mod writes `results.json.<INSTANCE>.tmp` in the same directory, then `mv -f` renames it over `results.json` (`writeResults`, `tempPath`). That is one exec per write, and a reader sees the old file or the new one, never a mix. If the rename fails, the mod writes in place as before, so sharing never stops. Older mods always write in place. **So a reader must still keep its last good copy when a read doesn't parse,** and show its age.
- **Temp files:** each is named by its writer (`<INSTANCE>`, or `prw-<id>`), so writers never share one, and a writer's next write takes over one it left behind. Ones older than `PRUNE_MS` (24 h) are pruned with the session files. No reader opens `*.tmp` (`isTemp`), and they sit beside `results.json`, never in `sessions/`.

## Who polls (the give-way rule)

The mod polls whenever a round is due and it gets the lease. It has no other rule.

A poller that is not the mod (`prw`) should poll only when `should_poll(sessions, lease, results, now)` holds, and must still take the lease as above:

- No live session file says `writer: "mod"`, counting a file with no `writer` as an older mod's (live: mtime less than `ALIVE_MS` old). **Or:**
- A mod is live, but the turn has been free for more than two round intervals. That is: no lease held (`until` ≤ now), and `now − nextAt` > 120 000 ms (twice the default idle interval). This covers a mod too old to poll for others, or one stuck.

Otherwise it reads `results.json` like any other session, and announces its own PRs in its session file so the mod's round fetches them too. The rule has no constant in the mod's code: the 120 000 ms belongs to the companion.

## Reader rules, in short

| File | Torn or unparsable | Missing |
|---|---|---|
| `sessions/<id>.json` | No URLs this time (or the last good parse); liveness still by mtime. One that has never parsed may be left out until it does (prw does: its writer and name aren't known yet) | Not a session |
| `lease.json` | No lease held | No lease held |
| `results.json` | Keep the last good copy | Nothing fetched yet (`empty()`: every field zero or empty) |
