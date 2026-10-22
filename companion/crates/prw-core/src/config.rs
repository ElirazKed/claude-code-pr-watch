//! `prw`'s own settings, `config.toml`: the saved searches, each a list of the PRs GitHub's
//! search finds for it. The cache stays the mod's JSON; this file is `prw`'s alone.
//!
//! ```toml
//! [[search]]
//! name = "Needs my review"
//! query = "is:open review-requested:@me"
//!
//! [[search]]
//! name = "Team web"
//! query = "is:open repo:acme/web"
//! login = "octocat"   # only as this gh login; else as each
//! ```

use serde::Deserialize;

/// The most saved searches run: each costs a point a minute per login.
pub const SEARCHES_MAX: usize = 10;

/// The most PRs a saved search lists ("30 of 112").
pub const SEARCH_CAP: usize = 30;

/// One saved search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Search {
    /// What the list is called; unique in the file.
    pub name: String,
    /// The search as written, for GitHub's search box.
    pub query: String,
    /// The gh login to search as; `None`: as each, their finds together.
    pub login: Option<String>,
}

/// What `config.toml` sets, as `prw` uses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub searches: Vec<Search>,
    /// What was wrong with the file, where something was: the searches that read are kept.
    pub problems: Vec<String>,
}

impl Default for Config {
    /// With no file: the PRs waiting on your review, as the plan's lists have it.
    fn default() -> Self {
        Self {
            searches: vec![Search {
                name: "Review requests".to_owned(),
                query: "is:open review-requested:@me".to_owned(),
                login: None,
            }],
            problems: Vec::new(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    search: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    query: String,
    login: Option<String>,
}

/// `config.toml`'s text. A file that won't parse runs no search, and says why; a search
/// with no name or query, or a name taken already, is left out, and said.
#[must_use]
pub fn parse_config(text: &str) -> Config {
    let file: File = match toml::from_str(text) {
        Ok(file) => file,
        Err(error) => {
            return Config {
                searches: Vec::new(),
                problems: vec![format!("config.toml: {}", one_line(&error.to_string()))],
            };
        }
    };
    let mut problems = Vec::new();
    let mut searches: Vec<Search> = Vec::new();
    for (i, entry) in file.search.into_iter().enumerate() {
        let name = entry.name.trim().to_owned();
        let query = entry.query.trim().to_owned();
        let at = format!("config.toml: search {}", i + 1);
        if name.is_empty() || query.is_empty() {
            problems.push(format!("{at} needs a name and a query"));
        } else if searches.iter().any(|s| s.name == name) {
            problems.push(format!(
                "{at}: \"{name}\" is named twice; the first is used"
            ));
        } else {
            searches.push(Search {
                name,
                query,
                login: entry
                    .login
                    .map(|l| l.trim().to_owned())
                    .filter(|l| !l.is_empty()),
            });
        }
    }
    if searches.len() > SEARCHES_MAX {
        problems.push(format!(
            "config.toml: {} searches; the first {SEARCHES_MAX} run",
            searches.len()
        ));
        searches.truncate(SEARCHES_MAX);
    }
    Config { searches, problems }
}

/// The text GitHub is asked: the query as written, made a PR search where it doesn't say so
/// (`type: ISSUE` finds issues too, which no list shows).
#[must_use]
pub fn search_text(query: &str) -> String {
    let is_pr = query.split_whitespace().any(|word| {
        let word = word.to_ascii_lowercase();
        ["is:pr", "type:pr", "is:pull-request", "type:pull-request"].contains(&word.as_str())
    });
    if is_pr {
        query.to_owned()
    } else {
        format!("is:pr {query}")
    }
}

/// TOML's errors run over several lines (the line, a caret); the toast has one.
fn one_line(error: &str) -> String {
    error
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('|') && !l.starts_with('^'))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn searches_read_in_order_with_their_logins() {
        let config = parse_config(
            r#"
[[search]]
name = "Needs my review"
query = "is:open review-requested:@me"

[[search]]
name = " Team web "
query = "is:open repo:acme/web"
login = "octocat"
"#,
        );
        assert_eq!(config.problems, Vec::<String>::new());
        assert_eq!(
            config.searches,
            [
                Search {
                    name: "Needs my review".into(),
                    query: "is:open review-requested:@me".into(),
                    login: None,
                },
                Search {
                    name: "Team web".into(),
                    query: "is:open repo:acme/web".into(),
                    login: Some("octocat".into()),
                },
            ]
        );
    }

    #[test]
    fn an_empty_file_has_no_searches_and_no_file_has_the_default() {
        assert_eq!(parse_config("").searches, []);
        assert_eq!(Config::default().searches[0].name, "Review requests");
    }

    #[test]
    fn what_is_wrong_is_said_and_the_rest_kept() {
        let config = parse_config(
            r#"
[[search]]
name = "A"
query = "is:open"
[[search]]
name = "A"
query = "is:closed"
[[search]]
name = ""
query = "x"
"#,
        );
        assert_eq!(config.searches.len(), 1);
        assert_eq!(
            config.problems,
            [
                "config.toml: search 2: \"A\" is named twice; the first is used",
                "config.toml: search 3 needs a name and a query",
            ]
        );
        let broken = parse_config("[[search]\nname = 1");
        assert_eq!(broken.searches, []);
        assert_eq!(broken.problems.len(), 1);
        assert!(!broken.problems[0].contains('\n'));
        // A key it doesn't know is a typo to say, not to pass over.
        let typo = parse_config("[[search]]\nname = \"A\"\nqeury = \"x\"\nquery = \"y\"");
        assert_eq!(typo.searches, []);
        assert!(typo.problems[0].contains("qeury"), "{:?}", typo.problems);
    }

    #[test]
    fn past_the_most_only_the_first_run() {
        let text = (0..12)
            .map(|i| format!("[[search]]\nname = \"s{i}\"\nquery = \"is:open\"\n"))
            .collect::<Vec<_>>()
            .concat();
        let config = parse_config(&text);
        assert_eq!(config.searches.len(), SEARCHES_MAX);
        assert_eq!(
            config.problems,
            ["config.toml: 12 searches; the first 10 run"]
        );
    }

    #[test]
    fn a_search_is_made_a_pr_search() {
        assert_eq!(
            search_text("is:open author:@me"),
            "is:pr is:open author:@me"
        );
        assert_eq!(search_text("is:PR is:open"), "is:PR is:open");
        assert_eq!(
            search_text("type:pr repo:acme/web"),
            "type:pr repo:acme/web"
        );
    }

    proptest! {
        #[test]
        fn any_text_parses_without_panicking(text in "\\PC{0,200}") {
            let config = parse_config(&text);
            prop_assert!(config.searches.len() <= SEARCHES_MAX);
        }

        #[test]
        fn a_pr_search_stays_one(query in "[a-z:@/ -]{0,40}") {
            let once = search_text(&query);
            prop_assert_eq!(search_text(&once), once.clone());
            prop_assert!(once.ends_with(query.as_str()));
        }
    }
}
