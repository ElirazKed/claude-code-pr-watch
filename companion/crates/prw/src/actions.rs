//! Running what the action keys asked (`prw_core::act::Job`), each on a thread of its own so a
//! log that takes a minute holds nothing else up: a merge (`runMerge` in the mod's
//! `hooks/register.tsx`), a comment (`postComment`), the full view's changes (a reply, a
//! thread resolved or reopened, an approval: `prw_core::mutate`), the PR closed or reopened,
//! its failed or skipped Actions runs started again (`prw_core::rerun`), and the reads a Fix or
//! Address prompt needs (`collectFailure`, `collectThreads`). Every call goes as the login `results.json`
//! maps the repo's owner to, by that login's own token, and never as another.
//!
//! After a merge, a comment or a change the PR is asked again at once (the mod's `fetchNow`), so the
//! card shows what happened before the next round; with `--no-poll` nothing is written.
//!
//! The full view's reads (`prw_core::opened::Read`) run the same way: the PR whole, and its
//! failed jobs' logs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use prw_core::accounts::login_for;
use prw_core::act::{Done, Job, OnThread, Outcome};
use prw_core::address::{address_prompt, open_threads, parse_threads, threads_query};
use prw_core::batch::{PrRef, checks_query, parse_checks, ref_from_url};
use prw_core::comment::{comment_argv, comment_input, refused};
use prw_core::fix::{
    Failure, Handoff, LOG_MS, failing_checks, fix_prompt, log_argv, log_error, log_problems,
    to_fetch,
};
use prw_core::full::{Full, full_query, parse_full};
use prw_core::handoff::{Kind, PrFacts};
use prw_core::merge::{
    After, MergeError, MergeRun, RETRY_MS, after_server_error, is_server_error, merge_argv,
    merge_error, merge_read_argv, merge_read_of,
};
use prw_core::mutate::{MUTATE_ARGV, approve_input, close_argv, reply_input, resolve_input};
use prw_core::opened::{Got, Read, ReadDone};
use prw_core::parse_shared;
use prw_core::rerun::{parse_reruns, rerun_argv, rerun_query};
use prw_core::ui::{Msg, repo_of};

use crate::gh::{Clock, Gh, Ran, ms};
use crate::poller::{Files, refresh};
use crate::round::{Auth, Ctx, GH_MS};

/// What it says where a comment's call was cut off: GitHub may have taken it, so trying
/// again could post it twice.
const UNSURE: &str =
    "Couldn't tell whether it posted: gh api did not finish. Check the PR before trying again";

/// What it says where a resolve's or an approval's call was cut off.
const UNSURE_CHANGE: &str = "gh api did not finish: the PR is read again to show whether it took";

/// What it says where a close's or a reopen's call was cut off.
const UNSURE_CLOSE: &str = "gh pr did not finish: the PR is read again to show whether it took";

/// What runs the jobs: `gh`, the clock, gh's logins and tokens (shared by every job), and the
/// cache.
pub struct Runner<G: Gh, C: Clock> {
    gh: G,
    clock: C,
    auth: Mutex<Auth>,
    dir: PathBuf,
    /// Where a refresh writes; `None` with `--no-poll`.
    files: Option<Files>,
}

impl<G: Gh, C: Clock> Runner<G, C> {
    pub fn new(gh: G, clock: C, dir: PathBuf, files: Option<Files>) -> Self {
        Self {
            gh,
            clock,
            auth: Mutex::new(Auth::default()),
            dir,
            files,
        }
    }

