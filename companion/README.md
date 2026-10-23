# prw

`prw` is an optional terminal companion for the pr-watch mod. It watches every PR your agents touch
(Claude through the mod; Codex and others through `prw mcp` or `prw watch`) beside your saved GitHub
searches, in a responsive TUI with a full PR view (conversation, threads, checks, files), so GitHub can
stay closed. The mod stays complete on its own; `prw` is for when you want the big picture, with or
without Claude.

**Status: step 8 of the plan: saved searches.** `prw` shows the PRs your sessions watch, from the mod's shared
cache (`PROTOCOL.md`): a list of all of them, one list per live session (Claude through the mod;
others by the `agent` their session file names), each PR's card worked out as the mod does
(`prw-core`, held to the mod's own fixtures in `fixtures/`). It also finds every open PR each of your
gh logins authored and shows them as "My PRs", and a list for each GitHub search you save (below), so
it is of use without Claude running at all.

It takes part in the poll as one more session. It announces "My PRs" in its own session file, so a
running mod fetches them in its rounds. When no mod is polling, it polls GitHub itself, for every live
session, under the same lease, rate limits and hold as the mod. The header says who polled last.
`--no-poll` keeps it to reading: it never writes to the cache, and asks GitHub only when you act on a
PR.

On the selected PR, `m` merges, `c` comments, `f` hands it off and `o` opens it in the browser
(below). Each goes as the gh login that sees the PR's repo (the one the poll uses), with that login's
own token, whichever login `gh` has active. After a merge or a comment the PR is fetched again at
once, so its card is current before the next round.

- **Which login:** with more than one gh login, the PR's details say which it goes as ("as hubot"),
  and `a` moves its owner to the next one: written to `results.json`'s `accounts`, so the mod goes as
  it too, and the PR is fetched again at once as that login. A fetch then keeps it, unless that login
  can't see one of the owner's PRs (the poll asks the others, as ever).

- **Merge** asks first on the bottom line: "Squash-merge #409 into main?", `y` to merge, `m` for the
  repo's next allowed method. Where GitHub won't merge yet, it offers auto-merge instead, and cancels
  one already on. A method the repo refuses is skipped next time; a server error is retried once, after
  checking whether the merge went through.
- **Comment** opens a field on the bottom line: Enter posts, Alt-Enter is a new line, a paste goes in
  whole, Esc cancels. A post that fails keeps the text for the next `c`. The text goes to `gh` on
  stdin, never in its arguments.
- **Hand-offs** copy a prompt for the agent that runs the PR, as the mod drafts it: Resolve (conflicts
  with the base), Fix (failed checks, with the tail of each failed job's log) or Address (the open
  review threads, each comment cleaned of markup). With more than one, `f` asks which. The toast says
  how it was copied: `pbcopy` on macOS, `wl-copy`, `xclip` or `xsel` on Linux, OSC 52 over SSH or where
  none works. OSC 52 can't confirm the copy, so `v` shows the prompt last copied, to select by hand.

Agents other than Claude watch PRs through `prw mcp` (a tool server with the mod's `watch` tool) or
`prw watch <url>`. Each agent's session gets a list of its own, as a Claude session does (below).

## Build and run

With nix (flakes), from the repository root:

```sh
nix run .#prw                              # build and run, reading ~/.cache/pr-watch
nix run .#prw -- --cache companion/demo --demo   # the demo cache (below)
nix build .#prw                            # ./result/bin/prw
nix develop                                # cargo, rustc, clippy, rustfmt, rust-analyzer
cd companion && cargo run -p prw
```

Without nix, any recent stable Rust (1.88 or newer):

```sh
cd companion
cargo run -p prw -- --cache demo --demo
cargo test --workspace
```

`--cache <dir>` (or `PRW_CACHE`) uses another cache directory than `~/.cache/pr-watch`. `prw` runs
`gh` from `PATH`, as each login (`gh auth token --user`), and never switches gh's active account.

### Saved searches

Each search in `~/.config/prw/config.toml` is a list of its own, beside All and My PRs:

```toml
[[search]]
name = "Review requests"
query = "is:open review-requested:@me"

[[search]]
name = "Team web"
query = "is:open repo:acme/web"
login = "octocat"   # only as this gh login; else as each, their finds together
```

The query is what you would type in GitHub's search box; `is:pr` is added where it doesn't say. Each
list shows at most 30 PRs, and its title says how many match in all ("4 of 112"). With no
`config.toml`, there is one list, "Review requests" as above; an empty file has none. The file is read
again when it changes, and a file that won't read, or a search with no name or query, is said on the
status line. At most 10 searches run.

Searches run with the "My PRs" search, every minute, in one call per gh login (a point a search). A
login low on points waits, keeping what it found. What they find is announced in a file of its own
(`sessions/prw-<id>-searches.json`, `"kind": "searches"`), so the poll fetches those PRs as it does
any session's. They stay out of All and the header's counts, which are your sessions' PRs.
`--config <file>` (or `PRW_CONFIG`) reads another file; `$XDG_CONFIG_HOME/prw/config.toml` is used
where that is set. `--no-poll` and `--demo` run no search (the demo shows `demo/searches.json`).

### The demo cache

`demo/` is a hand-made cache: a `results.json` of acme/* PRs built from the status fixtures (CI
running, failed, conflicts, changes requested, comments to address, in review, ready, merged; one PR
no gh account could see, one whose fetches have failed for a while) and four session files: two
Claude sessions (`app-retry`, `web-checkout`), a Codex session written by `prw` (`billing`), and an
older mod's file with only `urls` (listed as "Claude session"), which also watches a PR not read yet.
`searches.json` is two saved searches as if searched, which `--demo` lists.

Its files never change, so a session would read as dead (a session file is live while its mtime is
under 60 s old) and the results as hours old. `--demo` shows it as if it were live: every session file
counts as just written, the clock starts 12 s after the results' round, and nothing polls. Without
`--demo`, touch the session files first (`touch companion/demo/sessions/*.json`) and the header says
how old the results really are (and pass `--no-poll`, or `prw` polls GitHub for the demo's PRs).

### Watching for other agents

```sh
prw watch https://github.com/acme/app/pull/12          # in this folder's session, as this agent
prw watch <url> --agent codex --session billing         # named
prw watch                                               # what is watched, by session
prw unwatch <url>                                       # everywhere (--session/--agent: just there)
prw unwatch --all --session billing
```

The agent is `--agent`, else `$PRW_AGENT`, else `claude` or `codex` where `prw watch` runs inside
one, else `cli`. The session is `--session`, else the current folder's name; a session named by its
folder is that folder's (two projects in folders of one name are two sessions), and one named with
`--session` is one session wherever it is watched from. A watch lasts until it is unwatched, or its PR
has been merged or closed for an hour. While a `prw` TUI runs, every agent's session with a watch is
listed and polled; with none running, a `prw watch` session is polled for a minute (a running
`prw mcp` keeps its own going), and `prw watch` says so.

`prw mcp` serves the same over MCP on stdio, for any agent that takes MCP servers: a `watch` tool
(the mod's name and `urls` argument) and an `unwatch`, in the agent's session (its agent from
`--agent`, else the MCP client's name; its session from the folder the agent starts it in). For
Codex:

```sh
codex mcp add prw -- prw mcp
```

or in `~/.codex/config.toml`:

```toml
[mcp_servers.prw]
command = "prw"
args = ["mcp"]
```

Then a line in the project's `AGENTS.md` makes it a habit: "When you open or push to a pull request,
ask whether to watch it in prw, and call prw's `watch` tool with its URL if the user agrees."

### Keys

| key                  | does                                                         |
|----------------------|--------------------------------------------------------------|
| `j` `k`, `↓` `↑`     | the next or previous PR (or list, in the lists pane)         |
| `g` `G`, Home End    | the first or last                                            |
| Tab, Shift-Tab       | the next or previous list                                    |
| `h` `l`, `←` `→`     | three panes: the lists pane or the PRs; narrower: the previous or next list |
| `m`                  | merge the PR, after a `y` (`m` again: the next method)       |
| `c`                  | comment on it                                                |
| `f`                  | hand it off: copy the Fix, Resolve or Address prompt         |
| `o`                  | open it in the browser                                       |
| `a`                  | switch the gh login its owner's PRs go as (with two or more)  |
| `v`                  | show the prompt last copied (`j` `k` scroll, Esc closes)     |
| `?`                  | the key help                                                 |
| `q`, Esc, Ctrl-C     | quit (Esc closes the help, a question or the field first)    |

Enter (the full PR view) comes in a later step.

The mouse works too. A click picks a list or a PR, or presses the key it shows: a hint on the
bottom row, an action on the card, an answer to the question on the status line (while a question
is open, only its answers take clicks). The wheel moves in the pane under it. While `v` shows the
prompt, prw lets go of the mouse, so a plain drag selects the prompt's text; elsewhere most
terminals select with Shift (iTerm2: Option) held.

## Layout

The screen adapts to the terminal's width (`prw_core::layout`, and `prw_core::panes` for where each
pane goes):

| width (columns) | layout        | shows                                                         |
|-----------------|---------------|---------------------------------------------------------------|
| 100 and up      | three panes   | the lists, the PR table and the selected PR's card side by side |
| 60 to 99        | strip         | the lists as a strip on top; the table and the card below, side by side |
| 40 to 59        | side          | one list (Tab for the next), the table, and a short card under it (beside an agent) |
| under 40        | too narrow    | "prw needs 40 columns"                                        |

The cut-offs sit between the design's reference widths (120, 84 and 49 columns), so each lands well
inside its own band. A wider terminal never gets a smaller layout.

## Crates

- `crates/prw-core`: the pure kernel (no I/O, no clock): protocol types, status logic, the layout, the
  TUI's model (`ui`: `update(Model, Msg) -> (Model, Vec<Cmd>)`) and the poll's decisions, ported from
  the mod (`poll`, `batch`, `accounts`, `rate`, `entries`, `search`), the saved searches (`config`),
  other agents' watches (`watch`),
  the MCP messages (`mcp`), and the actions (`act`: what each key asks and shows; `merge`, `comment`,
  and the hand-off prompts in `handoff`, `fix`, `resolve` and `address`), tested with properties and against the mod's fixtures
  (`tests/fixtures.rs`).
- `crates/prw`: the binary: the terminal (ratatui), reading the cache (`cache.rs`), drawing the model
  (`view.rs`), and the poller thread (`poller.rs`: a tick every 10 s; `round.rs`: asking GitHub; `gh.rs`:
  running `gh` and the clock, behind traits the tests fake), the actions (`actions.rs`: running each one as its own thread; `desktop.rs`: the clipboard and the
  browser), and the other commands (`cli.rs`: `watch`
  and `unwatch`; `mcp.rs`: the tool server; `watches.rs`: the watch files and their sessions).
