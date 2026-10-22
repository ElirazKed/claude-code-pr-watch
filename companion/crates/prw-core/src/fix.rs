//! Handing a failed CI run to an agent: which checks failed, their logs trimmed to the part
//! that says why, and the message that asks for a fix. A port of `hooks/fix.ts`; the binary
//! only fetches.

use std::sync::LazyLock;

use regex::Regex;

use crate::batch::first_line;
use crate::gh::{CheckItem, Conclusion, StatusState};
use crate::handoff::{PrFacts, chars, clip};
use crate::status::{CheckVerdict, is_status_context, latest_runs, verdict_of};

/// The tail of each failed job's log that is kept.
pub const LOG_LINES: usize = 120;
/// How many jobs' logs are fetched.
pub const LOG_JOBS: usize = 3;
/// The characters every log together may take: enough to find the error, not the runner's
/// setup.
pub const LOG_CHARS: usize = 12_000;
const LINE_CHARS: usize = 400;
/// The longest `gh run view --log-failed` may take (ms): a log can be large.
pub const LOG_MS: i64 = 60_000;

/// A failing check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub name: String,
    pub workflow: Option<String>,
    /// In words: "failure", "timed out", "error".
    pub conclusion: String,
    pub url: Option<String>,
    /// A check run's title and summary, or a status's description.
    pub summary: Option<String>,
    /// A GitHub Actions job, whose failed steps' log gh can fetch; `None` for any other CI.
    pub job_id: Option<i64>,
}

impl Failure {
    /// A failure known only by its name (the card's, where the checks wouldn't read).
    #[must_use]
    pub fn named(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            workflow: None,
            conclusion: "failed".to_owned(),
            url: None,
            summary: None,
            job_id: None,
        }
    }

    /// The check as the PR page names it, its workflow first when that says more (`labelOf`).
    fn label(&self) -> String {
        match &self.workflow {
            Some(workflow) if *workflow != self.name => format!("{workflow} / {}", self.name),
            _ => self.name.clone(),
        }
    }
}

static SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("a valid pattern"));

/// GitHub's value in words: lowercase, underscores as spaces (`words`).
fn words(value: Option<&str>) -> String {
    value.unwrap_or("failed").to_lowercase().replace('_', " ")
}

/// The checks failing on the latest run of each, counted the way the card counts them
/// (`failingChecks`).
#[must_use]
pub fn failing_checks(items: &[CheckItem], repo: &str) -> Vec<Failure> {
    latest_runs(items)
        .into_iter()
        .filter(|item| verdict_of(item) == CheckVerdict::Failed)
        .map(|item| {
            let is_context = is_status_context(item);
            let about = if is_context {
                item.description.clone().unwrap_or_default()
            } else {
                [&item.title, &item.summary]
                    .into_iter()
                    .flatten()
                    .filter(|text| !text.is_empty())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" · ")
            };
            let conclusion = if is_context {
                item.state
                    .as_ref()
                    .and_then(Option::as_ref)
                    .map(StatusState::as_str)
            } else {
                item.conclusion.as_ref().map(Conclusion::as_str)
            };
            let fallback = item
                .database_id
                .map(|id| format!("https://github.com/{repo}/runs/{id}"));
            Failure {
                name: item
                    .name
                    .clone()
                    .or_else(|| item.context.clone())
                    .unwrap_or_else(|| "check".to_owned()),
                workflow: item.workflow.clone(),
                conclusion: words(conclusion),
                url: item
                    .details_url
                    .clone()
                    .or_else(|| item.target_url.clone())
                    .or(fallback),
                summary: (!about.is_empty())
                    .then(|| clip(SPACE.replace_all(&about, " ").trim(), 300)),
                // Only a run in a workflow is an Actions job; another app's check run has no
                // log here.
                job_id: if !is_context && item.workflow.is_some() {
                    item.database_id
                } else {
                    None
                },
            }
        })
        .collect()
}

