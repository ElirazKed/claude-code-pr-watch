//! Which gh login asks about a PR (`hooks/accounts.ts`). Every call made as a login carries
//! that login's own token, the active one's too: gh's active account is whichever was
//! switched to last, in any terminal, so it says nothing about who a call runs as.

use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

use regex::Regex;

use crate::batch::PrRef;

/// gh's logins on github.com: the active one, and the rest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Logins {
    pub active: String,
    pub others: Vec<String>,
}

static ACCOUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"account (\S+)[^\n]*\n\s*-\s*Active account: (true|false)")
        .expect("a valid pattern")
});

/// The logins `gh auth status` lists (`loginsOf`), or `None` where it lists none (gh timed
/// out, or nobody is logged in): a reading worth nothing, so never kept. Where none is marked
/// active, the first stands for it.
#[must_use]
pub fn logins_of(text: &str) -> Option<Logins> {
    let all: Vec<(&str, bool)> = ACCOUNT
        .captures_iter(text)
        .map(|m| {
            let login = m.get(1).map_or("", |l| l.as_str());
            (login, &m[2] == "true")
        })
        .filter(|(login, _)| !login.is_empty())
        .collect();
    let active = all
        .iter()
        .find(|(_, is_active)| *is_active)
        .or_else(|| all.first())?
        .0;
    let mut others: Vec<String> = Vec::new();
    for (login, _) in &all {
        if *login != active && !others.iter().any(|o| o == login) {
            others.push((*login).to_owned());
        }
    }
    Some(Logins {
        active: active.to_owned(),
        others,
    })
}

/// Every login, the active one first (`everyLogin`).
#[must_use]
pub fn every_login(logins: &Logins) -> Vec<String> {
    let mut all: Vec<String> = Vec::new();
    for login in std::iter::once(&logins.active).chain(&logins.others) {
        if !login.is_empty() && !all.contains(login) {
            all.push(login.clone());
        }
    }
    all
}

/// The login an owner's PRs are polled as (`loginFor`): the one found to see them, while it
/// is still logged in (or nothing says it isn't), else the active one.
#[must_use]
pub fn login_for(accounts: &BTreeMap<String, String>, owner: &str, logins: &Logins) -> String {
    let all = every_login(logins);
    match accounts.get(owner) {
        Some(mapped) if all.is_empty() || all.contains(mapped) => mapped.clone(),
        _ => logins.active.clone(),
    }
}

/// What the card says where gh won't give a login's token (`tokenError`), or, with no login
/// at all, where gh's logins couldn't be read.
#[must_use]
pub fn token_error(login: &str) -> String {
    if login.is_empty() {
        "Couldn't read gh's logins (gh auth status)".to_owned()
    } else {
        format!("Couldn't get {login}'s gh token")
    }
}

/// How long a reading of gh's logins, or a login's token, is kept (ms): a login added,
/// switched to or logged in again shows within this.
pub const KEEP_MS: i64 = 300_000;
/// A reading that failed is asked again this long after (ms), so a keyring locked after
/// sleep isn't asked on every call.
pub const AGAIN_MS: i64 = 5_000;

/// When gh was last asked, and whether it answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asked {
    pub at: i64,
    pub is_ok: bool,
}

/// Whether to ask gh again (`isDue`): never asked, a failure [`AGAIN_MS`] old, or a good
/// answer [`KEEP_MS`] old.
#[must_use]
pub fn is_due(last: Option<Asked>, at: i64) -> bool {
    last.is_none_or(|last| {
        at.saturating_sub(last.at) >= if last.is_ok { KEEP_MS } else { AGAIN_MS }
    })
}

/// The missing PRs `login` is still to be asked about: those it hasn't answered for
/// (`untriedBy`).
#[must_use]
pub fn untried_by<'a, S: std::hash::BuildHasher>(
    missing: &'a [PrRef],
    asked: &HashMap<String, Vec<String>, S>,
    login: &str,
) -> Vec<&'a PrRef> {
    missing
        .iter()
        .filter(|r| {
            !asked
                .get(&r.url)
                .is_some_and(|by| by.iter().any(|l| l == login))
        })
        .collect()
}

