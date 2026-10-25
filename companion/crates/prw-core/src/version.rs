//! Which `prw` this is, and whether a newer one is out. `prw` has its own version, apart from
//! the mod's: at most every [`CHECK_MS`] it reads `companion/Cargo.toml` on the repo's default
//! branch (one small call, any gh login: the repo is public), as the mod reads its
//! `plugin.json` (`hooks/update.ts`).

/// This `prw`'s version.
pub const OWN: &str = env!("CARGO_PKG_VERSION");

/// How often to ask for the newest version (the mod's `CHECK_MS`).
pub const CHECK_MS: i64 = 2 * 3_600_000;

/// How soon to ask again when GitHub gave no answer.
pub const RETRY_MS: i64 = 10 * 60_000;

/// How long the ask may take: it holds up the poller's tick, so less than a round's call (slow
/// networks still get an answer).
pub const ASK_MS: i64 = 10_000;

/// What `gh api` asks: the companion's `Cargo.toml` on the default branch, as raw text.
pub const ASK: [&str; 4] = [
    "api",
    "-H",
    "Accept: application/vnd.github.raw+json",
    "repos/ElirazKed/claude-code-pr-watch/contents/companion/Cargo.toml",
];

/// A version in semver's order: major, minor, patch, then a release after its pre-releases
/// (`1.0.0-rc.1` < `1.0.0`), and pre-releases by their dot-separated parts.
type Version = (u64, u64, u64, bool, Vec<Part>);

/// A pre-release's part: a number goes by its value, and before any word (semver's rule).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Part {
    Number(u64),
    Word(String),
}

/// `major.minor.patch`, with a `-pre.release` or not; anything else (build metadata, a `v`,
/// a leading zero, a typo) is never compared.
fn parse(text: &str) -> Option<Version> {
    let text = text.trim();
    let (core, pre) = match text.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (text, None),
    };
    let is_digits = |n: &str| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit());
    // semver's numbers: no leading zero, and one too big for a u64 isn't read as a word.
    let number = |n: &str| {
        let is_number = is_digits(n) && (n == "0" || !n.starts_with('0'));
        is_number.then(|| n.parse().ok()).flatten()
    };
    let mut parts = core.split('.').map(number);
    let (major, minor, patch) = (parts.next()??, parts.next()??, parts.next()??);
    if parts.next().is_some() {
        return None;
    }
    let pre = match pre {
        None => Vec::new(),
        Some(pre) => pre
            .split('.')
            .map(|part| {
                let is_word = !is_digits(part)
                    && !part.is_empty()
                    && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
                number(part)
                    .map(Part::Number)
                    .or_else(|| is_word.then(|| Part::Word(part.to_owned())))
            })
            .collect::<Option<_>>()?,
    };
    Some((major, minor, patch, pre.is_empty(), pre))
}

/// Whether to ask at `now`, after the last ask (when, and whether GitHub answered): every
/// [`CHECK_MS`], sooner ([`RETRY_MS`]) after no answer, and at once after the clock went back.
#[must_use]
pub fn is_due(last: Option<(i64, bool)>, now: i64) -> bool {
    last.is_none_or(|(at, is_answered)| {
        let wait = if is_answered { CHECK_MS } else { RETRY_MS };
        now < at || now - at >= wait
    })
}

/// The workspace's version in a `Cargo.toml`'s text; `None` when it has none to compare.
#[must_use]
pub fn version_in(cargo_toml: &str) -> Option<String> {
    let table: toml::Table = toml::from_str(cargo_toml).ok()?;
    let version = table
        .get("workspace")?
        .get("package")?
        .get("version")?
        .as_str()?
        .trim();
    parse(version).map(|_| version.to_owned())
}

/// Whether `latest` is a later version than `own`; false when either won't parse.
#[must_use]
pub fn is_newer(latest: &str, own: &str) -> bool {
    matches!((parse(latest), parse(own)), (Some(a), Some(b)) if a > b)
}

