//! Starting a PR's GitHub Actions runs again (`R`): which of the head commit's runs, and how.
//! A run with a job that failed, was cancelled or timed out gets its failed jobs again, with
//! the jobs that wait on them; one whose jobs were only skipped, or that never got going, runs
//! again whole, so its `if:`s are asked afresh. A job its own `if:` skipped skips again
//! unless what the `if:` asks has changed.

use std::collections::HashMap;
use std::collections::HashSet;

use serde_json::Value;

use crate::batch::{PrRef, first_line, json_string, or};
use crate::gh::{CheckItem, CheckStatus, Conclusion};
use crate::status::latest_runs;

/// A job that ended this way is run again with `--failed`.
const FAILED: [&str; 3] = ["FAILURE", "CANCELLED", "TIMED_OUT"];

/// A run that ended this way, or holds a job that was skipped, is run again whole.
const WHOLE: [&str; 2] = ["STARTUP_FAILURE", "SKIPPED"];

/// One workflow run to start again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rerun {
    /// The run's id (its `databaseId`).
    pub id: i64,
    pub workflow: String,
    /// Its failed jobs only (`--failed`), else the whole run.
    pub failed_only: bool,
}

impl Rerun {
    /// What the toast names it by: "CI (failed jobs)".
    #[must_use]
    pub fn words(&self) -> String {
        let how = if self.failed_only {
            "failed jobs"
        } else {
            "whole run"
        };
        format!("{} ({how})", self.workflow)
    }
}

/// A check of the poll's that `R` starts again: a GitHub Actions job (it names its workflow)
/// that ended failed, cancelled, timed out or skipped.
#[must_use]
pub fn is_rerunnable(item: &CheckItem) -> bool {
    item.workflow.is_some()
        && item.status == Some(CheckStatus::Completed)
        && matches!(
            item.conclusion,
            Some(
                Conclusion::Failure
                    | Conclusion::Cancelled
                    | Conclusion::TimedOut
                    | Conclusion::StartupFailure
                    | Conclusion::Skipped
            )
        )
}

/// The names of a PR's checks that `R` starts again, the latest run of each. A workflow
/// with a job still going is left out: its run isn't finished, so it can't start again yet.
#[must_use]
pub fn rerunnable(items: &[CheckItem]) -> Vec<String> {
    let latest = latest_runs(items);
    let running: HashSet<&str> = latest
        .iter()
        .filter(|item| item.status != Some(CheckStatus::Completed))
        .filter_map(|item| item.workflow.as_deref())
        .collect();
    latest
        .into_iter()
        .filter(|item| is_rerunnable(item))
        .filter(|item| {
            !item
                .workflow
                .as_deref()
                .is_some_and(|w| running.contains(w))
        })
        .map(crate::status::check_name)
        .collect()
}

/// One job to start again by itself (`R` on a check selected in the full view): GitHub runs
/// it, and the jobs that wait on it, as the run's next attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RerunJob {
    /// The job's id (the check run's `databaseId`).
    pub id: i64,
    pub name: String,
}

/// The check `item` as a job to start again, where it is one: [`is_rerunnable`], with a job
/// id, and its workflow done (`items` are the PR's checks), as GitHub won't start a job of a
/// run that still runs.
#[must_use]
pub fn job_to_rerun(items: &[CheckItem], item: &CheckItem) -> Option<RerunJob> {
    let id = item.database_id.filter(|_| is_rerunnable(item))?;
    let is_running = latest_runs(items).into_iter().any(|other| {
        other.workflow == item.workflow && other.status != Some(CheckStatus::Completed)
    });
    (!is_running).then(|| RerunJob {
        id,
        name: crate::status::check_name(item),
    })
}

/// `gh run rerun --job`'s arguments.
#[must_use]
pub fn rerun_job_argv(repo: &str, job: &RerunJob) -> Vec<String> {
    ["run", "rerun", "--job", &job.id.to_string(), "--repo", repo]
        .map(str::to_owned)
        .to_vec()
}

