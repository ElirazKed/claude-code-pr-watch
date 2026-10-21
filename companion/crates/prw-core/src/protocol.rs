//! The shared files under `~/.cache/pr-watch/` (`companion/PROTOCOL.md` is the spec):
//! `results.json` ([`Shared`]), `sessions/<id>.json` ([`SessionFile`]) and `lease.json`
//! ([`Lease`]).
//!
//! Parsing is the boundary. Text that isn't JSON, or isn't the object a file holds, is no
//! file this time (what a reader does then is the protocol's: keep the last good
//! `results.json`, list no URLs for a session, take the lease as free). Inside a file, a
//! field that is missing, `null` or of the wrong type is absent, as the mod's `empty()` and
//! spreads leave it; unknown fields are skipped, except `results.json`'s top-level ones,
//! which a writer must keep ([`Shared::extra`]).

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, DeserializeOwned};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::gh::GhPr;
use crate::lenient::{lenient, map_of, ms, ms_of, ms_opt, string_enum, strings};

/// `results.json`: what GitHub said about every watched PR, and the poll's own state.
///
/// [`Shared::default`] is the mod's `empty()`: what a reader has before anything was fetched.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Shared {
    /// When the last round ran (ms).
    pub fetched_at: i64,
    /// When the next round is due (ms).
    pub next_at: i64,
    /// By PR URL.
    pub prs: BTreeMap<String, Entry>,
    /// Repo owner → the gh login that sees its repos (a login, never a token).
    pub accounts: BTreeMap<String, String>,
    /// Legacy: what mods before per-login rates read.
    pub rate: Option<RateLimit>,
    /// By gh login.
    pub rates: BTreeMap<String, LoginRates>,
    /// pr-watch's newest version, as last asked.
    pub latest: Latest,
    /// A secondary rate limit holds everyone off GitHub until then (ms).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub held_until: Option<i64>,
    /// The top-level fields this version doesn't know, kept as they were so a write passes
    /// them on (the protocol's read-merge-write rule). Never holds a known field's name.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The fields [`Shared`] knows, as `results.json` names them.
const SHARED_FIELDS: [&str; 8] = [
    "fetchedAt",
    "nextAt",
    "prs",
    "accounts",
    "rate",
    "rates",
    "latest",
    "heldUntil",
];

impl Shared {
    /// `results.json`'s fields, each read leniently; the rest go to [`Shared::extra`].
    fn from_fields(mut fields: Map<String, Value>) -> Self {
        let mut take = |key: &str| fields.remove(key).unwrap_or(Value::Null);
        let fetched_at = ms_of(&take("fetchedAt")).unwrap_or(0);
        let next_at = ms_of(&take("nextAt")).unwrap_or(0);
        let prs = map_of(take("prs"));
        let accounts = map_of(take("accounts"));
        let rate = RateLimit::deserialize(take("rate")).ok();
        let rates = map_of(take("rates"));
        let latest = Latest::deserialize(take("latest")).unwrap_or_default();
        let held_until = ms_of(&take("heldUntil"));
        debug_assert!(SHARED_FIELDS.iter().all(|key| !fields.contains_key(*key)));

        Self {
            fetched_at,
            next_at,
            prs,
            accounts,
            rate,
            rates,
            latest,
            held_until,
            extra: fields,
        }
    }
}

impl<'de> Deserialize<'de> for Shared {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match Value::deserialize(d)? {
            Value::Object(fields) => Ok(Self::from_fields(fields)),
            _ => Err(de::Error::custom(ParseError::NotAnObject)),
        }
    }
}

/// Why `results.json` didn't read.
#[derive(Debug)]
pub enum ParseError {
    /// Not JSON: empty, or cut off mid-write by a writer that writes in place.
    Json(serde_json::Error),
    /// JSON, but not an object.
    NotAnObject,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(f, "results.json is not JSON: {error}"),
            Self::NotAnObject => f.write_str("results.json is not an object"),
        }
    }
}

impl std::error::Error for ParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::NotAnObject => None,
        }
    }
}

/// `results.json`'s text as [`Shared`]. Total: any text gives a value or an error, never a
/// panic. An error means "keep the last good copy".
///
/// # Errors
///
/// [`ParseError::Json`] where the text isn't JSON (a torn or empty file), and
/// [`ParseError::NotAnObject`] where it is JSON but not an object.
pub fn parse_shared(text: &str) -> Result<Shared, ParseError> {
    match serde_json::from_str(text).map_err(ParseError::Json)? {
        Value::Object(fields) => Ok(Shared::from_fields(fields)),
        _ => Err(ParseError::NotAnObject),
    }
}

