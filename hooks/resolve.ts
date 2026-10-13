// Handing a conflicted PR to Claude: the message that asks to bring its base in and resolve
// the conflicts. GitHub doesn't say which files conflict, so there is nothing to fetch: git
// finds them once the base is merged in. Pure, so the pane only delivers.
import type { TrackedPr } from '../types'

type PrFacts = Pick<TrackedPr, 'repo' | 'number' | 'title' | 'url' | 'branch' | 'base'>

// The draft's first words, by which a press finds an earlier draft of its own in the prompt box.
export const resolveHeading = (pr: Pick<PrFacts, 'repo' | 'number'>) => `Merge conflicts on ${pr.repo}#${pr.number}:`

// The message for Claude: the PR, what GitHub says, and the ask. A merge, not a rebase, so the
// pushed history stays as it is and no force-push is needed; and like Fix with Claude's, it
// never asks for a push or a merge: that stays the person's call.
export function resolvePrompt(pr: PrFacts): string {
  const branch = pr.branch || 'its head branch'
  const base = pr.base || 'its base'
  const origin = pr.base ? `origin/${pr.base}` : 'the base from origin'

  return [
    `${resolveHeading(pr)} ${pr.title || 'untitled'}`,
    `${pr.url} · ${branch} → ${base}`,
    '',
    `GitHub says ${branch} can't be merged cleanly into ${base}.`,
    '',
    `Bring ${base} into this branch (${branch}): fetch, then merge ${origin} into it, so no history is rewritten and ` +
      `no force-push is needed. If origin isn't ${pr.repo} (a fork's clone), merge ${base} from the remote that is. ` +
      `If the branch or repo clearly works by rebase instead, say so and ask me first. ` +
      `If ${pr.repo} or ${branch} isn't checked out here, say so first.`,
    "Resolve each conflict keeping what both sides meant; where a side's intent is unclear, ask me rather than pick one. " +
      "Then run the project's checks and tests, and commit the merge.",
    "Don't push: that stays my call.",
  ].join('\n')
}
