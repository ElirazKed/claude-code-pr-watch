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

/// The login an owner's PRs are polled as (`loginFor`): the one remembered for it, while it
/// is still logged in (or nothing says it isn't), else the login of the owner's own name, else
/// the active one.
#[must_use]
pub fn login_for(accounts: &BTreeMap<String, String>, owner: &str, logins: &Logins) -> String {
    let all = every_login(logins);
    match accounts.get(owner) {
        Some(mapped) if all.is_empty() || all.contains(mapped) => mapped.clone(),
        _ => all
            .into_iter()
            .find(|login| login.eq_ignore_ascii_case(owner))
            .unwrap_or_else(|| logins.active.clone()),
    }
}

/// The login after `current` in [`every_login`]'s order, round to the first: what a switch
/// moves an owner to. `None` with fewer than two logins.
#[must_use]
pub fn next_login(logins: &Logins, current: &str) -> Option<String> {
    let all = every_login(logins);
    if all.len() < 2 {
        return None;
    }
    let at = all.iter().position(|l| l == current);
    Some(all[at.map_or(0, |i| (i + 1) % all.len())].clone())
}

/// What a fetch learned about `accounts`: the owners whose login it set or changed, so a
/// writer merges those alone and a login set meanwhile (a switch) isn't put back.
#[must_use]
pub fn learned(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    after
        .iter()
        .filter(|&(owner, login)| before.get(owner) != Some(login))
        .map(|(o, l)| (o.clone(), l.clone()))
        .collect()
}

/// GitHub's `viewerPermission` as a rank, more rights higher (`rankOf`): 0 for none or one
/// unknown.
#[must_use]
pub fn rank_of(permission: Option<&str>) -> usize {
    ["READ", "TRIAGE", "WRITE", "MAINTAIN", "ADMIN"]
        .iter()
        .position(|&p| Some(p) == permission)
        .map_or(0, |i| i + 1)
}

/// The owners worth asking as each other login (`ownersBelowWrite`): no login remembered for
/// them, and their PRs seen with less than write (anyone sees a public repo, the other
/// account's too, but only to read). With one login there is nobody else to ask.
#[must_use]
pub fn owners_below_write<'a>(
    seen: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
    accounts: &BTreeMap<String, String>,
    logins: &Logins,
) -> Vec<String> {
    if every_login(logins).len() < 2 {
        return Vec::new();
    }
    let mut best: Vec<(&str, usize)> = Vec::new();
    for (owner, permission) in seen {
        let rank = rank_of(permission);
        match best.iter_mut().find(|(o, _)| *o == owner) {
            Some((_, was)) => *was = (*was).max(rank),
            None => best.push((owner, rank)),
        }
    }
    best.into_iter()
        .filter(|&(owner, rank)| {
            !owner.is_empty() && !accounts.contains_key(owner) && rank < rank_of(Some("WRITE"))
        })
        .map(|(owner, _)| owner.to_owned())
        .collect()
}

/// One login's answer about an owner's PRs, for [`best_login`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub login: String,
    /// How many of the PRs it saw.
    pub seen: usize,
    /// The most rights it had on them ([`rank_of`]).
    pub rank: usize,
}

