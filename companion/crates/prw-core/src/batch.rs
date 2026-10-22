//! One GraphQL query for many PRs (`hooks/batch.ts`): an aliased field per PR, so a round
//! costs one call per 20 PRs whatever the count, plus the rate limit it left; what GitHub said
//! about a call (`gh api graphql -i`'s status and headers); why a call failed, as far as asking
//! again goes; a round's time; and the failure log.
//!
//! A PR comes back as raw JSON in the shape `parseReply` stores it (`GhPr`), never through
//! [`GhPr`](crate::gh::GhPr): a writer passes on every field GitHub sent, known here or not.

use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::lenient::read;
use crate::protocol::RateLimit;
use crate::time::format_iso;

/// 20 PRs a query, run one after another: a query of 40 busy PRs can outlast GitHub's own
/// timeout (a 502 or 504) and lose the whole round, while two of 20 cost the same points.
pub const BATCH_SIZE: usize = 20;

/// A PR by its URL: `https://github.com/<owner>/<repo>/pull/<n>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PrRef {
    pub url: String,
    /// `owner/repo`.
    pub repo: String,
    pub number: u64,
}

impl PrRef {
    /// The repo's owner: the part before the `/`.
    #[must_use]
    pub fn owner(&self) -> &str {
        self.repo.split('/').next().unwrap_or_default()
    }
}

static PR_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^https://github\.com/([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)/pull/([0-9]+)$")
        .expect("a valid pattern")
});

/// The PR a URL names (`refFromUrl`); `None` where it isn't a github.com PR URL.
#[must_use]
pub fn ref_from_url(url: &str) -> Option<PrRef> {
    let m = PR_URL.captures(url)?;
    Some(PrRef {
        url: url.to_owned(),
        repo: m[1].to_owned(),
        number: m[2].parse().ok()?,
    })
}

/// The head commit's checks, with whatever more a caller asks of each kind, and of the
/// commit.
fn checks_of(run: &str, status: &str, commit: &str) -> String {
    format!(
        "commits(last: 1) {{ nodes {{ commit {{ statusCheckRollup {{ contexts(first: 100) {{ nodes {{
    __typename
    ... on CheckRun {{ name status conclusion startedAt {run}checkSuite {{ workflowRun {{ workflow {{ name }} }} }} }}
    ... on StatusContext {{ context state createdAt{status} }}
  }} }} }}{commit} }} }} }}"
    )
}

/// Who reviewed and the open threads: small pages, logins and states, and of the newest
/// threads only whether they are resolved or outdated, never a comment's text.
const REVIEWS: &str = "latestReviews(first: 10) { totalCount nodes { author { __typename login } state submittedAt } }
  latestOpinionatedReviews(first: 10) { nodes { author { __typename login } state submittedAt } }
  reviewRequests(first: 10) { totalCount nodes { requestedReviewer {
    __typename ... on User { login } ... on Bot { login } ... on Mannequin { login } ... on Team { slug }
  } } }
  reviewThreads(last: 50) { totalCount nodes { isResolved isOutdated } }";

fn fragment() -> String {
    format!(
        "fragment pr on PullRequest {{
  number title url state isDraft reviewDecision mergeStateStatus mergeable mergedAt closedAt
  additions deletions headRefName baseRefName author {{ login }} autoMergeRequest {{ enabledAt mergeMethod }}
  viewerCanEnableAutoMerge viewerCanDisableAutoMerge
  {REVIEWS}
  {}
}}
fragment repo on Repository {{
  squashMergeAllowed rebaseMergeAllowed mergeCommitAllowed viewerDefaultMergeMethod viewerPermission
}}",
        checks_of("", "", "")
    )
}

/// The query for `refs` (`buildQuery`), the rate limit first. Never with the mod's newest
/// version field: that check is the mod's own notice.
#[must_use]
pub fn build_query(refs: &[PrRef]) -> String {
    let fields: Vec<String> = refs
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut parts = r.repo.split('/');
            let owner = parts.next().unwrap_or_default();
            let name = parts.next().unwrap_or_default();
            format!(
                "p{i}: repository(owner: {}, name: {}) {{ pullRequest(number: {}) {{ ...pr }} ...repo }}",
                json_string(owner),
                json_string(name),
                r.number
            )
        })
        .collect();
    format!(
        "query {{\n  rateLimit {{ limit remaining resetAt }}\n  {}\n}}\n{}",
        fields.join("\n  "),
        fragment()
    )
}

