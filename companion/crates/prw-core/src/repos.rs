//! How a row names its repo, and which of the repo colours it wears.

use std::collections::{HashMap, HashSet};

/// What each of `repos` (`owner/name`) is shown as: its name alone, or `owner/name` where
/// another of them has the same name under a different owner (names compared ignoring case,
/// as GitHub does).
#[must_use]
pub fn short_names<'a>(repos: impl IntoIterator<Item = &'a str>) -> HashMap<&'a str, String> {
    let mut repos: Vec<&str> = repos.into_iter().collect();
    repos.sort_unstable();
    repos.dedup();
    // Each name's repos, ignoring case: `acme/web` and `Acme/Web` are one repo, not namesakes.
    let mut owners: HashMap<String, HashSet<String>> = HashMap::new();
    for repo in &repos {
        owners
            .entry(name_of(repo).to_lowercase())
            .or_default()
            .insert(repo.to_lowercase());
    }
    repos
        .into_iter()
        .map(|repo| {
            let name = name_of(repo);
            let shown = if owners[&name.to_lowercase()].len() > 1 {
                repo
            } else {
                name
            };
            (repo, shown.to_owned())
        })
        .collect()
}

/// The part after the `/` (all of it where there is none).
#[must_use]
pub fn name_of(repo: &str) -> &str {
    repo.split_once('/').map_or(repo, |(_, name)| name)
}

/// Which of `hues` colours `repo`'s chip: an FNV-1a hash of its `owner/name` (lower case), so
/// a repo keeps its colour across runs and machines. `hues` is at least 1.
#[must_use]
pub fn hue_of(repo: &str, hues: usize) -> usize {
    let hash = repo
        .to_lowercase()
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        });
    usize::try_from(hash % u64::try_from(hues.max(1)).unwrap_or(1)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn the_owner_shows_only_where_two_repos_share_a_name() {
        let names = short_names(["acme/web", "me/Web", "acme/api", "acme/web"]);
        assert_eq!(names.len(), 3);
        assert_eq!(names["acme/web"], "acme/web");
        assert_eq!(names["me/Web"], "me/Web");
        assert_eq!(names["acme/api"], "api");
        // One repo in two cases is no namesake of itself.
        let names = short_names(["acme/web", "Acme/web"]);
        assert_eq!(names["acme/web"], "web");
        assert_eq!(names["Acme/web"], "web");
    }

    #[test]
    fn a_repos_hue_is_the_same_whatever_its_case() {
        assert_eq!(hue_of("Acme/Web", 8), hue_of("acme/web", 8));
        assert_eq!(hue_of("acme/web", 1), 0);
    }

    proptest! {
        #[test]
        fn every_repo_gets_a_hue_in_range(repo in "[a-z]{1,8}/[a-zA-Z.-]{1,12}", hues in 1_usize..20) {
            prop_assert!(hue_of(&repo, hues) < hues);
        }

        #[test]
        fn a_shown_name_ends_with_the_repos_name(repos in prop::collection::vec("[ab]{1,2}/[xyX]{1,2}", 1..8)) {
            let names = short_names(repos.iter().map(String::as_str));
            for repo in &repos {
                let shown = &names[repo.as_str()];
                prop_assert!(repo.ends_with(shown.as_str()));
                prop_assert!(shown == repo || !shown.contains('/'));
            }
        }
    }
}