/// The login an owner is kept as, of those asked about its `total` PRs, the one that asked
/// first, first (`bestLogin`): one that saw every one of them, with more rights than any
/// before it; else the first.
#[must_use]
pub fn best_login(answers: &[Answer], total: usize) -> String {
    let Some((first, rest)) = answers.split_first() else {
        return String::new();
    };
    rest.iter()
        .fold(first, |best, a| {
            if a.seen == total && a.rank > best.rank {
                a
            } else {
                best
            }
        })
        .login
        .clone()
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
    fn an_owner_of_a_logins_name_goes_as_that_login_before_the_active_one() {
        let known = logins("octocat", &["hubot"]);
        assert_eq!(login_for(&BTreeMap::new(), "Hubot", &known), "hubot");
        // A login remembered for it still stands.
        let accounts: BTreeMap<String, String> = [("hubot".into(), "octocat".into())].into();
        assert_eq!(login_for(&accounts, "hubot", &known), "octocat");
    }

    #[test]
    fn an_owner_seen_only_to_read_with_nobody_remembered_is_asked_as_the_others() {
        let seen = [
            ("acme", Some("READ")),
            ("globex", Some("READ")),
            ("globex", Some("WRITE")),
            ("initech", None),
            ("umbrella", Some("READ")),
        ];
        let accounts: BTreeMap<String, String> = [("umbrella".into(), "hubot".into())].into();
        let known = logins("octocat", &["hubot"]);
        assert_eq!(
            owners_below_write(seen, &accounts, &known),
            ["acme", "initech"]
        );
        assert_eq!(
            owners_below_write(seen, &BTreeMap::new(), &logins("octocat", &[])),
            Vec::<String>::new()
        );
        assert!(rank_of(Some("ADMIN")) > rank_of(Some("WRITE")));
        assert_eq!(rank_of(None), 0);
        assert_eq!(rank_of(Some("OWNER")), 0);
    }

    #[test]
    fn an_owner_is_kept_as_a_login_that_sees_all_with_more_rights_else_the_first() {
        let answer = |login: &str, seen, permission| Answer {
            login: login.into(),
            seen,
            rank: rank_of(Some(permission)),
        };
        let asked = answer("octocat", 2, "READ");
        let best = |other| best_login(&[asked.clone(), other], 2);
        assert_eq!(best(answer("hubot", 2, "ADMIN")), "hubot");
        // Seeing fewer of them, more rights are no reason to move.
        assert_eq!(best(answer("hubot", 1, "ADMIN")), "octocat");
        assert_eq!(best(answer("hubot", 2, "READ")), "octocat");
        assert_eq!(best_login(&[], 0), "");
    }

    #[test]
    fn a_switch_moves_to_the_next_login_round_to_the_first() {
        let known = logins("octocat", &["hubot", "monalisa"]);
        assert_eq!(next_login(&known, "octocat").as_deref(), Some("hubot"));
        assert_eq!(next_login(&known, "monalisa").as_deref(), Some("octocat"));
        // A login no longer there: the first.
        assert_eq!(next_login(&known, "gone").as_deref(), Some("octocat"));
        assert_eq!(next_login(&logins("octocat", &[]), "octocat"), None);
    }

    #[test]
    fn a_fetch_hands_on_only_what_it_learned() {
        let before: BTreeMap<String, String> = [
            ("acme".into(), "hubot".into()),
            ("globex".into(), "octocat".into()),
        ]
        .into();
        let mut after = before.clone();
        after.insert("globex".into(), "hubot".into());
        after.insert("initech".into(), "octocat".into());
        assert_eq!(
            learned(&before, &after),
            [
                ("globex".into(), "hubot".into()),
                ("initech".into(), "octocat".into())
            ]
            .into()
        );
        assert_eq!(learned(&before, &before), BTreeMap::new());
    }

    proptest::proptest! {
        #[test]
        fn switching_as_often_as_there_are_logins_comes_back(
            others in proptest::collection::vec("[a-e]", 1..4),
        ) {
            let known = logins("z", &others.iter().map(String::as_str).collect::<Vec<_>>());
            let all = every_login(&known);
            let mut at = all[0].clone();
            let mut seen = vec![at.clone()];
            for _ in 1..all.len() {
                at = next_login(&known, &at).unwrap();
                seen.push(at.clone());
            }
            proptest::prop_assert_eq!(next_login(&known, &at), Some(all[0].clone()));
            seen.sort();
            let mut every = all.clone();
            every.sort();
            proptest::prop_assert_eq!(seen, every);
        }

        #[test]
        fn the_kept_login_is_the_first_or_one_that_sees_all_with_more(
            answers in proptest::collection::vec(("[a-c]", 0_usize..4, 0_usize..6), 1..5),
            total in 0_usize..4,
        ) {
            let answers: Vec<Answer> = answers
                .into_iter()
                .map(|(login, seen, rank)| Answer { login, seen, rank })
                .collect();
            let best = best_login(&answers, total);
            let first = &answers[0];
            proptest::prop_assert!(answers.iter().any(|a| a.login == best));
            if best != first.login {
                let moved = answers
                    .iter()
                    .any(|a| a.login == best && a.seen == total && a.rank > first.rank);
                proptest::prop_assert!(moved);
            }
        }
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