/// A string as `JSON.stringify` writes it.
fn json_string(text: &str) -> String {
    Value::String(text.to_owned()).to_string()
}

/// What one batched call said (`BatchResult`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BatchReply {
    /// By URL: each PR as `parseReply` stores it.
    pub found: BTreeMap<String, Map<String, Value>>,
    /// Not visible to this token: another account may see them.
    pub missing: Vec<PrRef>,
    pub rate: Option<RateLimit>,
    /// The whole call failed (network, auth, a limit): nothing in it is known.
    pub error: Option<String>,
}

/// The first line of `text`, trimmed.
pub(crate) fn first_line(text: &str) -> &str {
    text.trim().lines().next().unwrap_or_default()
}

/// `gh api graphql`'s reply (`parseReply`). gh exits 1 when any alias errs, yet still prints
/// every alias it resolved, so the exit code says nothing here.
#[must_use]
pub fn parse_reply(stdout: &str, stderr: &str, refs: &[PrRef]) -> BatchReply {
    let mut out = BatchReply::default();
    let Ok(reply) = serde_json::from_str::<Value>(stdout) else {
        out.error = Some(or(first_line(stderr), "gh api graphql failed"));
        return out;
    };
    let Some(data) = reply.get("data").and_then(Value::as_object) else {
        let said = reply
            .pointer("/errors/0/message")
            .and_then(Value::as_str)
            .map(str::to_owned);
        out.error = Some(said.unwrap_or_else(|| or(first_line(stderr), "GitHub returned no data")));
        return out;
    };
    out.rate = data.get("rateLimit").cloned().and_then(read);
    for (i, r) in refs.iter().enumerate() {
        let mut repository = match data.get(&format!("p{i}")) {
            Some(Value::Object(repo)) => repo.clone(),
            _ => Map::new(),
        };
        match repository.remove("pullRequest") {
            Some(Value::Object(pr)) => {
                out.found.insert(r.url.clone(), flatten(pr, repository));
            }
            _ => out.missing.push(r.clone()),
        }
    }
    out
}

/// `text`, or `fallback` where it is empty (JS `text || fallback`).
pub(crate) fn or(text: &str, fallback: &str) -> String {
    if text.is_empty() { fallback } else { text }.to_owned()
}

/// A PR as `gh pr view` gives it: the head commit's checks lifted into `statusCheckRollup`
/// (each run's workflow beside it), the repository's settings under `repository`.
fn flatten(mut pr: Map<String, Value>, repository: Map<String, Value>) -> Map<String, Value> {
    let commit = pr
        .remove("commits")
        .and_then(|c| c.pointer("/nodes/0/commit").cloned())
        .unwrap_or(Value::Null);
    let nodes = commit
        .pointer("/statusCheckRollup/contexts/nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // A failed run's title and summary, where they were asked apart (`checksQuery`).
    let about: HashMap<i64, &Value> = commit
        .pointer("/checkSuites/nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|suite| suite.pointer("/checkRuns/nodes").and_then(Value::as_array))
        .flatten()
        .filter_map(|run| Some((run.get("databaseId")?.as_i64()?, run)))
        .collect();
    let checks = nodes.into_iter().map(|node| check(node, &about)).collect();
    pr.insert("repository".into(), Value::Object(repository));
    pr.insert("statusCheckRollup".into(), Value::Array(checks));
    pr
}

/// One check as stored (`flattenCheck`): its workflow's name lifted out of its suite, so two
/// workflows' same-named jobs stay apart, and its title and summary where they were asked.
fn check(node: Value, about: &HashMap<i64, &Value>) -> Value {
    let Value::Object(mut check) = node else {
        return node;
    };
    let suite = check.remove("checkSuite");
    if let Some(name) = suite
        .as_ref()
        .and_then(|s| s.get("workflowRun"))
        .filter(|run| !run.is_null())
        .and_then(|run| run.get("workflow"))
        .filter(|workflow| !workflow.is_null())
        .and_then(|workflow| workflow.get("name"))
    {
        check.insert("workflow".into(), name.clone());
    }
    if let Some(text) = check
        .get("databaseId")
        .and_then(Value::as_i64)
        .and_then(|id| about.get(&id))
    {
        let field = |key: &str| text.get(key).cloned().unwrap_or(Value::Null);
        check.insert("title".into(), field("title"));
        check.insert("summary".into(), field("summary"));
    }
    Value::Object(check)
}

// ---- What GitHub said about a call ---------------------------------------------------------

/// `gh api graphql -i`'s stdout taken apart (`Http`): the status line, the headers (keyed in
/// lower case), and the body. With no status line, all of stdout is the body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Http {
    pub status: Option<u16>,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

static HEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^HTTP/[0-9.]+ ([0-9]{3})[^\n]*\n((?:[^\r\n]+\r?\n)*)\r?\n")
        .expect("a valid pattern")
});