/// What `results.json` keeps of one PR (`hooks/cache.ts`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Entry {
    /// When `pr` was fetched; with no `pr`, when the fetch last failed (ms).
    #[serde(deserialize_with = "ms")]
    pub at: i64,
    /// The PR as the batched query returned it; absent where it was never read.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub pr: Option<GhPr>,
    /// What the card says, once it says anything.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The first failure in a row while `pr` still stands (ms).
    #[serde(deserialize_with = "ms_opt", skip_serializing_if = "Option::is_none")]
    pub failed_since: Option<i64>,
    /// Never read: no logged-in gh account could see it. Written only when true.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "is_false")]
    pub is_unreachable: bool,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes a reference"
)]
const fn is_false(value: &bool) -> bool {
    !*value
}

/// The legacy rate field: GraphQL's limit as a reply's `rateLimit` gave it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimit {
    pub remaining: i64,
    /// ISO 8601.
    pub reset_at: String,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub limit: Option<i64>,
}

/// One gh login's buckets, and when `/rate_limit` was last asked for it (`LoginRates`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoginRates {
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub graphql: Option<Bucket>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub core: Option<Bucket>,
    #[serde(deserialize_with = "ms_opt", skip_serializing_if = "Option::is_none")]
    pub asked_at: Option<i64>,
}

/// An hourly budget as last seen (`Bucket`). Times in ms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    pub limit: i64,
    pub remaining: i64,
    pub reset_at: i64,
    pub seen_at: i64,
}

/// pr-watch's newest version as the last check found it, and when that check rode a round.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Latest {
    /// `None`: none read yet.
    #[serde(deserialize_with = "lenient")]
    pub version: Option<String>,
    #[serde(deserialize_with = "ms")]
    pub checked_at: i64,
}

string_enum! {
    /// Who wrote a session file. A file with no `writer` is an older mod's.
    pub enum Writer {
        Mod = "mod",
        Prw = "prw",
    }
}

/// `sessions/<id>.json`: one session's watch list, and who and where it is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionFile {
    /// PR URLs: only the strings of the file's `urls` list.
    #[serde(deserialize_with = "strings")]
    pub urls: Vec<String>,
    /// Absent: an older mod (before 0.14.1).
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub writer: Option<Writer>,
    /// The agent the session runs: `claude` for the mod; `codex`, … through `prw`.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// What a list of sessions calls it: its project root's folder name.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The directory the session runs in, absolute.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// A session file's text. `None` where it isn't a JSON object: torn or empty mid-write, so
/// this session lists no URLs this time (its liveness still goes by the file's mtime).
#[must_use]
pub fn parse_session(text: &str) -> Option<SessionFile> {
    object(text)
}

/// The PR URLs a session file lists (`urlsIn`): only strings from an array `urls`, whatever
/// else is there; none where the file doesn't parse.
#[must_use]
pub fn urls_in(text: &str) -> Vec<String> {
    parse_session(text)
        .map(|file| file.urls)
        .unwrap_or_default()
}

/// `lease.json`: whose turn it is to poll, and until when (ms).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lease {
    /// A session file's `<id>`.
    #[serde(deserialize_with = "lenient")]
    pub owner: String,
    #[serde(deserialize_with = "ms")]
    pub until: i64,
}

/// `lease.json`'s text. `None` where it isn't a JSON object: torn, empty or missing reads as
/// no lease held.
#[must_use]
pub fn parse_lease(text: &str) -> Option<Lease> {
    object(text)
}

