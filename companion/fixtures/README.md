# Status fixtures

`status.ts` holds the cases the companion's status logic is held to, so the TypeScript mod and the Rust port (`prw-core`) say the same thing about the same PR. Each case has:

- `name` and `about`: what the case is.
- `pr`: a PR exactly as the poll stores it in `~/.cache/pr-watch/results.json` under `prs[url].pr`. It is built as GitHub's answer to the batched GraphQL query and read by the mod's own `parseReply` (`hooks/batch.ts`), so its shape is the stored shape: checks flattened into `statusCheckRollup` with their `workflow`, the repository's merge settings under `repository`, and the review connections.
- `card`: what the mod derives from it, as data only. There is no theme, no colour and no clock in it (a card's *checked 1m ago* comes from the reader).
  - `stage`, `step`, `tone`, `pill`, `headline`, `isMoving` and `checks` come from `derive` and `stepIndex` (`hooks/pr.ts`). `pillKind` is the pill's key in `PILL`, which `hooks/band.ts` ranks by.
  - `isConflicted` comes from `metaOf`.
  - `merge` comes from `mergeableOf`: the methods, and whether to offer merge, auto-merge or Cancel auto-merge.
  - `reviews` comes from `reviewsOf` (`hooks/review.ts`).

The cases cover drafts, running, failing and re-run checks, status contexts, an optional check failing (`UNSTABLE`), approved and mergeable, read-only, open and capped threads, changes requested, waiting for review, conflicts (also while GitHub is still computing), behind, blocked, auto-merge offered, on, and on but blocked, no checks, merged and closed. Every pill a polled PR can lead to appears at least once. The poll doesn't ask whether a check is required, so the mod doesn't tell required checks from optional ones. `UNSTABLE` is how GitHub says only optional checks fail.

## Why a `.ts` file

Rust reads the fixtures as JSON, but the mod's tests run where hooks run: there is no file system, and an import has to be code (`.ts`, `.js`, …), not `.json`. So `status.ts` is a JSON array behind one `export default`. **The JSON starts at the file's first `[`.** The comment lines above it contain none.

```rust
// From crates/prw-core/tests/:
let src = include_str!("../../../fixtures/status.ts");
let cases: Vec<Case> = serde_json::from_str(&src[src.find('[').unwrap()..])?;
```

## Changing them

The cases live in `hooks/fixtures.test.ts`. `status.ts` is generated: don't edit it by hand.

`hooks/fixtures.test.ts` fails whenever `status.ts` differs from what the cases and the mod's code give now. That happens when a case is added or changed, or when `derive`, `mergeableOf`, `reviewsOf` or `parseReply` changes what they say. To regenerate:

```sh
sh companion/fixtures/update.sh
```

This runs `claude plugin test .`, which prints the expected file between two marker lines when the stored one differs (a test can't write files), and the script writes that to `status.ts`. Review the diff: every changed `card` is a change the Rust port must make too, and its tests fail until it does.