/// `stdout` as status, headers and body (`parseHttp`).
#[must_use]
pub fn parse_http(stdout: &str) -> Http {
    let Some(head) = HEAD.captures(stdout) else {
        return Http {
            body: stdout.to_owned(),
            ..Http::default()
        };
    };
    let headers = head[2]
        .lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_lowercase(), value.trim().to_owned()))
        .collect();
    Http {
        status: head[1].parse().ok(),
        headers,
        body: stdout[head[0].len()..].to_owned(),
    }
}

/// A header's value as JS `Number` reads it: blank is 0, and anything but a finite number is
/// none.
fn number(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return Some(0.0);
    }
    text.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// The primary limit as the headers give it, failed calls included (`rateOf`); `None` where
/// they don't, or where they speak of a limit other than GraphQL's.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "GitHub's counts and reset times are whole numbers far inside i64"
)]
pub fn rate_of(headers: &BTreeMap<String, String>) -> Option<RateLimit> {
    let header = |name: &str| headers.get(name).map(String::as_str);
    let remaining = number(header("x-ratelimit-remaining")?)?;
    let reset = number(header("x-ratelimit-reset")?)?;
    if header("x-ratelimit-resource").is_some_and(|r| r != "graphql") {
        return None;
    }
    Some(RateLimit {
        remaining: remaining as i64,
        reset_at: format_iso((reset * 1_000.0).trunc() as i64)?,
        limit: header("x-ratelimit-limit")
            .and_then(number)
            .map(|limit| limit as i64),
    })
}

/// Seconds GitHub asks to wait (`retry-after`), or `None` where it doesn't say
/// (`retryAfterOf`). An empty header names no wait.
#[must_use]
pub fn retry_after_of(headers: &BTreeMap<String, String>) -> Option<f64> {
    let said = headers.get("retry-after")?.trim();
    if said.is_empty() {
        return None;
    }
    number(said).filter(|seconds| *seconds >= 0.0)
}

/// Why a whole call failed, as far as asking again goes (`CallFailure`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CallFailure {
    /// GitHub having a moment (a gateway's 5xx, GraphQL's own timeout, a cut connection):
    /// asked once more.
    Timeout,
    /// GitHub's limits on bursts: never asked again at once; everyone holds off.
    SecondaryLimit,
    /// The hourly points are spent: this login waits for its reset.
    PrimaryLimit,
    /// A refusal and the rest: asking again won't help.
    Other,
}

static SECONDARY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)secondary rate limit|abuse").expect("a valid pattern"));
static PRIMARY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)rate limit").expect("a valid pattern"));
static GATEWAY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?-u:\b)HTTP 50[234](?-u:\b)").expect("a valid pattern"));
static CUT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?-u:\b)(timeout|EOF|connection reset|broken pipe|stream error|GOAWAY|deadline exceeded)(?-u:\b)|request in time")
        .expect("a valid pattern")
});

/// Why a call that failed with `error` failed (`failureOf`).
#[must_use]
pub fn failure_of(error: &str, http: &Http) -> CallFailure {
    if retry_after_of(&http.headers).is_some() || SECONDARY.is_match(error) {
        return CallFailure::SecondaryLimit;
    }
    if http
        .headers
        .get("x-ratelimit-remaining")
        .map(String::as_str)
        == Some("0")
        || PRIMARY.is_match(error)
    {
        return CallFailure::PrimaryLimit;
    }
    let is_gateway = matches!(http.status, Some(502..=504)) || GATEWAY.is_match(error);
    if is_gateway || CUT.is_match(error) {
        CallFailure::Timeout
    } else {
        CallFailure::Other
    }
}