/// A file that must be a JSON object, its fields read leniently.
fn object<T: DeserializeOwned>(text: &str) -> Option<T> {
    match serde_json::from_str(text).ok()? {
        value @ Value::Object(_) => T::deserialize(value).ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_empty_object_is_the_mods_empty() {
        let shared = parse_shared("{}").unwrap();
        assert_eq!(shared, Shared::default());
        assert_eq!(
            serde_json::to_value(&shared).unwrap(),
            json!({"fetchedAt": 0, "nextAt": 0, "prs": {}, "accounts": {}, "rate": null, "rates": {}, "latest": {"version": null, "checkedAt": 0}})
        );
    }

    #[test]
    fn torn_or_not_an_object_is_an_error() {
        assert!(matches!(parse_shared(""), Err(ParseError::Json(_))));
        assert!(matches!(
            parse_shared(r#"{"fetchedAt": 12, "pr"#),
            Err(ParseError::Json(_))
        ));
        assert!(matches!(parse_shared("[]"), Err(ParseError::NotAnObject)));
        assert!(matches!(parse_shared("null"), Err(ParseError::NotAnObject)));
    }

    #[test]
    fn unknown_top_level_fields_are_kept_for_the_write() {
        let shared = parse_shared(r#"{"nextAt": 5, "future": {"a": [1]}}"#).unwrap();
        assert_eq!(shared.next_at, 5);
        assert_eq!(shared.extra.get("future"), Some(&json!({"a": [1]})));
        assert_eq!(
            serde_json::to_value(&shared).unwrap()["future"],
            json!({"a": [1]})
        );
    }

    #[test]
    fn a_bad_field_is_absent_and_the_rest_still_reads() {
        let shared = parse_shared(
            r#"{"fetchedAt": "soon", "nextAt": 1.5, "heldUntil": 99, "rate": {"remaining": 1},
                "prs": {"a": {"at": 3, "pr": 7}, "b": 4, "c": {"at": 2, "isUnreachable": true, "error": "no"}},
                "accounts": {"acme": "octocat", "bad": 1}, "rates": {"octocat": {"askedAt": 8, "core": {"limit": 1}}}}"#,
        )
        .unwrap();
        assert_eq!(shared.fetched_at, 0);
        assert_eq!(shared.next_at, 2);
        assert_eq!(shared.held_until, Some(99));
        assert_eq!(shared.rate, None);
        assert_eq!(shared.prs.len(), 2);
        assert_eq!(
            shared.prs["a"],
            Entry {
                at: 3,
                ..Entry::default()
            }
        );
        assert!(shared.prs["c"].is_unreachable);
        assert_eq!(shared.accounts.len(), 1);
        assert_eq!(
            shared.rates["octocat"],
            LoginRates {
                asked_at: Some(8),
                ..LoginRates::default()
            }
        );
    }

    #[test]
    fn session_files_read_as_urls_in_does() {
        assert_eq!(
            urls_in(r#"{"urls": ["a", 1, null, "b"], "writer": "mod"}"#),
            ["a", "b"]
        );
        assert_eq!(urls_in(r#"{"urls": "a"}"#), Vec::<String>::new());
        assert_eq!(urls_in(r#"{"urls": ["a""#), Vec::<String>::new());
        assert_eq!(urls_in(r#"["a"]"#), Vec::<String>::new());
        let file = parse_session(
            r#"{"urls": [], "writer": "codex-bridge", "agent": 5, "cwd": "/work/app"}"#,
        )
        .unwrap();
        assert_eq!(file.writer, Some(Writer::Other("codex-bridge".into())));
        assert_eq!(file.agent, None);
        assert_eq!(file.cwd.as_deref(), Some("/work/app"));
        assert_eq!(parse_session("{}"), Some(SessionFile::default()));
    }

    #[test]
    fn a_session_file_writes_as_the_protocol_shows_it() {
        let file = SessionFile {
            urls: vec!["https://github.com/acme/app/pull/12".into()],
            writer: Some(Writer::Mod),
            agent: Some("claude".into()),
            name: Some("app".into()),
            cwd: Some("/work/app/hooks".into()),
        };
        let text = r#"{"urls":["https://github.com/acme/app/pull/12"],"writer":"mod","agent":"claude","name":"app","cwd":"/work/app/hooks"}"#;
        assert_eq!(serde_json::to_string(&file).unwrap(), text);
        assert_eq!(parse_session(text), Some(file));
    }

    #[test]
    fn a_torn_lease_is_no_lease() {
        assert_eq!(
            parse_lease(r#"{"owner":"a","until":5}"#),
            Some(Lease {
                owner: "a".into(),
                until: 5
            })
        );
        assert_eq!(parse_lease(r#"{"owner":"a","unt"#), None);
        assert_eq!(parse_lease(""), None);
        assert_eq!(parse_lease("null"), None);
        let lease = Lease {
            owner: "prw-1".into(),
            until: 0,
        };
        assert_eq!(
            parse_lease(&serde_json::to_string(&lease).unwrap()),
            Some(lease)
        );
    }
}