/// The jobs whose logs are worth a fetch: the first few Actions failures (`toFetch`).
#[must_use]
pub fn to_fetch(failures: &[Failure]) -> Vec<&Failure> {
    failures
        .iter()
        .filter(|f| f.job_id.is_some())
        .take(LOG_JOBS)
        .collect()
}

/// The `gh` arguments that fetch a job's failed steps' log.
#[must_use]
pub fn log_argv(job_id: i64, repo: &str) -> Vec<String> {
    [
        "run",
        "view",
        "--job",
        &job_id.to_string(),
        "--log-failed",
        "-R",
        repo,
    ]
    .map(str::to_owned)
    .to_vec()
}

static ANSI: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07]*\x07").expect("a valid pattern")
});
static STAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\x{feff}?\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z ?")
        .expect("a valid pattern")
});

/// A log, cleaned (`cleanLog`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cleaned {
    /// The failed steps' names, first seen first.
    pub steps: Vec<String>,
    pub lines: Vec<String>,
}

/// `gh run view --log-failed` prints "job<TAB>step<TAB>timestamp text" a line: kept are the
/// text, without colour codes or the runner's group markers, and the failed steps' names.
#[must_use]
pub fn clean_log(raw: &str) -> Cleaned {
    let mut steps: Vec<String> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    for row in raw.replace('\r', "").split('\n') {
        let parts: Vec<&str> = row.split('\t').collect();
        let is_columns = parts.len() >= 3;
        let step = if is_columns { parts[1] } else { "" };
        if !step.is_empty() && step != "UNKNOWN STEP" && !steps.iter().any(|s| s == step) {
            steps.push(step.to_owned());
        }
        let text = if is_columns {
            parts[2..].join("\t")
        } else {
            row.to_owned()
        };
        let text = STAMP.replace(&text, "");
        let text = ANSI.replace_all(&text, "");
        if text.starts_with("##[endgroup]") {
            continue;
        }
        let text = text.strip_prefix("##[group]").unwrap_or(&text);
        lines.push(clip(text, LINE_CHARS));
    }
    let is_blank = |line: &String| line.trim().is_empty();
    while lines.last().is_some_and(is_blank) {
        lines.pop();
    }
    let lead = lines.iter().take_while(|l| is_blank(l)).count();
    lines.drain(..lead);
    Cleaned { steps, lines }
}

/// The end of a log, where the error is (`trimLog`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trimmed {
    pub text: String,
    pub steps: Vec<String>,
    /// The lines kept, of `total`.
    pub kept: usize,
    pub total: usize,
}

/// The log's last `max_lines` lines, fewer if they pass `max_chars`.
#[must_use]
pub fn trim_log(raw: &str, max_lines: usize, max_chars: usize) -> Trimmed {
    let Cleaned { steps, lines } = clean_log(raw);
    let total = lines.len();
    let mut tail = &lines[total.saturating_sub(max_lines)..];
    let mut size: usize = tail.iter().map(|line| chars(line) + 1).sum();
    while tail.len() > 1 && size > max_chars {
        size -= chars(&tail[0]) + 1;
        tail = &tail[1..];
    }
    let lead = tail.iter().take_while(|l| l.trim().is_empty()).count();
    let tail = &tail[lead..];
    Trimmed {
        text: tail.join("\n"),
        steps,
        kept: tail.len(),
        total,
    }
}

static LOG_SAID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^failed to get run log:\s*").expect("a valid pattern"));

/// What gh said when a log would not come, in plain words (`logError`).
#[must_use]
pub fn log_error(stderr: &str) -> String {
    let text = LOG_SAID.replace(first_line(stderr), "");
    if text.contains("HTTP 410") {
        "GitHub no longer keeps this log".to_owned()
    } else if text.contains("HTTP 404") {
        "GitHub has no log for it, or this account can't see it".to_owned()
    } else if text.to_lowercase().contains("in progress") {
        "its run is still going, and GitHub gives the log once it ends".to_owned()
    } else if text.is_empty() {
        "gh run view failed".to_owned()
    } else {
        text.into_owned()
    }
}

/// A failure and what came of fetching its log: the raw log, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handoff {
    pub failure: Failure,
    pub log: Option<String>,
    pub error: Option<String>,
}