/// What a PR no login could see says (`notFoundAs`): the logins that answered, "Not found as
/// octocat or hubot".
#[must_use]
pub fn not_found_as(asked: &[String]) -> String {
    let mut names: Vec<&str> = Vec::new();
    for login in asked {
        if !login.is_empty() && !names.contains(&login.as_str()) {
            names.push(login);
        }
    }
    match names.as_slice() {
        [] => "Not found".to_owned(),
        [one] => format!("Not found as {one}"),
        [rest @ .., last] => format!("Not found as {} or {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::ref_from_url;

    const T: i64 = 1_791_374_400_000;

    /// `gh auth status` as gh prints it, with two logins, the second one active.
    const STATUS: &str = "github.com
  ✓ Logged in to github.com account octocat (keyring)
  - Active account: false
  - Git operations protocol: ssh
  - Token: gho_************************************

  ✓ Logged in to github.com account hubot (keyring)
  - Active account: true
  - Git operations protocol: ssh";

    fn logins(active: &str, others: &[&str]) -> Logins {
        Logins {
            active: active.into(),
            others: others.iter().map(|&o| o.to_owned()).collect(),
        }
    }

    #[test]
    fn the_active_one_apart_from_the_rest_whatever_order_gh_lists_them_in() {
        assert_eq!(logins_of(STATUS), Some(logins("hubot", &["octocat"])));
        assert_eq!(
            every_login(&logins("hubot", &["octocat"])),
            ["hubot", "octocat"]
        );
    }

    #[test]
    fn one_whose_token_gh_calls_invalid_is_still_a_login() {
        let text = format!(
            "{STATUS}\n  X Failed to log in to github.com account monalisa (keyring)\n  - Active account: false\n  - The token in keyring is invalid.\n"
        );
        assert_eq!(
            logins_of(&text),
            Some(logins("hubot", &["octocat", "monalisa"]))
        );
    }

    #[test]
    fn a_status_that_lists_none_is_no_reading() {
        assert_eq!(logins_of(""), None);
        assert_eq!(
            logins_of("You are not logged into any GitHub hosts. To log in, run: gh auth login"),
            None
        );
        assert_eq!(every_login(&Logins::default()), Vec::<String>::new());
    }

    #[test]
    fn none_marked_active_the_first_stands_for_it() {
        let text = STATUS.replace("Active account: true", "Active account: false");
        assert_eq!(logins_of(&text), Some(logins("octocat", &["hubot"])));
    }

    #[test]
    fn a_good_answer_stands_keep_ms_a_failure_again_ms() {
        let ok = Some(Asked { at: T, is_ok: true });
        let failed = Some(Asked {
            at: T,
            is_ok: false,
        });
        assert!(is_due(None, T));
        assert!(!is_due(ok, T + KEEP_MS - 1));
        assert!(is_due(ok, T + KEEP_MS));
        assert!(!is_due(failed, T + AGAIN_MS - 1));
        assert!(is_due(failed, T + AGAIN_MS));
    }

    #[test]
    fn an_owners_prs_go_as_the_login_found_to_see_them_others_as_the_active_one() {
        let known = logins("octocat", &["hubot"]);
        let accounts: BTreeMap<String, String> = [("acme".into(), "hubot".into())].into();
        assert_eq!(login_for(&accounts, "acme", &known), "hubot");
        assert_eq!(login_for(&accounts, "globex", &known), "octocat");
        // Logged out since: the active one.
        let gone: BTreeMap<String, String> = [("acme".into(), "monalisa".into())].into();
        assert_eq!(login_for(&gone, "acme", &known), "octocat");
        // gh's logins unknown: the remembered one stands.
        assert_eq!(login_for(&accounts, "acme", &Logins::default()), "hubot");
        assert_eq!(login_for(&BTreeMap::new(), "acme", &Logins::default()), "");
    }

    #[test]
    fn a_login_is_asked_only_about_the_missing_prs_it_has_not_answered_for() {
        let pr = |n: u64| ref_from_url(&format!("https://github.com/acme/app/pull/{n}")).unwrap();
        let missing = [pr(1), pr(2)];
        let asked: HashMap<String, Vec<String>> =
            [(missing[0].url.clone(), vec!["octocat".into()])].into();
        assert_eq!(untried_by(&missing, &asked, "octocat"), [&missing[1]]);
        assert_eq!(
            untried_by(&missing, &asked, "hubot"),
            [&missing[0], &missing[1]]
        );
    }

    #[test]
    fn the_logins_asked_in_the_order_they_were() {
        let of =
            |names: &[&str]| not_found_as(&names.iter().map(|&n| n.to_owned()).collect::<Vec<_>>());
        assert_eq!(of(&["octocat"]), "Not found as octocat");
        assert_eq!(of(&["octocat", "hubot"]), "Not found as octocat or hubot");
        assert_eq!(
            of(&["octocat", "hubot", "monalisa", "hubot"]),
            "Not found as octocat, hubot or monalisa"
        );
        assert_eq!(of(&[]), "Not found");
    }

    #[test]
    fn a_token_gh_wont_give_names_its_login() {
        assert_eq!(token_error("hubot"), "Couldn't get hubot's gh token");
        assert_eq!(
            token_error(""),
            "Couldn't read gh's logins (gh auth status)"
        );
    }
}