/// The head commit's workflow runs, each with its jobs' ends: one call, on that press only.
#[must_use]
pub fn rerun_query(pr: &PrRef) -> String {
    let mut parts = pr.repo.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    format!(
        "query {{
  p0: repository(owner: {}, name: {}) {{ pullRequest(number: {}) {{
    commits(last: 1) {{ nodes {{ commit {{ checkSuites(first: 50) {{ nodes {{
      status conclusion
      workflowRun {{ databaseId workflow {{ name }} }}
      checkRuns(first: 100, filterBy: {{ checkType: LATEST }}) {{ nodes {{ conclusion }} }}
    }} }} }} }} }}
  }} }}
}}",
        json_string(owner),
        json_string(name),
        pr.number,
    )
}

/// The runs [`rerun_query`]'s reply says to start again: those finished with a job to run
/// again, Actions' only (a suite with no workflow run is another app's).
///
/// # Errors
///
/// Why none could be read: the call failed, or this account can't see the PR.
pub fn parse_reruns(stdout: &str, stderr: &str) -> Result<Vec<Rerun>, String> {
    let reply: Value = serde_json::from_str(stdout)
        .map_err(|_| or(first_line(stderr), "gh api graphql failed"))?;
    let Some(pr) = reply
        .pointer("/data/p0/pullRequest")
        .filter(|v| !v.is_null())
    else {
        let said = reply.pointer("/errors/0/message").and_then(Value::as_str);
        return Err(said.map_or_else(
            || "Not found, or this gh account can't see it".to_owned(),
            str::to_owned,
        ));
    };
    let suites = pr
        .pointer("/commits/nodes/0/commit/checkSuites/nodes")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    // A workflow's latest run on the head only (the highest run id): an older one, cancelled
    // as a newer one began, is not run again.
    let mut latest: HashMap<String, i64> = HashMap::new();
    for suite in suites {
        let Some(run) = suite.get("workflowRun").filter(|r| !r.is_null()) else {
            continue;
        };
        let id = run
            .get("databaseId")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let entry = latest.entry(workflow_of(run)).or_insert(id);
        *entry = (*entry).max(id);
    }
    Ok(suites
        .iter()
        .filter_map(rerun_of)
        .filter(|r| latest.get(&r.workflow) == Some(&r.id))
        .collect())
}

/// A run's workflow name ("workflow" where none is given).
fn workflow_of(run: &Value) -> String {
    run.pointer("/workflow/name")
        .and_then(Value::as_str)
        .unwrap_or("workflow")
        .to_owned()
}