impl Handoff {
    /// A failure whose log wasn't asked for.
    #[must_use]
    pub const fn unfetched(failure: Failure) -> Self {
        Self {
            failure,
            log: None,
            error: None,
        }
    }
}

/// Logs that would not come, for the toast: "No log for build: GitHub no longer keeps this
/// log" (`logProblems`).
#[must_use]
pub fn log_problems(handoffs: &[Handoff]) -> Option<String> {
    let problems: Vec<String> = handoffs
        .iter()
        .filter_map(|h| {
            h.error
                .as_ref()
                .map(|error| format!("No log for {}: {error}", h.failure.name))
        })
        .collect();
    (!problems.is_empty()).then(|| problems.join("; "))
}

/// A fence longer than any run of backticks in the text, so the text can't close it.
#[must_use]
pub fn fence(text: &str) -> String {
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    "`".repeat((longest + 1).max(3))
}

/// The prompt's first words (`fixHeading`).
#[must_use]
pub fn fix_heading(pr: &PrFacts) -> String {
    format!("CI failed on {}#{}:", pr.repo, pr.number)
}

/// The message for the agent: the PR, what failed, the logs' tails, and the ask (`fixPrompt`).
/// It never asks for a push or a merge; that stays the person's call.
#[must_use]
pub fn fix_prompt(pr: &PrFacts, handoffs: &[Handoff]) -> String {
    let logs = handoffs.iter().filter(|h| h.log.is_some()).count();
    let budget = LOG_CHARS / logs.max(1);
    let branch = pr.branch();
    let mut out = vec![
        format!("{} {}", fix_heading(pr), pr.title()),
        format!(
            "{} · {branch} → {} · checks: {}/checks",
            pr.url,
            pr.base(),
            pr.url
        ),
        String::new(),
        "Failing checks (the latest run of each):".to_owned(),
    ];
    out.extend(handoffs.iter().map(|h| {
        let f = &h.failure;
        let parts: Vec<String> = [
            Some(f.label()),
            Some(f.conclusion.clone()),
            f.summary.clone(),
            f.url.clone(),
        ]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect();
        format!("- {}", parts.join(" · "))
    }));
    for h in handoffs {
        let name = h.failure.label();
        if let Some(error) = &h.error {
            out.extend([String::new(), format!("{name}: no log ({error}).")]);
        }
        let Some(raw) = &h.log else { continue };
        let log = trim_log(raw, LOG_LINES, budget);
        if log.kept == 0 {
            out.extend([
                String::new(),
                format!("{name}: gh found no failed step in its log."),
            ]);
            continue;
        }
        let step = if log.steps.is_empty() {
            String::new()
        } else {
            let names: Vec<String> = log.steps.iter().map(|s| format!("\"{s}\"")).collect();
            format!(", failed step {}", names.join(", "))
        };
        let span = if log.kept < log.total {
            format!("last {} of {} lines, trimmed", log.kept, log.total)
        } else {
            format!("all {} lines", log.total)
        };
        let bar = fence(&log.text);
        out.extend([
            String::new(),
            format!("{name} log ({span}{step}):"),
            bar.clone(),
            log.text,
            bar,
        ]);
    }
    let unfetched = handoffs
        .iter()
        .filter(|h| h.failure.job_id.is_some() && h.log.is_none() && h.error.is_none())
        .count();
    if unfetched > 0 {
        out.extend([
            String::new(),
            format!(
                "Logs of the first {LOG_JOBS} failing jobs only; {unfetched} more not fetched."
            ),
        ]);
    }
    out.extend([
        String::new(),
        format!(
            "Find the cause and fix it on this branch ({branch}). If {} or {branch} isn't checked out here, say so first.",
            pr.repo
        ),
    ]);
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::{Value, json};

    fn facts() -> PrFacts {
        PrFacts {
            repo: "acme/app".to_owned(),
            number: 13,
            title: "Retry uploads with exponential backoff".to_owned(),
            url: "https://github.com/acme/app/pull/13".to_owned(),
            branch: "feat/retry".to_owned(),
            base: "main".to_owned(),
            author: String::new(),
        }
    }

    fn items(value: Value) -> Vec<CheckItem> {
        serde_json::from_value(value).expect("check items")
    }

    fn job(name: &str, id: i64, conclusion: &str, started_at: &str) -> Value {
        json!({
            "__typename": "CheckRun", "name": name, "status": "COMPLETED", "conclusion": conclusion,
            "startedAt": started_at, "databaseId": id,
            "detailsUrl": format!("https://github.com/acme/app/actions/runs/1/job/{id}"), "workflow": "CI",
        })
    }

    /// What `gh run view --job <id> --log-failed` prints: job, step, then the timestamped line.
    fn gh_log(lines: &[String], step: &str) -> String {
        lines
            .iter()
            .enumerate()
            .map(|(i, line)| {
                format!(
                    "build\t{step}\t2026-10-07T11:{:02}:00.1234567Z {line}",
                    i % 60
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn numbered(n: usize, line: impl Fn(usize) -> String) -> Vec<String> {
        (0..n).map(line).collect()
    }

    #[test]
    fn the_latest_run_of_each_a_rerun_that_passed_does_not_count() {
        let failures = failing_checks(
            &items(json!([
                job("build", 901, "FAILURE", "2026-10-07T11:00:00Z"),
                job("lint", 902, "FAILURE", "2026-10-07T11:00:00Z"),
                job("lint", 903, "SUCCESS", "2026-10-07T11:05:00Z"),
                {"__typename": "CheckRun", "name": "e2e", "status": "IN_PROGRESS", "startedAt": "2026-10-07T11:00:00Z", "workflow": "CI"},
            ])),
            "acme/app",
        );
        assert_eq!(failures.len(), 1);
        assert_eq!(
            (failures[0].name.as_str(), failures[0].job_id),
            ("build", Some(901))
        );
    }

    #[test]
    fn another_cis_check_run_or_status_has_no_job_only_its_link_and_summary() {
        let failures = failing_checks(
            &items(json!([
                {"__typename": "CheckRun", "name": "ci/external", "status": "COMPLETED", "conclusion": "TIMED_OUT", "databaseId": 77, "title": "Timed out"},
                {"__typename": "StatusContext", "context": "ci/jenkins", "state": "ERROR", "description": "tests failed", "targetUrl": "https://ci.example.com/job/7"},
            ])),
            "acme/app",
        );
        assert_eq!(
            failures,
            vec![
                Failure {
                    name: "ci/external".to_owned(),
                    workflow: None,
                    conclusion: "timed out".to_owned(),
                    url: Some("https://github.com/acme/app/runs/77".to_owned()),
                    summary: Some("Timed out".to_owned()),
                    job_id: None,
                },
                Failure {
                    name: "ci/jenkins".to_owned(),
                    workflow: None,
                    conclusion: "error".to_owned(),
                    url: Some("https://ci.example.com/job/7".to_owned()),
                    summary: Some("tests failed".to_owned()),
                    job_id: None,
                },
            ]
        );
    }

    #[test]
    fn logs_are_fetched_for_the_first_three_actions_jobs_only() {
        let all: Vec<Value> = (1..=4)
            .map(|n| job(&format!("job{n}"), n, "FAILURE", "2026-10-07T11:00:00Z"))
            .collect();
        let failures = failing_checks(&items(Value::Array(all)), "acme/app");
        let ids: Vec<Option<i64>> = to_fetch(&failures).iter().map(|f| f.job_id).collect();
        assert_eq!(ids, vec![Some(1), Some(2), Some(3)]);
        assert_eq!(
            log_argv(901, "acme/app"),
            [
                "run",
                "view",
                "--job",
                "901",
                "--log-failed",
                "-R",
                "acme/app"
            ]
        );
    }

    #[test]
    fn cleaning_drops_columns_timestamps_colour_codes_and_group_markers() {
        let raw = format!(
            "\u{feff}{}\n",
            gh_log(
                &[
                    "##[group]Run npm test".to_owned(),
                    "\u{1b}[31mFAIL\u{1b}[0m src/upload.test.ts".to_owned(),
                    "##[endgroup]".to_owned()
                ],
                "Run tests"
            )
        );
        assert_eq!(
            clean_log(&raw),
            Cleaned {
                steps: vec!["Run tests".to_owned()],
                lines: vec![
                    "Run npm test".to_owned(),
                    "FAIL src/upload.test.ts".to_owned()
                ],
            }
        );
    }

    #[test]
    fn trimming_keeps_the_tail_and_says_how_much_it_dropped() {
        let log = trim_log(
            &gh_log(&numbered(300, |i| format!("line {i}")), "Run tests"),
            LOG_LINES,
            LOG_CHARS,
        );
        assert_eq!((log.kept, log.total), (120, 300));
        assert!(log.text.starts_with("line 180\n"));
        assert!(log.text.ends_with("line 299"));
    }

    #[test]
    fn a_character_budget_cuts_further_from_the_front() {
        let x = "x".repeat(97);
        let log = trim_log(
            &gh_log(&numbered(50, |i| format!("{i:02} {x}")), "Run tests"),
            120,
            1_000,
        );
        assert_eq!(log.kept, 9);
        assert!(log.text.ends_with(&format!("49 {x}")));
    }

    #[test]
    fn ghs_refusal_in_plain_words() {
        let gone = "failed to get run log: HTTP 410: Server Error (https://api.github.com/repos/acme/app/actions/runs/1/logs)\n";
        assert_eq!(log_error(gone), "GitHub no longer keeps this log");
        assert_eq!(log_error(""), "gh run view failed");
        assert_eq!(
            log_error("run 1 is still in progress"),
            "its run is still going, and GitHub gives the log once it ends"
        );
        assert_eq!(
            log_error("failed to get run log: something else"),
            "something else"
        );
    }

    fn build() -> Failure {
        failing_checks(
            &items(json!([job(
                "build",
                901,
                "FAILURE",
                "2026-10-07T11:00:00Z"
            )])),
            "acme/app",
        )
        .remove(0)
    }

    fn jenkins() -> Failure {
        failing_checks(
            &items(json!([{"__typename": "StatusContext", "context": "ci/jenkins", "state": "FAILURE", "description": "tests failed", "targetUrl": "https://ci.example.com/job/7"}])),
            "acme/app",
        )
        .remove(0)
    }

    #[test]
    fn the_prompt_names_the_pr_and_checks_fences_the_log_and_asks_for_a_fix() {
        let mut lines = numbered(200, |i| format!("setup {i}"));
        lines.push("Error: expected 3 retries, got 1".to_owned());
        let text = fix_prompt(
            &facts(),
            &[
                Handoff {
                    failure: build(),
                    log: Some(gh_log(&lines, "Run tests")),
                    error: None,
                },
                Handoff::unfetched(jenkins()),
            ],
        );
        assert!(
            text.starts_with("CI failed on acme/app#13: Retry uploads with exponential backoff\n")
        );
        assert!(text.contains("https://github.com/acme/app/pull/13 · feat/retry → main"));
        assert!(text.contains(
            "- CI / build · failure · https://github.com/acme/app/actions/runs/1/job/901"
        ));
        assert!(
            text.contains("- ci/jenkins · failure · tests failed · https://ci.example.com/job/7")
        );
        assert!(text.contains(
            "build log (last 120 of 201 lines, trimmed, failed step \"Run tests\"):\n```\n"
        ));
        assert!(text.contains("Error: expected 3 retries, got 1\n```"));
        assert!(!text.contains("setup 80\n"));
        assert!(!text.contains("2026-10-07T11"));
        assert!(text.contains("Find the cause and fix it on this branch (feat/retry). If acme/app or feat/retry isn't checked out here, say so first."));
        let lower = text.to_lowercase();
        assert!(!lower.contains(" push") && !lower.contains("merge"));
    }

    #[test]
    fn every_log_together_stays_inside_the_cap() {
        let big = gh_log(
            &numbered(120, |i| format!("{i} {}", "y".repeat(300))),
            "Run tests",
        );
        let all: Vec<Value> = (1..=3)
            .map(|n| job(&format!("job{n}"), n, "FAILURE", "2026-10-07T11:00:00Z"))
            .collect();
        let handoffs: Vec<Handoff> = failing_checks(&items(Value::Array(all)), "acme/app")
            .into_iter()
            .map(|failure| Handoff {
                failure,
                log: Some(big.clone()),
                error: None,
            })
            .collect();
        let text = fix_prompt(&facts(), &handoffs);
        let logs: Vec<&str> = text
            .split("```\n")
            .skip(1)
            .step_by(2)
            .map(|l| l.trim_end_matches('\n'))
            .collect();
        assert_eq!(logs.len(), 3);
        assert!(logs.iter().map(|l| chars(l)).sum::<usize>() <= LOG_CHARS);
        assert!(text.contains("trimmed"));
    }

    #[test]
    fn a_log_that_would_not_come_leaves_the_name_and_link_and_says_why() {
        let handoffs = [Handoff {
            failure: build(),
            log: None,
            error: Some("GitHub no longer keeps this log".to_owned()),
        }];
        let text = fix_prompt(&facts(), &handoffs);
        assert!(text.contains(
            "- CI / build · failure · https://github.com/acme/app/actions/runs/1/job/901"
        ));
        assert!(text.contains("CI / build: no log (GitHub no longer keeps this log)."));
        assert_eq!(
            log_problems(&handoffs).as_deref(),
            Some("No log for build: GitHub no longer keeps this log")
        );
    }

    #[test]
    fn leading_blank_lines_dont_make_an_uncut_log_read_as_trimmed() {
        let lines = ["", "  ", "Error: boom"].map(str::to_owned);
        let text = fix_prompt(
            &facts(),
            &[Handoff {
                failure: build(),
                log: Some(gh_log(&lines, "Run tests")),
                error: None,
            }],
        );
        assert!(text.contains("build log (all 1 lines, failed step \"Run tests\"):"));
        assert!(!text.contains("trimmed"));
    }

    #[test]
    fn a_log_with_a_fence_in_it_gets_a_longer_fence() {
        let lines = ["```", "boom"].map(str::to_owned);
        let text = fix_prompt(
            &facts(),
            &[Handoff {
                failure: build(),
                log: Some(gh_log(&lines, "Run tests")),
                error: None,
            }],
        );
        assert!(text.contains("````\n```\nboom\n````"));
    }

    #[test]
    fn jobs_past_the_first_few_are_counted() {
        let failures: Vec<Handoff> = (1..=4)
            .map(|n| {
                let mut f = build();
                f.job_id = Some(n);
                Handoff::unfetched(f)
            })
            .collect();
        assert!(
            fix_prompt(&facts(), &failures)
                .contains("Logs of the first 3 failing jobs only; 4 more not fetched.")
        );
    }

    proptest! {
        #[test]
        fn no_log_closes_its_fence(lines in proptest::collection::vec("[`a-z ]{0,12}", 1..8)) {
            let text = lines.join("\n");
            let bar = fence(&text);
            prop_assert!(bar.len() >= 3);
            // The bar never appears in the text as a run of its own length or longer.
            prop_assert!(!text.contains(&bar));
        }

        #[test]
        fn the_heading_is_the_prompts_prefix(title in ".{0,30}", n in 1u64..10_000) {
            let pr = PrFacts { title, number: n, ..facts() };
            prop_assert!(fix_prompt(&pr, &[]).starts_with(&fix_heading(&pr)));
        }

        #[test]
        fn a_trimmed_log_keeps_its_caps(lines in proptest::collection::vec(".{0,80}", 0..200), max_lines in 1usize..150, max_chars in 1usize..5_000) {
            let raw = lines.join("\n");
            let log = trim_log(&raw, max_lines, max_chars);
            prop_assert!(log.kept <= max_lines.min(log.total));
            // Over the budget only where one line is all that is left.
            prop_assert!(log.kept <= 1 || chars(&log.text) < max_chars);
        }
    }
}