    /// gh, as a round asks it, with the logins and tokens every job shares: held while it is
    /// used, so two jobs never ask gh for the same token at once.
    fn with_ctx<T>(&self, f: impl FnOnce(&mut Ctx<'_, G, C>) -> T) -> T {
        let mut auth = self.auth.lock().unwrap_or_else(PoisonError::into_inner);
        let mut ctx = Ctx {
            gh: &self.gh,
            clock: &self.clock,
            auth: &mut auth,
            log: self.files.as_ref().map(Files::log_path),
        };
        f(&mut ctx)
    }

    /// The login that acts on `repo` (the one the poll found sees it, else gh's active one),
    /// and its token or why there is none.
    fn token_for(&self, repo: &str) -> (String, Result<String, String>) {
        let accounts = self.accounts();
        let owner = repo.split('/').next().unwrap_or_default();
        self.with_ctx(|ctx| {
            let logins = ctx.logins(false, GH_MS);
            let login = login_for(&accounts, owner, &logins);
            let token = ctx.token(&login, GH_MS);
            (login, token)
        })
    }

    /// `results.json`'s owner → login map, as it stands (empty where it doesn't read).
    fn accounts(&self) -> BTreeMap<String, String> {
        std::fs::read_to_string(self.dir.join("results.json"))
            .ok()
            .and_then(|text| parse_shared(&text).ok())
            .map(|shared| shared.accounts)
            .unwrap_or_default()
    }

    /// The PR asked again, so its card shows what an action did.
    fn refresh(&self, url: &str) {
        let (Some(files), Some(pr)) = (&self.files, ref_from_url(url)) else {
            return;
        };
        self.with_ctx(|ctx| refresh(files, ctx, &[pr]));
    }

    /// Runs a job to its end.
    pub fn run(&self, job: Job) -> Done {
        let url = job.url().to_owned();
        let (number, outcome) = match job {
            Job::Merge { number, run, .. } => (number, self.merge(&url, run)),
            Job::Comment { number, text, .. } => (number, self.comment(&url, number, &text)),
            Job::Switch {
                number,
                owner,
                login,
                ..
            } => (number, self.switch(&url, owner, login)),
            Job::Reply {
                number,
                thread,
                text,
                ..
            } => (number, self.reply(&url, thread, &text)),
            Job::Resolve {
                number,
                thread,
                resolve,
                ..
            } => (number, self.resolve(&url, thread, resolve)),
            Job::Approve { number, pr, .. } => {
                let outcome = match self.change(
                    &url,
                    &mutate_argv(),
                    &approve_input(&pr),
                    "approve",
                    UNSURE_CHANGE,
                ) {
                    Ok(login) => Outcome::Approved { login },
                    Err(error) => Outcome::ApproveFailed(error),
                };
                (number, outcome)
            }
            Job::Close { number, close, .. } => {
                let verb = if close { "close" } else { "reopen" };
                let outcome =
                    match self.change(&url, &close_argv(&url, close), "", verb, UNSURE_CLOSE) {
                        Ok(login) => Outcome::Closed { close, login },
                        Err(error) => Outcome::CloseFailed(error),
                    };
                (number, outcome)
            }
            Job::Rerun { number, .. } => (number, self.rerun(&url, number)),
            Job::Collect {
                kind,
                facts,
                failing,
            } => (facts.number, self.collect(kind, &facts, &failing)),
        };
        Done {
            url,
            number,
            outcome,
        }
    }

    /// `owner` remembered as `login` in `results.json`, and the PR asked again as it, so its
    /// card shows what that login sees and may do.
    fn switch(&self, url: &str, owner: String, login: String) -> Outcome {
        let Some(files) = &self.files else {
            return Outcome::SwitchFailed("With --no-poll nothing is written".to_owned());
        };
        if let Err(error) = files.remember(&owner, &login) {
            return Outcome::SwitchFailed(format!("Couldn't write results.json: {error}"));
        }
        self.refresh(url);
        // A login that can't see the PR: the fetch found it as another, and kept that one.
        match self.accounts().remove(&owner) {
            Some(kept) if kept != login => Outcome::PutBack { owner, login, kept },
            _ => Outcome::Switched { owner, login },
        }
    }

    /// `gh pr merge`, and where GitHub's side gave up (a 502, a gateway timeout), the PR read
    /// again `RETRY_MS` on to tell whether it went through (`afterServerError`).
    fn merge(&self, url: &str, run: MergeRun) -> Outcome {
        let (_, token) = self.token_for(&repo_of(url));
        let token = match token {
            Ok(token) => token,
            Err(error) => {
                return Outcome::MergeFailed {
                    run,
                    error: MergeError {
                        message: error,
                        is_method_refused: false,
                    },
                };
            }
        };
        let call = |argv: &[String]| self.gh.run(argv, Some(&token), ms(GH_MS));
        let merged = |ran: Result<Ran, _>| match ran {
            Ok(ran) if ran.is_ok() => Ok(()),
            Ok(ran) => Err(said(&ran)),
            Err(_) => Err("gh pr merge did not finish".to_owned()),
        };
        let result = merged(call(&merge_argv(url, run))).or_else(|text| {
            if !is_server_error(&text) {
                return Err(text);
            }
            self.clock.sleep(RETRY_MS);
            let read = call(&merge_read_argv(url))
                .ok()
                .filter(Ran::is_ok)
                .and_then(|ran| merge_read_of(&ran.stdout));
            match after_server_error(run, read.as_ref()) {
                After::Done => Ok(()),
                After::Retry => merged(call(&merge_argv(url, run))),
                After::Failed => Err(text),
            }
        });
        self.refresh(url);
        match result {
            Ok(()) => Outcome::Merged(run),
            Err(text) => Outcome::MergeFailed {
                run,
                error: merge_error(&text),
            },
        }
    }

    /// The head commit's runs read, and each that holds a job to run again started again
    /// (`gh run rerun`), as the repo's login; then the PR asked again, so its checks show
    /// queued.
    fn rerun(&self, url: &str, number: u64) -> Outcome {
        let repo = repo_of(url);
        let (login, token) = self.token_for(&repo);
        let token = match token {
            Ok(token) => token,
            Err(error) => return Outcome::RerunFailed(error),
        };
        let pr = PrRef {
            url: url.to_owned(),
            repo: repo.clone(),
            number,
        };
        let runs = match self
            .graphql(&rerun_query(&pr), &token)
            .and_then(|ran| parse_reruns(&ran.stdout, &ran.stderr))
        {
            Ok(runs) => runs,
            Err(error) => return Outcome::RerunFailed(error),
        };
        let (mut ran, mut refused) = (Vec::new(), Vec::new());
        for run in &runs {
            match self
                .gh
                .run(&rerun_argv(&repo, run), Some(&token), ms(GH_MS))
            {
                Ok(done) if done.is_ok() => ran.push(run.words()),
                Ok(done) => refused.push(format!(
                    "{}: {}",
                    run.workflow,
                    prw_core::comment::refused(&said(&done), &login, "run it again")
                )),
                Err(_) => refused.push(format!("{}: gh run rerun did not finish", run.workflow)),
            }
        }
        if !runs.is_empty() {
            self.refresh(url);
        }
        Outcome::Reran { ran, refused }
    }

    /// A comment on the PR's conversation, its text on stdin (`comment_input`).
    fn comment(&self, url: &str, number: u64, text: &str) -> Outcome {
        let argv = comment_argv(&repo_of(url), number);
        match self.change(url, &argv, &comment_input(text), "comment", UNSURE) {
            Ok(login) => Outcome::Commented { login },
            Err(error) => Outcome::CommentFailed(error),
        }
    }

    fn reply(&self, url: &str, thread: OnThread, text: &str) -> Outcome {
        match self.change(
            url,
            &mutate_argv(),
            &reply_input(&thread.id, text),
            "reply",
            UNSURE,
        ) {
            Ok(login) => Outcome::Replied { thread, login },
            Err(error) => Outcome::ReplyFailed { thread, error },
        }
    }

    fn resolve(&self, url: &str, thread: OnThread, resolve: bool) -> Outcome {
        let verb = if resolve { "resolve" } else { "reopen" };
        match self.change(
            url,
            &mutate_argv(),
            &resolve_input(&thread.id, resolve),
            verb,
            UNSURE_CHANGE,
        ) {
            Ok(_) => Outcome::Resolved { thread, resolve },
            Err(error) => Outcome::ResolveFailed { thread, error },
        }
    }

    /// A write (`argv`, its body `input` on stdin), as the repo's login: the login, or why
    /// GitHub refused it ("Couldn't `verb`: …"), or `unsure` where the call was cut off.
    fn change(
        &self,
        url: &str,
        argv: &[String],
        input: &str,
        verb: &str,
        unsure: &str,
    ) -> Result<String, String> {
        let (login, token) = self.token_for(&repo_of(url));
        let token = token?;
        match self
            .gh
            .run_input(argv, Some(&token), Some(input), ms(GH_MS))
        {
            Ok(ran) if ran.is_ok() => {
                self.refresh(url);
                Ok(login)
            }
            Ok(ran) => Err(refused(&said(&ran), &login, verb)),
            Err(_) => {
                // It may have taken: the card is asked again too.
                self.refresh(url);
                Err(unsure.to_owned())
            }
        }
    }

    /// A GraphQL query, as `token`'s login.
    fn graphql(&self, query: &str, token: &str) -> Result<Ran, String> {
        let argv = ["api", "graphql", "-f", &format!("query={query}")].map(str::to_owned);
        self.gh
            .run(&argv, Some(token), ms(GH_MS))
            .map_err(|_| "gh api graphql did not finish".to_owned())
    }

    /// Runs a read for the full view to its end, as the repo's login.
    pub fn read(&self, read: Read) -> ReadDone {
        let url = read.url().to_owned();
        let got = match read {
            Read::Pr { url, repo, number } => Got::Pr(self.read_pr(&PrRef { url, repo, number })),
            Read::Logs { repo, jobs, .. } => Got::Logs(self.read_logs(&repo, &jobs)),
        };
        ReadDone { url, got }
    }

    fn read_pr(&self, pr: &PrRef) -> Result<Full, String> {
        let token = self.token_for(&pr.repo).1?;
        let ran = self.graphql(&full_query(pr), &token)?;
        parse_full(&ran.stdout, &ran.stderr, pr)
    }

    fn read_logs(&self, repo: &str, jobs: &[i64]) -> Vec<(i64, Result<String, String>)> {
        match self.token_for(repo).1 {
            Ok(token) => jobs
                .iter()
                .map(|&job| (job, self.log_text(job, repo, &token)))
                .collect(),
            Err(why) => jobs.iter().map(|&job| (job, Err(why.clone()))).collect(),
        }
    }

    /// What a Fix or Address prompt needs, read as the repo's login, and the prompt.
    fn collect(&self, kind: Kind, facts: &PrFacts, failing: &[String]) -> Outcome {
        let failed = |error: String| Outcome::PromptFailed { kind, error };
        let (_, token) = self.token_for(&facts.repo);
        let token = match token {
            Ok(token) => token,
            Err(error) => return failed(error),
        };
        let pr = PrRef {
            url: facts.url.clone(),
            repo: facts.repo.clone(),
            number: facts.number,
        };
        let graphql = |query: String| self.graphql(&query, &token);
        match kind {
            Kind::Fix => {
                let read = graphql(checks_query(&pr))
                    .and_then(|ran| parse_checks(&ran.stdout, &ran.stderr, &pr));
                // The names the card shows, and the PR's checks page, are still worth handing on.
                let (failures, problem) = match read {
                    Ok(items) => (failing_checks(&items, &facts.repo), None),
                    Err(error) => (
                        failing.iter().map(|name| Failure::named(name)).collect(),
                        Some(format!("Couldn't read the checks: {error}")),
                    ),
                };
                if failures.is_empty() {
                    // With no checks read, nothing says none fail: the read's error stands.
                    if let Some(problem) = problem {
                        return failed(problem);
                    }
                    self.refresh(&facts.url);
                    return failed("No check is failing on the latest runs now".to_owned());
                }
                let wanted: Vec<i64> = to_fetch(&failures)
                    .iter()
                    .filter_map(|f| f.job_id)
                    .collect();
                let handoffs: Vec<Handoff> = failures
                    .into_iter()
                    .map(
                        |failure| match failure.job_id.filter(|id| wanted.contains(id)) {
                            None => Handoff::unfetched(failure),
                            Some(id) => self.log(failure, id, &facts.repo, &token),
                        },
                    )
                    .collect();
                let note: Vec<String> =
                    problem.into_iter().chain(log_problems(&handoffs)).collect();
                Outcome::Prompt {
                    kind,
                    text: fix_prompt(facts, &handoffs),
                    note: (!note.is_empty()).then(|| note.join("; ")),
                }
            }
            Kind::Address => {
                let read = graphql(threads_query(&pr))
                    .and_then(|ran| parse_threads(&ran.stdout, &ran.stderr));
                let threads = match read {
                    Ok(threads) => threads,
                    Err(error) => {
                        return failed(format!("Couldn't read the review threads: {error}"));
                    }
                };
                if open_threads(&threads.threads).is_empty() {
                    self.refresh(&facts.url);
                    return failed("No review thread is unresolved now".to_owned());
                }
                Outcome::Prompt {
                    kind,
                    text: address_prompt(facts, &threads.threads, threads.total),
                    note: None,
                }
            }
            // Nothing to read: the model copies it itself.
            Kind::Resolve => failed("Nothing to read for a conflict".to_owned()),
        }
    }

    /// A failed job's log, or why it wouldn't come.
    fn log(&self, failure: Failure, job: i64, repo: &str, token: &str) -> Handoff {
        let (log, error) = match self.log_text(job, repo, token) {
            Ok(log) => (Some(log), None),
            Err(error) => (None, Some(error)),
        };
        Handoff {
            failure,
            log,
            error,
        }
    }

    /// A job's failed steps' log as gh prints it, or why it wouldn't come.
    fn log_text(&self, job: i64, repo: &str, token: &str) -> Result<String, String> {
        match self.gh.run(&log_argv(job, repo), Some(token), ms(LOG_MS)) {
            Ok(ran) if ran.is_ok() => Ok(ran.stdout),
            Ok(ran) => Err(log_error(&said(&ran))),
            Err(_) => Err("gh run view did not finish".to_owned()),
        }
    }
}

/// `gh api graphql`, its body on stdin: what the full view's changes run.
fn mutate_argv() -> [String; 4] {
    MUTATE_ARGV.map(str::to_owned)
}

/// What gh said: its stderr, else its stdout.
fn said(ran: &Ran) -> String {
    if ran.stderr.trim().is_empty() {
        ran.stdout.clone()
    } else {
        ran.stderr.clone()
    }
}

/// A job on a thread of its own; what came of it on `done`.
pub fn start<G, C>(runner: &Arc<Runner<G, C>>, job: Job, done: Sender<Msg>)
where
    G: Gh + Send + Sync + 'static,
    C: Clock + Send + Sync + 'static,
{
    let runner = Arc::clone(runner);
    thread::spawn(move || {
        let _ = done.send(Msg::Done(runner.run(job)));
    });
}

/// A read on a thread of its own; what came of it on `done`.
pub fn start_read<G, C>(runner: &Arc<Runner<G, C>>, read: Read, done: Sender<Msg>)
where
    G: Gh + Send + Sync + 'static,
    C: Clock + Send + Sync + 'static,
{
    let runner = Arc::clone(runner);
    thread::spawn(move || {
        let _ = done.send(Msg::Read(runner.read(read)));
    });
}

/// What a demo reads for the full view: `full/<owner>-<repo>-<n>.json` beside its cache, a
/// reply as gh prints it, and `full/<job>.log` for a job's log.
pub fn demo_read(read: Read, dir: &Path) -> ReadDone {
    let full = dir.join("full");
    let url = read.url().to_owned();
    let got = match read {
        Read::Pr { url, repo, number } => {
            let file = full.join(format!("{}-{number}.json", repo.replace('/', "-")));
            let pr = PrRef { url, repo, number };
            Got::Pr(match std::fs::read_to_string(file) {
                Ok(reply) => parse_full(&reply, "", &pr),
                Err(_) => Err(format!("The demo has nothing more on #{number}")),
            })
        }
        Read::Logs { jobs, .. } => Got::Logs(
            jobs.into_iter()
                .map(|job| {
                    let log = std::fs::read_to_string(full.join(format!("{job}.log")))
                        .map_err(|_| "the demo has no log for it".to_owned());
                    (job, log)
                })
                .collect(),
        ),
    };
    ReadDone { url, got }
}

/// What a demo says to a job: it sends nothing to GitHub. A Fix prompt is still written, from
/// what the card knows (no logs), so the hand-off can be shown.
pub fn demo(job: Job) -> Done {
    const NOTHING: &str = "the demo sends nothing to GitHub";
    let url = job.url().to_owned();
    let (number, outcome) = match job {
        Job::Merge { number, run, .. } => (
            number,
            Outcome::MergeFailed {
                run,
                error: MergeError {
                    message: NOTHING.to_owned(),
                    is_method_refused: false,
                },
            },
        ),
        Job::Comment { number, .. } => (
            number,
            Outcome::CommentFailed(format!("Not posted: {NOTHING}")),
        ),
        Job::Switch { number, .. } => (
            number,
            Outcome::SwitchFailed("Not switched: the demo writes nothing".to_owned()),
        ),
        Job::Reply { number, thread, .. } => (
            number,
            Outcome::ReplyFailed {
                thread,
                error: format!("Not posted: {NOTHING}"),
            },
        ),
        Job::Resolve {
            number,
            thread,
            resolve,
            ..
        } => {
            let done = if resolve { "resolved" } else { "reopened" };
            (
                number,
                Outcome::ResolveFailed {
                    thread,
                    error: format!("Not {done}: {NOTHING}"),
                },
            )
        }
        Job::Approve { number, .. } => (
            number,
            Outcome::ApproveFailed(format!("Not approved: {NOTHING}")),
        ),
        Job::Close { number, close, .. } => {
            let done = if close { "closed" } else { "reopened" };
            (
                number,
                Outcome::CloseFailed(format!("Not {done}: {NOTHING}")),
            )
        }
        Job::Rerun { number, .. } => (
            number,
            Outcome::RerunFailed(format!("Nothing started: {NOTHING}")),
        ),
        Job::Collect {
            kind: Kind::Fix,
            facts,
            failing,
        } => {
            let handoffs: Vec<Handoff> = failing
                .iter()
                .map(|name| Handoff::unfetched(Failure::named(name)))
                .collect();
            (
                facts.number,
                Outcome::Prompt {
                    kind: Kind::Fix,
                    text: fix_prompt(&facts, &handoffs),
                    note: Some("the demo fetches no logs".to_owned()),
                },
            )
        }
        Job::Collect { kind, facts, .. } => (
            facts.number,
            Outcome::PromptFailed {
                kind,
                error: "the demo reads no review threads".to_owned(),
            },
        ),
    };
    Done {
        url,
        number,
        outcome,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gh::RunError;
    use crate::gh::fake::{Call, FakeClock, FakeGh, failed, ok};
    use prw_core::status::MergeMethod::Squash;
    use serde_json::{Value, json};
    use std::fs;

    const URL: &str = "https://github.com/acme/app/pull/7";

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("prw-actions-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("a temp dir");
            // acme's PRs are seen by hubot, not the active login.
            let results =
                json!({"fetchedAt": 1, "nextAt": 1, "prs": {}, "accounts": {"acme": "hubot"}});
            fs::write(dir.join("results.json"), results.to_string()).expect("results.json");
            Self(dir)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn starts(words: &'static [&'static str]) -> impl Fn(&Call) -> bool + 'static {
        move |call| {
            call.argv
                .iter()
                .map(String::as_str)
                .take(words.len())
                .eq(words.iter().copied())
        }
    }

    /// gh with two logins, each token its login's name.
    fn gh() -> FakeGh {
        let gh = FakeGh::default();
        gh.on(starts(&["auth", "status"]), |_| {
            ok("github.com\n  ✓ Logged in to github.com account ada (keyring)\n  - Active account: true\n  ✓ Logged in to github.com account hubot (keyring)\n  - Active account: false\n")
        });
        gh.on(starts(&["auth", "token"]), |call| {
            ok(format!(
                "{}\n",
                call.argv.last().cloned().unwrap_or_default()
            ))
        });
        gh
    }

    fn runner(gh: FakeGh, dir: &Dir) -> Runner<FakeGh, FakeClock> {
        Runner::new(gh, FakeClock::new(1_000, 1), dir.0.clone(), None)
    }

    fn calls(runner: &Runner<FakeGh, FakeClock>, first: &str) -> Vec<Call> {
        runner
            .gh
            .calls
            .borrow()
            .iter()
            .filter(|c| c.argv.first().is_some_and(|a| a == first))
            .cloned()
            .collect()
    }

    #[test]
    fn a_merge_goes_as_the_login_that_sees_the_repo() {
        let dir = Dir::new("merge");
        let gh = gh();
        gh.on(starts(&["pr", "merge"]), |_| ok(""));
        let runner = runner(gh, &dir);
        let done = runner.run(Job::Merge {
            url: URL.to_owned(),
            number: 7,
            run: MergeRun::Merge(Squash),
        });
        assert_eq!(done.outcome, Outcome::Merged(MergeRun::Merge(Squash)));
        let merges = calls(&runner, "pr");
        assert_eq!(merges.len(), 1);
        assert_eq!(merges[0].argv, ["pr", "merge", URL, "--squash"]);
        assert_eq!(merges[0].token.as_deref(), Some("hubot"));
    }

    #[test]
    fn a_server_error_reads_the_pr_again_and_a_merged_pr_counts_as_done() {
        let dir = Dir::new("server-error");
        let gh = gh();
        gh.on(starts(&["pr", "merge"]), |_| {
            failed(
                "",
                "HTTP 502: Bad Gateway (https://api.github.com/graphql)\n",
            )
        });
        gh.on(starts(&["pr", "view"]), |_| {
            ok(r#"{"state":"MERGED","autoMergeRequest":null}"#)
        });
        let runner = runner(gh, &dir);
        let done = runner.run(Job::Merge {
            url: URL.to_owned(),
            number: 7,
            run: MergeRun::Merge(Squash),
        });
        assert_eq!(done.outcome, Outcome::Merged(MergeRun::Merge(Squash)));
        assert_eq!(
            calls(&runner, "pr").len(),
            2,
            "one merge, one read, no second merge"
        );
    }

    #[test]
    fn a_server_error_on_a_pr_still_open_runs_once_more() {
        let dir = Dir::new("retry");
        let gh = gh();
        gh.on(starts(&["pr", "merge"]), |_| ok(""));
        gh.once(starts(&["pr", "merge"]), |_| {
            failed("", "gh: Server Error (HTTP 502)\n")
        });
        gh.on(starts(&["pr", "view"]), |_| {
            ok(r#"{"state":"OPEN","autoMergeRequest":null}"#)
        });
        let runner = runner(gh, &dir);
        let done = runner.run(Job::Merge {
            url: URL.to_owned(),
            number: 7,
            run: MergeRun::Merge(Squash),
        });
        assert_eq!(done.outcome, Outcome::Merged(MergeRun::Merge(Squash)));
        assert_eq!(calls(&runner, "pr").len(), 3);
    }

    #[test]
    fn a_refused_method_says_so() {
        let dir = Dir::new("refused");
        let gh = gh();
        gh.on(starts(&["pr", "merge"]), |_| {
            failed(
                "",
                "GraphQL: Squash merges are not allowed on this repository. (mergePullRequest)\n",
            )
        });
        let done = runner(gh, &dir).run(Job::Merge {
            url: URL.to_owned(),
            number: 7,
            run: MergeRun::Merge(Squash),
        });
        let Outcome::MergeFailed { error, .. } = done.outcome else {
            panic!("refused: {:?}", done.outcome);
        };
        assert!(error.is_method_refused);
        assert_eq!(
            error.message,
            "Squash merges are not allowed on this repository."
        );
    }

    #[test]
    fn a_comment_goes_on_stdin_never_in_argv() {
        let dir = Dir::new("comment");
        let gh = gh();
        gh.on(starts(&["api", "--method"]), |_| {
            ok(r#"{"html_url": "https://github.com/acme/app/pull/7#issuecomment-1"}"#)
        });
        let runner = runner(gh, &dir);
        let text = "@ops -F body=x\nthanks";
        let done = runner.run(Job::Comment {
            url: URL.to_owned(),
            number: 7,
            text: text.to_owned(),
        });
        assert_eq!(
            done.outcome,
            Outcome::Commented {
                login: "hubot".to_owned()
            }
        );
        let posts = calls(&runner, "api");
        assert_eq!(
            posts[0].argv,
            [
                "api",
                "--method",
                "POST",
                "repos/acme/app/issues/7/comments",
                "--input",
                "-"
            ]
        );
        assert_eq!(
            posts[0].input.as_deref(),
            Some(json!({"body": text}).to_string().as_str())
        );
        assert!(posts[0].argv.iter().all(|a| !a.contains("ops")));
    }

    #[test]
    fn a_comment_cut_off_may_have_posted_and_says_so() {
        let dir = Dir::new("comment-timeout");
        let gh = gh();
        gh.on(starts(&["api", "--method"]), |_| Err(RunError::Timeout));
        let done = runner(gh, &dir).run(Job::Comment {
            url: URL.to_owned(),
            number: 7,
            text: "hi".to_owned(),
        });
        assert_eq!(done.outcome, Outcome::CommentFailed(UNSURE.to_owned()));
    }

    fn thread() -> OnThread {
        OnThread {
            id: "PRRT_1".to_owned(),
            spot: "src/a.ts:4".to_owned(),
        }
    }

    #[test]
    fn a_reply_goes_on_stdin_as_the_repos_login() {
        let dir = Dir::new("reply");
        let gh = gh();
        gh.on(starts(&MUTATE_ARGV), |_| ok(r#"{"data": {}}"#));
        let runner = runner(gh, &dir);
        let text = "-F body=x\nthanks";
        let done = runner.run(Job::Reply {
            url: URL.to_owned(),
            number: 7,
            thread: thread(),
            text: text.to_owned(),
        });
        assert_eq!(
            done.outcome,
            Outcome::Replied {
                thread: thread(),
                login: "hubot".to_owned()
            }
        );
        let posts = calls(&runner, "api");
        assert_eq!(posts[0].argv, MUTATE_ARGV);
        assert_eq!(posts[0].token.as_deref(), Some("hubot"));
        let input: Value = serde_json::from_str(posts[0].input.as_deref().unwrap_or_default())
            .expect("a JSON body");
        assert_eq!(
            input["variables"],
            json!({"thread": "PRRT_1", "body": text})
        );
    }

    #[test]
    fn a_refused_change_says_githubs_words_and_a_cut_off_one_says_it_may_have_taken() {
        let dir = Dir::new("refused-change");
        let gh = gh();
        gh.on(starts(&MUTATE_ARGV), |call| {
            if call.input.as_deref().is_some_and(|i| i.contains("APPROVE")) {
                Err(RunError::Timeout)
            } else {
                failed("", "gh: Resource not accessible by integration\n")
            }
        });
        let runner = runner(gh, &dir);
        let done = runner.run(Job::Resolve {
            url: URL.to_owned(),
            number: 7,
            thread: thread(),
            resolve: false,
        });
        assert_eq!(
            done.outcome,
            Outcome::ResolveFailed {
                thread: thread(),
                error: "Couldn't reopen: Resource not accessible by integration".into()
            }
        );
        let done = runner.run(Job::Approve {
            url: URL.to_owned(),
            number: 7,
            pr: "PR_7".to_owned(),
        });
        assert_eq!(
            done.outcome,
            Outcome::ApproveFailed(UNSURE_CHANGE.to_owned())
        );
    }

    #[test]
    fn x_closes_or_reopens_as_the_login_that_sees_the_repo() {
        let dir = Dir::new("close");
        let gh = gh();
        gh.on(starts(&["pr", "close"]), |_| ok(""));
        gh.on(starts(&["pr", "reopen"]), |_| {
            failed("", "gh: Could not reopen: the branch was deleted\n")
        });
        let runner = runner(gh, &dir);
        let done = runner.run(Job::Close {
            url: URL.to_owned(),
            number: 7,
            close: true,
        });
        assert_eq!(
            done.outcome,
            Outcome::Closed {
                close: true,
                login: "hubot".to_owned()
            }
        );
        let closes = calls(&runner, "pr");
        assert_eq!(closes[0].argv, ["pr", "close", URL]);
        assert_eq!(closes[0].token.as_deref(), Some("hubot"));
        let done = runner.run(Job::Close {
            url: URL.to_owned(),
            number: 7,
            close: false,
        });
        assert!(
            matches!(&done.outcome, Outcome::CloseFailed(error) if error.starts_with("Couldn't reopen")),
            "{:?}",
            done.outcome
        );
    }

    #[test]
    fn r_reads_the_heads_runs_and_starts_each_with_a_job_to_run_again() {
        let dir = Dir::new("rerun");
        let gh = gh();
        let suite = |id: i64, name: &str, conclusion: &str, jobs: &[&str]| {
            json!({
                "status": "COMPLETED", "conclusion": conclusion,
                "workflowRun": {"databaseId": id, "workflow": {"name": name}},
                "checkRuns": {"nodes": jobs.iter().map(|c| json!({"conclusion": c})).collect::<Vec<_>>()},
            })
        };
        let suites = json!([
            suite(11, "CI", "SUCCESS", &["SUCCESS", "SKIPPED"]),
            suite(12, "Deploy", "FAILURE", &["FAILURE"]),
            suite(13, "Lint", "SUCCESS", &["SUCCESS"]),
            suite(14, "Docs", "FAILURE", &["FAILURE"]),
        ]);
        let reply = json!({"data": {"p0": {"pullRequest": {"commits": {"nodes": [{"commit": {"checkSuites": {"nodes": suites}}}]}}}}});
        gh.on(starts(&["api", "graphql"]), move |_| ok(reply.to_string()));
        gh.on(starts(&["run", "rerun"]), |call| {
            if call.argv.get(2).is_some_and(|id| id == "14") {
                failed("", "gh: HTTP 403: Must have admin rights to Repository.\n")
            } else {
                ok("")
            }
        });
        let runner = runner(gh, &dir);
        let done = runner.run(Job::Rerun {
            url: URL.to_owned(),
            number: 7,
        });
        let Outcome::Reran { ran, refused } = &done.outcome else {
            panic!("{:?}", done.outcome);
        };
        assert_eq!(ran, &["CI (whole run)", "Deploy (failed jobs)"]);
        assert_eq!(refused.len(), 1);
        assert!(
            refused[0].starts_with("Docs: Couldn't run it again"),
            "{refused:?}"
        );
        let reruns = calls(&runner, "run");
        assert_eq!(
            reruns.iter().map(|c| c.argv.clone()).collect::<Vec<_>>(),
            [
                vec!["run", "rerun", "11", "--repo", "acme/app"],
                vec!["run", "rerun", "12", "--repo", "acme/app", "--failed"],
                vec!["run", "rerun", "14", "--repo", "acme/app", "--failed"],
            ]
        );
        assert!(reruns.iter().all(|c| c.token.as_deref() == Some("hubot")));
    }

    #[test]
    fn r_on_a_pr_it_cant_read_says_why_and_starts_nothing() {
        let dir = Dir::new("rerun-missing");
        let gh = gh();
        gh.on(starts(&["api", "graphql"]), |_| {
            ok(r#"{"data": {"p0": {"pullRequest": null}}}"#)
        });
        let runner = runner(gh, &dir);
        let done = runner.run(Job::Rerun {
            url: URL.to_owned(),
            number: 7,
        });
        assert_eq!(
            done.outcome,
            Outcome::RerunFailed("Not found, or this gh account can't see it".to_owned())
        );
        assert_eq!(calls(&runner, "run"), []);
    }

    fn facts() -> PrFacts {
        PrFacts {
            repo: "acme/app".to_owned(),
            number: 7,
            title: "Retry uploads".to_owned(),
            url: URL.to_owned(),
            branch: "feat/retry".to_owned(),
            base: "main".to_owned(),
            author: "ada".to_owned(),
        }
    }

    #[test]
    fn fix_reads_the_checks_and_the_failed_jobs_log() {
        let dir = Dir::new("fix");
        let gh = gh();
        let check = json!({"__typename": "CheckRun", "name": "unit", "status": "COMPLETED", "conclusion": "FAILURE", "databaseId": 901,
            "checkSuite": {"workflowRun": {"workflow": {"name": "CI"}}}});
        let reply = json!({"data": {"p0": {"pullRequest": {"number": 7, "commits": {"nodes": [{"commit": {
            "statusCheckRollup": {"contexts": {"nodes": [check]}}}}]}}}}});
        gh.on(starts(&["api", "graphql"]), move |_| ok(reply.to_string()));
        gh.on(starts(&["run", "view"]), |_| {
            ok("unit\tRun tests\t2026-10-07T11:00:00.1Z Error: boom\n")
        });
        let runner = runner(gh, &dir);
        let done = runner.run(Job::Collect {
            kind: Kind::Fix,
            facts: facts(),
            failing: vec!["unit".to_owned()],
        });
        let Outcome::Prompt { text, note, .. } = done.outcome else {
            panic!("a prompt: {:?}", done.outcome);
        };
        assert!(text.starts_with("CI failed on acme/app#7: Retry uploads\n"));
        assert!(text.contains(
            "CI / unit log (all 1 lines, failed step \"Run tests\"):\n```\nError: boom\n```"
        ));
        assert_eq!(note, None);
        let logs = calls(&runner, "run");
        assert_eq!(
            logs[0].argv,
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
        assert_eq!(logs[0].token.as_deref(), Some("hubot"));
    }

    #[test]
    fn a_read_goes_as_the_repos_login_for_the_pr_whole_and_each_jobs_log() {
        let dir = Dir::new("read");
        let gh = gh();
        let reply = json!({"data": {"p0": {"pullRequest": {"body": "Retries uploads.",
            "reviewThreads": {"totalCount": 1, "nodes": [{"id": "T1", "path": "a.ts"}]},
            "commits": {"nodes": [{"commit": {"statusCheckRollup": {"contexts": {"nodes": []}}}}]}}}}});
        gh.on(starts(&["api", "graphql"]), move |_| ok(reply.to_string()));
        gh.on(starts(&["run", "view", "--job", "901"]), |_| {
            ok("unit\tRun\tt Error: boom\n")
        });
        gh.on(starts(&["run", "view"]), |_| {
            failed("", "HTTP 404: Not Found\n")
        });
        let runner = runner(gh, &dir);
        let done = runner.read(Read::Pr {
            url: URL.to_owned(),
            repo: "acme/app".to_owned(),
            number: 7,
        });
        let Got::Pr(Ok(full)) = done.got else {
            panic!("the PR: {:?}", done.got);
        };
        assert_eq!(
            (full.body.as_str(), full.threads[0].id.as_str()),
            ("Retries uploads.", "T1")
        );
        let query = &calls(&runner, "api")[0];
        assert!(query.argv[3].contains("pullRequest(number: 7)"));
        assert_eq!(query.token.as_deref(), Some("hubot"));
        let done = runner.read(Read::Logs {
            url: URL.to_owned(),
            repo: "acme/app".to_owned(),
            jobs: vec![901, 902],
        });
        let Got::Logs(logs) = done.got else {
            panic!("the logs: {:?}", done.got);
        };
        assert_eq!(logs[0], (901, Ok("unit\tRun\tt Error: boom\n".to_owned())));
        assert!(logs[1].1.is_err());
        assert!(
            calls(&runner, "run")
                .iter()
                .all(|c| c.token.as_deref() == Some("hubot"))
        );
    }

    #[test]
    fn checks_that_wont_read_still_hand_on_the_cards_names() {
        let dir = Dir::new("fix-unread");
        let gh = gh();
        gh.on(starts(&["api", "graphql"]), |_| {
            failed("", "gh: Something went wrong\n")
        });
        let done = runner(gh, &dir).run(Job::Collect {
            kind: Kind::Fix,
            facts: facts(),
            failing: vec!["unit".to_owned()],
        });
        let Outcome::Prompt { text, note, .. } = done.outcome else {
            panic!("a prompt: {:?}", done.outcome);
        };
        assert!(text.contains("- unit · failed"));
        assert_eq!(
            note.as_deref(),
            Some("Couldn't read the checks: gh: Something went wrong")
        );
    }

    #[test]
    fn checks_that_wont_read_with_no_names_say_why_not_that_none_fail() {
        let dir = Dir::new("fix-unread-unnamed");
        let gh = gh();
        gh.on(starts(&["api", "graphql"]), |_| {
            failed("", "gh: Something went wrong\n")
        });
        let done = runner(gh, &dir).run(Job::Collect {
            kind: Kind::Fix,
            facts: facts(),
            failing: Vec::new(),
        });
        assert_eq!(
            done.outcome,
            Outcome::PromptFailed {
                kind: Kind::Fix,
                error: "Couldn't read the checks: gh: Something went wrong".to_owned(),
            }
        );
    }

    #[test]
    fn address_reads_the_threads_or_says_none_is_open() {
        let dir = Dir::new("address");
        let gh = gh();
        let thread = |resolved: bool| {
            json!({"data": {"p0": {"pullRequest": {"reviewThreads": {"totalCount": 1, "nodes": [
                {"isResolved": resolved, "isOutdated": false, "path": "src/upload.ts", "line": 3,
                 "comments": {"totalCount": 1, "nodes": [{"author": {"login": "kai"}, "body": "Cap the delay?", "url": "https://github.com/acme/app/pull/7#discussion_r1"}]}}
            ]}}}}})
        };
        let open = thread(false);
        gh.once(starts(&["api", "graphql"]), move |_| ok(open.to_string()));
        let resolved = thread(true);
        gh.on(starts(&["api", "graphql"]), move |_| {
            ok(resolved.to_string())
        });
        let runner = runner(gh, &dir);
        let collect = || Job::Collect {
            kind: Kind::Address,
            facts: facts(),
            failing: Vec::new(),
        };
        let Outcome::Prompt { text, .. } = runner.run(collect()).outcome else {
            panic!("a prompt");
        };
        assert!(text.contains("1. src/upload.ts:3 · https://github.com/acme/app/pull/7#discussion_r1\n   @kai: Cap the delay?"));
        assert_eq!(
            runner.run(collect()).outcome,
            Outcome::PromptFailed {
                kind: Kind::Address,
                error: "No review thread is unresolved now".to_owned()
            }
        );
    }

    #[test]
    fn no_token_for_the_login_sends_nothing_as_another() {
        let dir = Dir::new("no-token");
        let gh = FakeGh::default();
        gh.on(starts(&["auth", "status"]), |_| {
            ok("github.com\n  ✓ Logged in to github.com account hubot (keyring)\n  - Active account: true\n")
        });
        gh.on(starts(&["auth", "token"]), |_| {
            failed("", "no oauth token found for github.com account hubot\n")
        });
        let runner = runner(gh, &dir);
        let done = runner.run(Job::Merge {
            url: URL.to_owned(),
            number: 7,
            run: MergeRun::Merge(Squash),
        });
        assert!(matches!(done.outcome, Outcome::MergeFailed { .. }));
        assert_eq!(calls(&runner, "pr").len(), 0);
    }

    #[test]
    fn the_demo_sends_nothing() {
        let done = demo(Job::Merge {
            url: URL.to_owned(),
            number: 7,
            run: MergeRun::Merge(Squash),
        });
        assert!(
            matches!(done.outcome, Outcome::MergeFailed { error, .. } if error.message == "the demo sends nothing to GitHub")
        );
        let done = demo(Job::Collect {
            kind: Kind::Fix,
            facts: facts(),
            failing: vec!["unit".to_owned()],
        });
        assert!(
            matches!(done.outcome, Outcome::Prompt { text, .. } if text.contains("- unit · failed"))
        );
    }

    #[test]
    fn a_switch_writes_the_owners_login_and_asks_the_pr_again_as_it() {
        let dir = Dir::new("switch");
        let gh = gh();
        gh.on(starts(&["api", "graphql"]), |_| ok("{}"));
        let files = Files::new(dir.0.clone(), "prw-test".into());
        let runner = Runner::new(gh, FakeClock::new(1_000, 1), dir.0.clone(), Some(files));
        let job = Job::Switch {
            url: URL.to_owned(),
            number: 7,
            owner: "acme".to_owned(),
            login: "ada".to_owned(),
        };
        let done = runner.run(job.clone());
        assert_eq!(
            done.outcome,
            Outcome::Switched {
                owner: "acme".to_owned(),
                login: "ada".to_owned()
            }
        );
        let results: Value =
            serde_json::from_str(&fs::read_to_string(dir.0.join("results.json")).unwrap()).unwrap();
        assert_eq!(results["accounts"]["acme"], "ada");
        // Asked again, as the login switched to.
        let calls = runner.gh.calls.borrow();
        let asked = calls
            .iter()
            .find(|c| c.argv.first().is_some_and(|a| a == "api"));
        assert_eq!(asked.and_then(|c| c.token.as_deref()), Some("ada"));
        drop(calls);
        // With --no-poll nothing is written.
        let outcome = self::runner(self::gh(), &dir).run(job).outcome;
        assert_eq!(
            outcome,
            Outcome::SwitchFailed("With --no-poll nothing is written".to_owned())
        );
    }
}