// ---- A round's time ------------------------------------------------------------------------

/// A call starts only with this long left before the lease's end (ms): a batch has answered
/// in 2.5 s, and a gateway that gives up takes 10.
pub const MIN_CALL_MS: i64 = 5_000;

/// How long a call may take (`callBudget`): what is left before `until`, never more than
/// `max`. `None`: no call starts; what it would have asked waits for the next round.
#[must_use]
pub fn call_budget(until: i64, now: i64, max: i64) -> Option<i64> {
    let left = until.saturating_sub(now);
    (left >= MIN_CALL_MS).then(|| left.min(max))
}

// ---- The failure log -----------------------------------------------------------------------

/// `errors.log` keeps its last this many lines.
pub const LOG_LINES: usize = 200;

/// One failed call, for the log (`CallLog`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallLog<'a> {
    pub login: &'a str,
    pub size: usize,
    pub ms: i64,
    pub http: &'a Http,
    pub failure: CallFailure,
    pub error: &'a str,
}

/// A log line's fields, in the mod's order.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Line<'a> {
    at: Option<String>,
    login: &'a str,
    batch: usize,
    ms: i64,
    status: Option<u16>,
    class: CallFailure,
    remaining: Option<&'a str>,
    used: Option<&'a str>,
    resource: Option<&'a str>,
    retry_after: Option<&'a str>,
    error: String,
}

/// One JSON line for a failed call at `at` (`logLine`): when, who, how big, how long, and
/// what GitHub said. The error is cut to 200 characters.
#[must_use]
pub fn log_line(at: i64, call: &CallLog<'_>) -> String {
    let header = |name: &str| call.http.headers.get(name).map(String::as_str);
    let line = Line {
        at: format_iso(at),
        login: call.login,
        batch: call.size,
        ms: call.ms,
        status: call.http.status,
        class: call.failure,
        remaining: header("x-ratelimit-remaining"),
        used: header("x-ratelimit-used"),
        resource: header("x-ratelimit-resource"),
        retry_after: header("retry-after"),
        error: call.error.chars().take(200).collect(),
    };
    // Plain strings and numbers: serializing never fails.
    serde_json::to_string(&line).unwrap_or_default()
}

