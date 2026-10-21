# prw

`prw` is an optional terminal companion for the pr-watch mod. It watches every PR your agents touch
(Claude through the mod; Codex and others through `prw mcp` or `prw watch`) beside your saved GitHub
searches, in a responsive TUI with a full PR view (conversation, threads, checks, files), so GitHub can
stay closed. The mod stays complete on its own; `prw` is for when you want the big picture, with or
without Claude.

**Status: step 3 of the plan: core ports, parity with the mod.** `prw-core` reads the shared files
(`PROTOCOL.md`) and works out each PR's card as the mod does (`derive`, checks, merge options, reviews,
urgency order), held to the mod's own fixtures (`fixtures/`). The binary is still the skeleton: it opens
a box that shows the terminal's width and the layout it would use, follows resizes, and quits on `q`,
Esc or Ctrl-C. Nothing reads PRs on screen yet.

## Build and run

With nix (flakes), from the repository root:

```sh
nix run .#prw              # build and run
nix build .#prw            # ./result/bin/prw
nix develop                # cargo, rustc, clippy, rustfmt, rust-analyzer
cd companion && cargo run -p prw
```

Without nix, any recent stable Rust (1.88 or newer):

```sh
cd companion
cargo run -p prw
cargo test --workspace
```

## Layout

The screen adapts to the terminal's width (`prw_core::layout`):

| width (columns) | layout        | shows                                           |
|-----------------|---------------|-------------------------------------------------|
| 100 and up      | three panes   | lists, PR cards and the selected PR side by side |
| 60 to 99        | strip         | the lists as a strip on top, the cards below     |
| 40 to 59        | side          | one list and the selected PR's card (beside an agent) |
| under 40        | too narrow    | a "too narrow" screen                            |

The cut-offs sit between the design's reference widths (120, 84 and 49 columns), so each lands well
inside its own band. A wider terminal never gets a smaller layout.

## Crates

- `crates/prw-core`: the pure kernel (no I/O): protocol types and status logic, tested with properties
  and against the mod's fixtures (`tests/fixtures.rs`).
- `crates/prw`: the binary: terminal, files, `gh`.
