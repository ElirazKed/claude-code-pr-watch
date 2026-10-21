# prw

`prw` is an optional terminal companion for the pr-watch mod. It watches every PR your agents touch
(Claude through the mod; Codex and others through `prw mcp` or `prw watch`) beside your saved GitHub
searches, in a responsive TUI with a full PR view (conversation, threads, checks, files), so GitHub can
stay closed. The mod stays complete on its own; `prw` is for when you want the big picture, with or
without Claude.

**Status: step 4 of the plan: the kernel, read-only.** `prw` shows the PRs your sessions watch, from
the mod's shared cache (`PROTOCOL.md`): a list of all of them, one list per live session (Claude
through the mod; others by the `agent` their session file names), each PR's card worked out as the mod
does (`prw-core`, held to the mod's own fixtures in `fixtures/`). It reads `~/.cache/pr-watch/` about
once a second and never writes to it, and it doesn't poll GitHub itself yet: with no Claude session
running the mod, the header says "no poller" and the results stop moving.

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

`--cache <dir>` (or `PRW_CACHE`) reads another cache directory than `~/.cache/pr-watch`.

### The demo cache

`demo/` is a hand-made cache: a `results.json` of acme/* PRs built from the status fixtures (CI
running, failed, conflicts, changes requested, comments to address, in review, ready, merged; one PR
no gh account could see, one whose fetches have failed for a while) and four session files: two
Claude sessions (`app-retry`, `web-checkout`), a Codex session written by `prw` (`billing`), and an
older mod's file with only `urls` (listed as "Claude session"), which also watches a PR not read yet.

Its files never change, so a session would read as dead (a session file is live while its mtime is
under 60 s old) and the results as hours old. `--demo` shows it as if it were live: every session file
counts as just written, and the clock starts 12 s after the results' round. Without `--demo`, touch the
session files first (`touch companion/demo/sessions/*.json`) and the header says how old the results
really are.

### Keys

| key                  | does                                                         |
|----------------------|--------------------------------------------------------------|
| `j` `k`, `↓` `↑`     | the next or previous PR (or list, in the lists pane)         |
| `g` `G`, Home End    | the first or last                                            |
| Tab, Shift-Tab       | the next or previous list                                    |
| `h` `l`, `←` `→`     | three panes: the lists pane or the PRs; narrower: the previous or next list |
| `?`                  | the key help                                                 |
| `q`, Esc, Ctrl-C     | quit (Esc closes the help first)                             |

Enter (the full PR view), merging, commenting and hand-offs come in later steps.

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

- `crates/prw-core`: the pure kernel (no I/O, no clock): protocol types, status logic, the layout and
  the TUI's model (`ui`: `update(Model, Msg) -> (Model, Vec<Cmd>)`), tested with properties and against
  the mod's fixtures (`tests/fixtures.rs`).
- `crates/prw`: the binary: the terminal (ratatui), reading the cache (`cache.rs`), drawing the model
  (`view.rs`).