/// `log` with `line` added, cut to its last `max` lines (`appendLog`).
#[must_use]
pub fn append_log(log: &str, line: &str, max: usize) -> String {
    let lines: Vec<&str> = log
        .split('\n')
        .filter(|l| !l.is_empty())
        .chain([line])
        .collect();
    let kept = &lines[lines.len().saturating_sub(max)..];
    format!("{}\n", kept.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pr_ref(url: &str) -> PrRef {
        ref_from_url(url).unwrap()
    }

    fn a() -> PrRef {
        pr_ref("https://github.com/acme/app/pull/7")
    }

    fn b() -> PrRef {
        pr_ref("https://github.com/secret/app/pull/1")
    }

    #[test]
    fn urls_name_their_pr() {
        assert_eq!(
            a(),
            PrRef {
                url: "https://github.com/acme/app/pull/7".into(),
                repo: "acme/app".into(),
                number: 7
            }
        );
        assert_eq!(a().owner(), "acme");
        for url in [
            "https://github.com/acme/app/pull/7/files",
            "http://github.com/acme/app/pull/7",
            "https://github.com/acme/pull/7",
            "https://github.com/acme/app/issues/7",
        ] {
            assert_eq!(ref_from_url(url), None, "{url}");
        }
    }

    #[test]
    fn one_aliased_field_per_pr_and_the_rate_limit() {
        let query = build_query(&[a(), b()]);
        assert!(query.contains(
            r#"p0: repository(owner: "acme", name: "app") { pullRequest(number: 7) { ...pr } ...repo }"#
        ));
        assert!(query.contains(r#"p1: repository(owner: "secret", name: "app")"#));
        assert!(query.contains("rateLimit { limit remaining resetAt }"));
        assert!(!query.contains("pw:"));
    }

    #[test]
    fn asks_who_reviewed_and_whether_threads_are_resolved_never_what_anyone_wrote() {
        let query = build_query(&[a()]);
        for field in [
            "latestReviews(first: 10)",
            "latestOpinionatedReviews(first: 10)",
            "reviewRequests(first: 10)",
            "reviewThreads(last: 50) { totalCount nodes { isResolved isOutdated } }",
        ] {
            assert!(query.contains(field), "{field}");
        }
        let words = Regex::new(r"\b(body|bodyText|comments)\b").unwrap();
        assert!(!words.is_match(&query));
    }

    #[test]
    fn the_review_facts_come_through_as_github_sent_them() {
        let threads =
            json!({"totalCount": 1, "nodes": [{"isResolved": false, "isOutdated": true}]});
        let reviews = json!({"totalCount": 1, "nodes": [{"author": {"__typename": "User", "login": "nadav"}, "state": "COMMENTED", "submittedAt": "2026-10-07T10:00:00Z"}]});
        let stdout = json!({"data": {"p0": {"pullRequest": {"number": 7, "latestReviews": reviews, "reviewThreads": threads, "commits": {"nodes": []}}}}});
        let reply = parse_reply(&stdout.to_string(), "", &[a()]);
        let pr = &reply.found[&a().url];
        assert_eq!(pr["latestReviews"], reviews);
        assert_eq!(pr["reviewThreads"], threads);
    }

    #[test]
    fn a_partial_reply_found_prs_flattened_like_gh_pr_view_the_rest_missing() {
        let stdout = json!({
            "data": {
                "rateLimit": {"remaining": 4961, "resetAt": "2026-10-07T11:01:16Z"},
                "p0": {
                    "squashMergeAllowed": true,
                    "viewerPermission": "WRITE",
                    "pullRequest": {
                        "number": 7,
                        "state": "OPEN",
                        "commits": {"nodes": [{"commit": {"statusCheckRollup": {"contexts": {"nodes": [{"__typename": "CheckRun", "name": "build", "status": "IN_PROGRESS"}]}}}}]}
                    }
                },
                "p1": null
            },
            "errors": [{"type": "NOT_FOUND", "path": ["p1"], "message": "Could not resolve to a Repository with the name 'secret/app'."}]
        });
        let reply = parse_reply(&stdout.to_string(), "gh: Could not resolve…", &[a(), b()]);
        assert_eq!(reply.error, None);
        let pr = &reply.found[&a().url];
        assert_eq!(
            pr["statusCheckRollup"],
            json!([{"__typename": "CheckRun", "name": "build", "status": "IN_PROGRESS"}])
        );
        assert_eq!(
            pr["repository"],
            json!({"squashMergeAllowed": true, "viewerPermission": "WRITE"})
        );
        assert!(!pr.contains_key("commits"));
        assert_eq!(reply.missing, [b()]);
        assert_eq!(reply.rate.map(|r| r.remaining), Some(4961));
    }

    #[test]
    fn a_runs_workflow_is_lifted_out_of_its_suite_and_its_summary_joined_by_id() {
        let stdout = json!({"data": {"p0": {"pullRequest": {"commits": {"nodes": [{"commit": {
            "statusCheckRollup": {"contexts": {"nodes": [
                {"__typename": "CheckRun", "name": "test", "databaseId": 9, "checkSuite": {"workflowRun": {"workflow": {"name": "CI"}}}},
                {"__typename": "CheckRun", "name": "lint", "checkSuite": {"workflowRun": null}},
                {"__typename": "StatusContext", "context": "deploy"}
            ]}},
            "checkSuites": {"nodes": [{"checkRuns": {"nodes": [{"databaseId": 9, "title": "2 failed", "summary": null}]}}, null]}
        }}]}}}}});
        let reply = parse_reply(&stdout.to_string(), "", &[a()]);
        assert_eq!(
            reply.found[&a().url]["statusCheckRollup"],
            json!([
                {"__typename": "CheckRun", "name": "test", "databaseId": 9, "workflow": "CI", "title": "2 failed", "summary": null},
                {"__typename": "CheckRun", "name": "lint"},
                {"__typename": "StatusContext", "context": "deploy"}
            ])
        );
    }

    #[test]
    fn a_failed_call_knows_nothing_and_says_why() {
        let reply = parse_reply("", "error connecting to api.github.com\n", &[a()]);
        assert_eq!(
            reply.error.as_deref(),
            Some("error connecting to api.github.com")
        );
        assert_eq!(reply.missing, []);
        assert_eq!(
            parse_reply("", "", &[a()]).error.as_deref(),
            Some("gh api graphql failed")
        );
        let refused = json!({"errors": [{"message": "Bad credentials"}]}).to_string();
        assert_eq!(
            parse_reply(&refused, "", &[a()]).error.as_deref(),
            Some("Bad credentials")
        );
        assert_eq!(
            parse_reply("{}", "", &[a()]).error.as_deref(),
            Some("GitHub returned no data")
        );
    }

    /// `gh api graphql -i` on stdout: the status line ends "\n", each header "\r\n", then a
    /// blank line and the body.
    fn head(status: &str, headers: &[&str]) -> String {
        let lines: String = headers.iter().flat_map(|h| [h, "\r\n"]).collect();
        format!("HTTP/2.0 {status}\n{lines}\r\n")
    }

    const LIMITS: [&str; 5] = [
        "X-Ratelimit-Limit: 5000",
        "X-Ratelimit-Remaining: 4998",
        "X-Ratelimit-Reset: 1791378000",
        "X-Ratelimit-Resource: graphql",
        "X-Ratelimit-Used: 2",
    ];
    const SECONDARY_BODY: &str = r#"{"message":"You have exceeded a secondary rate limit. Please wait a few minutes before you try again.","documentation_url":"https://docs.github.com"}"#;

    fn with_status(status: Option<u16>) -> Http {
        Http {
            status,
            ..Http::default()
        }
    }

    fn with_header(status: u16, name: &str, value: &str) -> Http {
        Http {
            status: Some(status),
            headers: [(name.to_owned(), value.to_owned())].into(),
            body: String::new(),
        }
    }

    #[test]
    fn a_200_its_status_headers_by_lower_case_name_and_the_body() {
        let body =
            json!({"data": {"p0": {"pullRequest": {"number": 7, "commits": {"nodes": []}}}}})
                .to_string();
        let mut headers = vec!["Content-Type: application/json; charset=utf-8"];
        headers.extend(LIMITS);
        let http = parse_http(&(head("200 OK", &headers) + &body));
        assert_eq!(http.status, Some(200));
        assert_eq!(http.headers["x-ratelimit-remaining"], "4998");
        assert_eq!(
            http.headers["content-type"],
            "application/json; charset=utf-8"
        );
        assert_eq!(http.body, body);
        assert!(
            parse_reply(&http.body, "", &[a()])
                .found
                .contains_key(&a().url)
        );
        assert_eq!(
            rate_of(&http.headers),
            Some(RateLimit {
                remaining: 4998,
                reset_at: "2026-10-07T13:00:00.000Z".into(),
                limit: Some(5000)
            })
        );
    }

    #[test]
    fn the_rate_headers_count_only_where_they_speak_of_graphqls_points() {
        let of = |lines: &[&str]| rate_of(&parse_http(&head("200 OK", lines)).headers);
        let reset = "X-Ratelimit-Reset: 1791378000";
        assert_eq!(
            of(&["X-Ratelimit-Remaining: 12", reset]),
            Some(RateLimit {
                remaining: 12,
                reset_at: "2026-10-07T13:00:00.000Z".into(),
                limit: None
            })
        );
        assert_eq!(
            of(&[
                "X-Ratelimit-Remaining: 12",
                reset,
                "X-Ratelimit-Resource: core"
            ]),
            None
        );
        assert_eq!(of(&["X-Ratelimit-Remaining: lots", reset]), None);
    }

    #[test]
    fn a_gateways_502_its_html_page_is_the_body_with_no_rate_headers() {
        let http = parse_http(
            &(head(
                "502 Bad Gateway",
                &["Content-Length: 50", "Content-Type: text/html"],
            ) + "<html><body><h1>502 Bad Gateway</h1></body></html>\n"),
        );
        assert_eq!(http.status, Some(502));
        assert_eq!(rate_of(&http.headers), None);
        assert_eq!(
            parse_reply(&http.body, "gh: HTTP 502\n", &[a()])
                .error
                .as_deref(),
            Some("gh: HTTP 502")
        );
    }

    #[test]
    fn a_secondary_limits_403_and_its_retry_after() {
        let mut headers = vec!["Retry-After: 60"];
        headers.extend(LIMITS);
        let http = parse_http(&(head("403 Forbidden", &headers) + SECONDARY_BODY));
        assert_eq!(http.status, Some(403));
        assert_eq!(retry_after_of(&http.headers), Some(60.0));
        assert_eq!(retry_after_of(&BTreeMap::new()), None);
        let said =
            |value: &str| retry_after_of(&[("retry-after".to_owned(), value.to_owned())].into());
        assert_eq!(said("soon"), None);
        assert_eq!(said(""), None);
        assert_eq!(said("-1"), None);
        assert_eq!(said("1.5"), Some(1.5));
    }

    #[test]
    fn no_status_line_all_of_stdout_is_the_body() {
        assert_eq!(parse_http(""), Http::default());
        let body = json!({"data": {"p0": null}}).to_string();
        assert_eq!(
            parse_http(&body),
            Http {
                body: body.clone(),
                ..Http::default()
            }
        );
        // A head with no blank line after it is no head.
        assert_eq!(
            parse_http("HTTP/2.0 200 OK\nX-Ratelimit-Remaining: 1\r\n").status,
            None
        );
        // Nor headers at all: the blank line comes straight after the status.
        assert_eq!(
            parse_http("HTTP/2.0 204 No Content\n\r\n"),
            with_status(Some(204))
        );
    }

    #[test]
    fn a_gateways_5xx_graphqls_timeout_or_a_cut_connection_is_a_timeout() {
        let blips: [(&str, Option<u16>); 12] = [
            ("gh: HTTP 502", Some(502)),
            ("gh: HTTP 503", None),
            ("gh: HTTP 504", None),
            ("gh: Server Error (HTTP 502)", None),
            (
                "Something went wrong while executing your query. This may be the result of a timeout, or it could be a GitHub bug.",
                Some(200),
            ),
            (
                "Something went wrong while executing your query. This may be the result of a timeout, or it could be a GitHub bug.",
                Some(504),
            ),
            (
                "We couldn't respond to your request in time. Sorry about that.",
                Some(502),
            ),
            (r#"Post "https://api.github.com/graphql": EOF"#, None),
            (
                r#"Post "https://api.github.com/graphql": unexpected EOF"#,
                None,
            ),
            (
                r#"Post "https://api.github.com/graphql": read tcp 10.0.0.2:50123->140.82.112.6:443: read: connection reset by peer"#,
                None,
            ),
            (
                r#"Post "https://api.github.com/graphql": stream error: stream ID 3; INTERNAL_ERROR; received from peer"#,
                None,
            ),
            (
                r#"Post "https://api.github.com/graphql": net/http: TLS handshake timeout"#,
                None,
            ),
        ];
        for (error, status) in blips {
            assert_eq!(
                failure_of(error, &with_status(status)),
                CallFailure::Timeout,
                "{error}"
            );
        }
    }

    #[test]
    fn a_secondary_limit_by_its_message_on_a_403_or_a_200_or_by_any_retry_after() {
        let message = "You have exceeded a secondary rate limit. Please wait a few minutes before you try again.";
        assert_eq!(
            failure_of(
                &format!("gh: {message} (HTTP 403)"),
                &with_status(Some(403))
            ),
            CallFailure::SecondaryLimit
        );
        assert_eq!(
            failure_of(
                &format!("gh: {message} (HTTP 200)"),
                &with_status(Some(200))
            ),
            CallFailure::SecondaryLimit
        );
        assert_eq!(
            failure_of(
                "You have triggered an abuse detection mechanism.",
                &Http::default()
            ),
            CallFailure::SecondaryLimit
        );
        assert_eq!(
            failure_of("gh: HTTP 502", &with_header(502, "retry-after", "30")),
            CallFailure::SecondaryLimit
        );
    }

    #[test]
    fn the_primary_limit_spent_points_by_the_header_or_the_message() {
        assert_eq!(
            failure_of(
                "API rate limit already exceeded for user ID 1.",
                &with_status(Some(200))
            ),
            CallFailure::PrimaryLimit
        );
        assert_eq!(
            failure_of(
                "gh: HTTP 403",
                &with_header(403, "x-ratelimit-remaining", "0")
            ),
            CallFailure::PrimaryLimit
        );
    }

    #[test]
    fn a_refusal_a_missing_pr_and_the_rest_are_no_blip() {
        for (error, status) in [
            ("gh: Bad credentials (HTTP 401)", Some(401)),
            (
                "gh: Resource not accessible by integration (HTTP 403)",
                Some(403),
            ),
            ("gh: Not Found (HTTP 404)", Some(404)),
            ("gh: HTTP 500", Some(500)),
            ("Not found, or no gh account can see it", None),
            ("error connecting to api.github.com", None),
            ("gh api graphql failed", None),
        ] {
            assert_eq!(
                failure_of(error, &with_status(status)),
                CallFailure::Other,
                "{error}"
            );
        }
    }

    #[test]
    fn a_call_may_take_what_is_left_of_the_lease_never_more_than_its_cap() {
        let until = 1_791_374_439_000;
        assert_eq!(call_budget(until, until - 29_000, 20_000), Some(20_000));
        assert_eq!(call_budget(until, until - 8_850, 20_000), Some(8_850));
        assert_eq!(
            call_budget(until, until - MIN_CALL_MS, 20_000),
            Some(MIN_CALL_MS)
        );
        assert_eq!(call_budget(until, until - MIN_CALL_MS + 1, 20_000), None);
        assert_eq!(call_budget(until, until + 1_000, 20_000), None);
    }

    #[test]
    fn one_json_line_a_call() {
        let mut headers = vec!["Retry-After: 60"];
        headers.extend(LIMITS);
        let http = parse_http(&(head("403 Forbidden", &headers) + SECONDARY_BODY));
        let error = format!(
            "gh: You have exceeded a secondary rate limit. (HTTP 403) {}",
            "x".repeat(300)
        );
        let call = CallLog {
            login: "octocat",
            size: 20,
            ms: 3_120,
            http: &http,
            failure: CallFailure::SecondaryLimit,
            error: &error,
        };
        let line = log_line(1_791_374_400_000, &call);
        assert!(!line.contains('\n'));
        assert!(
            line.starts_with(r#"{"at":"2026-10-07T12:00:00.000Z","login":"octocat","batch":20"#)
        );
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap(),
            json!({
                "at": "2026-10-07T12:00:00.000Z",
                "login": "octocat",
                "batch": 20,
                "ms": 3_120,
                "status": 403,
                "class": "secondary-limit",
                "remaining": "4998",
                "used": "2",
                "resource": "graphql",
                "retryAfter": "60",
                "error": error[..200],
            })
        );
        let none = Http::default();
        let cut = CallLog {
            login: "octocat",
            size: 1,
            ms: 40,
            http: &none,
            failure: CallFailure::Timeout,
            error: r#"Post "https://api.github.com/graphql": EOF"#,
        };
        let cut: Value = serde_json::from_str(&log_line(0, &cut)).unwrap();
        assert_eq!(
            (&cut["status"], &cut["remaining"], &cut["retryAfter"]),
            (&Value::Null, &Value::Null, &Value::Null)
        );
    }

    #[test]
    fn appended_last_and_only_the_last_lines_kept() {
        assert_eq!(append_log("", "a", LOG_LINES), "a\n");
        assert_eq!(append_log("a\nb\n", "c", LOG_LINES), "a\nb\nc\n");
        assert_eq!(append_log("a\nb\nc\n", "d", 3), "b\nc\nd\n");
        let full: Vec<String> = (0..LOG_LINES).map(|i| i.to_string()).collect();
        let next = append_log(&full.join("\n"), "new", LOG_LINES);
        let next: Vec<&str> = next.trim_end().split('\n').collect();
        assert_eq!(next.len(), LOG_LINES);
        assert_eq!((next[0], next[LOG_LINES - 1]), ("1", "new"));
    }
}