/// Whether to tell someone on `own` that `latest` is out: it's newer, and a pre-release only
/// to someone already on one (as Cargo and npm offer them).
#[must_use]
pub fn is_update(latest: &str, own: &str) -> bool {
    let is_pre = |v: &str| parse(v).is_some_and(|(.., is_release, _)| !is_release);
    is_newer(latest, own) && (!is_pre(latest) || is_pre(own))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn the_workspace_version_reads_and_anything_else_doesnt() {
        let ours = include_str!("../../../Cargo.toml");
        assert_eq!(version_in(ours).as_deref(), Some(OWN));
        let at = |v: &str| format!("[workspace.package]\nversion = \"{v}\"\n");
        assert_eq!(version_in(&at("1.2.3")).as_deref(), Some("1.2.3"));
        assert_eq!(version_in(&at("1.0.0-rc.1")).as_deref(), Some("1.0.0-rc.1"));
        for odd in [
            "1.2",
            "1.2.3-",
            "1.2.3-rc..1",
            "1.2.3+build",
            "1.2.3-rc.01",
            "01.2.3",
            "1.2.3-99999999999999999999",
            "v1.2.3",
            "1..3",
            "",
        ] {
            assert_eq!(version_in(&at(odd)), None, "{odd}");
        }
        assert_eq!(version_in("[package]\nversion = \"1.2.3\"\n"), None);
        assert_eq!(version_in("not toml ["), None);
    }

    #[test]
    fn newer_goes_by_major_then_minor_then_patch() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("garbage", "0.1.0"));
        assert!(!is_newer("1.0.0", "garbage"));
    }

    #[test]
    fn a_release_is_newer_than_its_pre_releases_and_those_go_by_their_parts() {
        assert!(is_newer("1.0.0", "1.0.0-rc.1"));
        assert!(!is_newer("1.0.0-rc.1", "1.0.0"));
        assert!(is_newer("1.0.0-rc.1", "0.6.0"));
        // Told: a release, or a later pre-release to one already on a pre-release.
        assert!(!is_update("1.0.0-rc.1", "0.6.0"));
        assert!(is_update("1.0.0", "1.0.0-rc.1"));
        assert!(is_update("1.0.0-rc.2", "1.0.0-rc.1"));
        assert!(is_update("0.7.0", "0.6.0"));
        // semver's own example order.
        let order = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
        ];
        for pair in order.windows(2) {
            assert!(is_newer(pair[1], pair[0]), "{} after {}", pair[1], pair[0]);
        }
    }

    #[test]
    fn an_answer_waits_two_hours_and_no_answer_ten_minutes() {
        assert!(is_due(None, 0));
        assert!(!is_due(Some((0, true)), CHECK_MS - 1));
        assert!(is_due(Some((0, true)), CHECK_MS));
        assert!(!is_due(Some((0, false)), RETRY_MS - 1));
        assert!(is_due(Some((0, false)), RETRY_MS));
        assert!(is_due(Some((5_000, true)), 4_999));
    }

    proptest! {
        /// Once due, still due later on.
        #[test]
        fn due_stays_due(at in 0..10 * CHECK_MS, is_answered: bool, now in 0..10 * CHECK_MS, later in 0..CHECK_MS) {
            let last = Some((at, is_answered));
            prop_assert!(!is_due(last, now) || now < at || is_due(last, now + later));
        }

        #[test]
        fn newer_is_a_strict_order(
            a in (0..3u64, 0..3u64, 0..3u64, prop::option::of("(rc|beta|0|1|11)(\\.(rc|beta|0|1|11)){0,2}")),
            b in (0..3u64, 0..3u64, 0..3u64, prop::option::of("(rc|beta|0|1|11)(\\.(rc|beta|0|1|11)){0,2}")),
        ) {
            let text = |(x, y, z, pre): (u64, u64, u64, Option<String>)| match pre {
                Some(pre) => format!("{x}.{y}.{z}-{pre}"),
                None => format!("{x}.{y}.{z}"),
            };
            let (a, b) = (text(a), text(b));
            prop_assert!(parse(&a).is_some() && parse(&b).is_some());
            prop_assert!(!is_newer(&a, &a));
            prop_assert!(!(is_newer(&a, &b) && is_newer(&b, &a)));
            prop_assert_eq!(a == b, !is_newer(&a, &b) && !is_newer(&b, &a));
        }
    }
}