/// A suite as a run to start again, where it is one.
fn rerun_of(suite: &Value) -> Option<Rerun> {
    let run = suite.get("workflowRun").filter(|r| !r.is_null())?;
    if suite.get("status").and_then(Value::as_str) != Some("COMPLETED") {
        return None;
    }
    let text = |v: &Value, key: &str| {
        v.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let jobs: Vec<String> = suite
        .pointer("/checkRuns/nodes")
        .and_then(Value::as_array)
        .map(|nodes| nodes.iter().map(|job| text(job, "conclusion")).collect())
        .unwrap_or_default();
    let failed_only = jobs.iter().any(|c| FAILED.contains(&c.as_str()));
    let ended = text(suite, "conclusion");
    // A run that failed or was cancelled with no job to show for it (cancelled while
    // queued) runs again whole.
    let whole = WHOLE.contains(&ended.as_str())
        || FAILED.contains(&ended.as_str())
        || jobs.iter().any(|c| c == "SKIPPED");
    (failed_only || whole)
        .then(|| Rerun {
            id: run
                .get("databaseId")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
            workflow: workflow_of(run),
            failed_only,
        })
        .filter(|r| r.id > 0)
}

/// `gh run rerun`'s arguments.
#[must_use]
pub fn rerun_argv(repo: &str, run: &Rerun) -> Vec<String> {
    let mut argv = vec![
        "run".to_owned(),
        "rerun".to_owned(),
        run.id.to_string(),
        "--repo".to_owned(),
        repo.to_owned(),
    ];
    if run.failed_only {
        argv.push("--failed".to_owned());
    }
    argv
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    fn reply(suites: &Value) -> String {
        json!({"data": {"p0": {"pullRequest": {"commits": {"nodes": [{"commit": {"checkSuites": {"nodes": suites}}}]}}}}})
            .to_string()
    }

    fn suite(id: i64, conclusion: &str, jobs: &[&str]) -> Value {
        json!({
            "status": "COMPLETED",
            "conclusion": conclusion,
            "workflowRun": {"databaseId": id, "workflow": {"name": format!("wf{id}")}},
            "checkRuns": {"nodes": jobs.iter().map(|c| json!({"conclusion": c})).collect::<Vec<_>>()},
        })
    }

    #[test]
    fn a_failed_job_runs_again_with_its_dependents_and_a_skipped_one_runs_whole() {
        let suites = json!([
            suite(1, "FAILURE", &["FAILURE", "SKIPPED"]),
            suite(2, "SUCCESS", &["SUCCESS", "SKIPPED"]),
            suite(3, "SKIPPED", &[]),
            suite(4, "SUCCESS", &["SUCCESS"]),
            suite(5, "CANCELLED", &["CANCELLED"]),
        ]);
        let runs = parse_reruns(&reply(&suites), "").unwrap();
        let picked: Vec<(i64, bool)> = runs.iter().map(|r| (r.id, r.failed_only)).collect();
        assert_eq!(picked, [(1, true), (2, false), (3, false), (5, true)]);
        assert_eq!(runs[0].words(), "wf1 (failed jobs)");
        assert_eq!(runs[1].words(), "wf2 (whole run)");
    }

    #[test]
    fn only_a_workflows_latest_run_runs_again_and_a_run_cancelled_while_queued_runs_whole() {
        let mut older = suite(1, "CANCELLED", &["CANCELLED"]);
        older["workflowRun"]["workflow"]["name"] = json!("CI");
        let mut newer = suite(2, "SUCCESS", &["SUCCESS"]);
        newer["workflowRun"]["workflow"]["name"] = json!("CI");
        let queued = suite(3, "CANCELLED", &[]);
        let runs = parse_reruns(&reply(&json!([older, newer, queued])), "").unwrap();
        let picked: Vec<(i64, bool)> = runs.iter().map(|r| (r.id, r.failed_only)).collect();
        assert_eq!(picked, [(3, false)]);
    }

    #[test]
    fn a_running_run_or_another_apps_suite_is_left_alone() {
        let mut running = suite(1, "FAILURE", &["FAILURE"]);
        running["status"] = json!("IN_PROGRESS");
        let mut app = suite(2, "FAILURE", &["FAILURE"]);
        app["workflowRun"] = Value::Null;
        assert_eq!(parse_reruns(&reply(&json!([running, app])), ""), Ok(vec![]));
    }

    #[test]
    fn a_pr_it_can_t_read_says_why() {
        let missing = json!({"data": {"p0": {"pullRequest": null}}}).to_string();
        assert_eq!(
            parse_reruns(&missing, ""),
            Err("Not found, or this gh account can't see it".to_owned())
        );
        let refused = json!({"errors": [{"message": "Bad credentials"}]}).to_string();
        assert_eq!(
            parse_reruns(&refused, ""),
            Err("Bad credentials".to_owned())
        );
        assert_eq!(
            parse_reruns("", "gh: HTTP 502"),
            Err("gh: HTTP 502".to_owned())
        );
    }

    #[test]
    fn the_poll_offers_actions_jobs_that_ended_badly_or_skipped() {
        let item = |workflow: Option<&str>, conclusion: &str| -> CheckItem {
            serde_json::from_value(json!({
                "__typename": "CheckRun", "name": "build", "status": "COMPLETED",
                "conclusion": conclusion, "workflow": workflow,
            }))
            .unwrap()
        };
        assert!(is_rerunnable(&item(Some("CI"), "SKIPPED")));
        assert!(is_rerunnable(&item(Some("CI"), "FAILURE")));
        assert!(!is_rerunnable(&item(Some("CI"), "SUCCESS")));
        assert!(!is_rerunnable(&item(None, "FAILURE")), "not an Actions job");
        let mut running = item(Some("CI"), "SUCCESS");
        running.name = Some("test".to_owned());
        running.status = Some(CheckStatus::InProgress);
        running.conclusion = None;
        assert_eq!(
            rerunnable(&[item(Some("CI"), "FAILURE"), running]),
            Vec::<String>::new(),
            "CI is still going"
        );
        assert_eq!(rerunnable(&[item(Some("CI"), "FAILURE")]), ["build"]);
    }

    #[test]
    fn a_selected_skipped_job_runs_alone_once_its_workflow_is_done() {
        let item = |id: i64, workflow: &str, status: &str, conclusion: Option<&str>| -> CheckItem {
            serde_json::from_value(json!({
                "__typename": "CheckRun", "name": format!("job{id}"), "status": status,
                "conclusion": conclusion, "workflow": workflow, "databaseId": id,
            }))
            .unwrap()
        };
        let skipped = item(1, "E2E", "COMPLETED", Some("SKIPPED"));
        let passed = item(2, "CI", "COMPLETED", Some("SUCCESS"));
        let running = item(3, "CI", "IN_PROGRESS", None);
        let items = vec![skipped.clone(), passed.clone(), running];
        assert_eq!(
            job_to_rerun(&items, &skipped),
            Some(RerunJob {
                id: 1,
                name: "job1".to_owned()
            })
        );
        assert_eq!(job_to_rerun(&items, &passed), None, "it passed");
        let failed = item(4, "CI", "COMPLETED", Some("FAILURE"));
        let with_failed: Vec<CheckItem> = items.iter().cloned().chain([failed.clone()]).collect();
        assert_eq!(job_to_rerun(&with_failed, &failed), None, "CI still runs");
        assert_eq!(
            rerun_job_argv(
                "acme/app",
                &RerunJob {
                    id: 1,
                    name: "job1".to_owned()
                }
            ),
            ["run", "rerun", "--job", "1", "--repo", "acme/app"]
        );
    }

    proptest! {
        #[test]
        fn the_argv_names_the_run_and_its_repo_and_only_failed_runs_say_failed(
            id in 1_i64..i64::MAX, failed_only: bool, repo in "[a-z]{1,8}/[a-z]{1,8}",
        ) {
            let run = Rerun { id, workflow: "CI".to_owned(), failed_only };
            let argv = rerun_argv(&repo, &run);
            prop_assert_eq!(&argv[..5], &["run".to_owned(), "rerun".to_owned(), id.to_string(), "--repo".to_owned(), repo]);
            prop_assert_eq!(argv.last().map(String::as_str) == Some("--failed"), failed_only);
        }

        #[test]
        fn a_run_is_picked_once_it_holds_a_job_to_run_again(
            jobs in proptest::collection::vec(prop::sample::select(vec!["SUCCESS", "FAILURE", "SKIPPED", "CANCELLED", "NEUTRAL", "TIMED_OUT"]), 0..6),
        ) {
            let runs = parse_reruns(&reply(&json!([suite(7, "SUCCESS", &jobs)])), "").unwrap();
            let expected = jobs.iter().any(|c| *c != "SUCCESS" && *c != "NEUTRAL");
            prop_assert_eq!(runs.len() == 1, expected);
            if let Some(run) = runs.first() {
                prop_assert_eq!(run.failed_only, jobs.iter().any(|c| FAILED.contains(c)));
            }
        }
    }
}
