//! Handing a conflicted PR to an agent: the message that asks to bring its base in and
//! resolve the conflicts. A port of `hooks/resolve.ts`. GitHub doesn't say which files
//! conflict, so there is nothing to fetch: git finds them once the base is merged in.

use crate::handoff::PrFacts;

/// The prompt's first words (`resolveHeading`).
#[must_use]
pub fn resolve_heading(pr: &PrFacts) -> String {
    format!("Merge conflicts on {}#{}:", pr.repo, pr.number)
}

/// The message for the agent: the PR, what GitHub says, and the ask (`resolvePrompt`). A
/// merge, not a rebase, so the pushed history stays as it is and no force-push is needed; it
/// never asks for a push or a merge of the PR: that stays the person's call.
#[must_use]
pub fn resolve_prompt(pr: &PrFacts) -> String {
    let branch = pr.branch();
    let base = pr.base();
    let origin = if pr.base.is_empty() {
        "the base from origin".to_owned()
    } else {
        format!("origin/{}", pr.base)
    };
    let repo = &pr.repo;
    [
        format!("{} {}", resolve_heading(pr), pr.title()),
        format!("{} · {branch} → {base}", pr.url),
        String::new(),
        format!("GitHub says {branch} can't be merged cleanly into {base}."),
        String::new(),
        format!(
            "Bring {base} into this branch ({branch}): fetch, then merge {origin} into it, so no history is rewritten and \
             no force-push is needed. If origin isn't {repo} (a fork's clone), merge {base} from the remote that is. \
             If the branch or repo clearly works by rebase instead, say so and ask me first. \
             If {repo} or {branch} isn't checked out here, say so first."
        ),
        "Resolve each conflict keeping what both sides meant; where a side's intent is unclear, ask me rather than pick one. \
         Then run the project's checks and tests, and commit the merge."
            .to_owned(),
        "Don't push: that stays my call.".to_owned(),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> PrFacts {
        PrFacts {
            repo: "acme/widgets".to_owned(),
            number: 12,
            title: "Retry uploads".to_owned(),
            url: "https://github.com/acme/widgets/pull/12".to_owned(),
            branch: "feat/retry".to_owned(),
            base: "main".to_owned(),
            author: "octocat".to_owned(),
        }
    }

    #[test]
    fn names_the_pr_and_asks_for_a_merge_of_the_base_not_a_push() {
        let text = resolve_prompt(&facts());
        assert!(text.starts_with("Merge conflicts on acme/widgets#12: Retry uploads\n"));
        assert!(text.contains("https://github.com/acme/widgets/pull/12 · feat/retry → main"));
        assert!(text.contains("GitHub says feat/retry can't be merged cleanly into main."));
        assert!(text.contains("fetch, then merge origin/main into it, so no history is rewritten"));
        assert!(
            text.contains("If acme/widgets or feat/retry isn't checked out here, say so first.")
        );
        assert!(text.ends_with("Don't push: that stays my call."));
    }

    #[test]
    fn an_unknown_base_is_still_named() {
        let text = resolve_prompt(&PrFacts {
            base: String::new(),
            ..facts()
        });
        assert!(text.contains("merge the base from origin into it"));
        assert!(text.contains("can't be merged cleanly into its base."));
    }
}
