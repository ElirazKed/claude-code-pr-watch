//! Asking GitHub, as the mod's round does (`hooks/register.tsx`: `logins`, `tokenFor`,
//! `query`, `fetchRefs`, `askRest`): one batched query per gh login, each with that login's
//! own token, PRs one login can't see asked of the others, a timeout asked once more, and
//! the limits honoured. Every decision is `prw_core`'s; this is the order of the calls.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;

use prw_core::accounts::{
    Answer, Asked, Logins, best_login, every_login, is_due, login_for, logins_of, not_found_as,
    owners_below_write, rank_of, token_error, untried_by,
};
use prw_core::batch::{
    BATCH_SIZE, CallFailure, CallLog, Http, LOG_LINES, PrRef, append_log, build_query, call_budget,
    failure_of, log_line, parse_http, parse_reply, rate_of, retry_after_of,
};
use prw_core::poll::Failure;
use prw_core::protocol::{LoginRates, RateLimit};
use prw_core::rate::{Rates, bucket_of, is_rest_due, low_until, rest_of, with_seen};
use serde_json::{Map, Value};

use crate::gh::{Clock, Gh, RunError, ms};

/// The longest any one `gh` call may take (ms).
pub const GH_MS: i64 = 20_000;
/// A batch that failed on GitHub having a moment is asked once more this long after (ms).
const RETRY_MS: i64 = 2_000;
/// `/rate_limit` answers at once, or isn't worth waiting for (ms).
const REST_CALL_MS: i64 = 5_000;
/// How long everyone holds off after a secondary limit that names no retry-after (ms).
const LIMIT_WAIT_MS: i64 = 60_000;

/// A login's token as gh gave it, or what the card says where it wouldn't.
pub type Token = Result<String, String>;

/// gh's logins and tokens as last read, and when (`loginsKept`, `tokens`, `tokenMisses`).
#[derive(Debug, Default)]
pub struct Auth {
    kept: Logins,
    asked: Option<Asked>,
    tokens: HashMap<String, (i64, String)>,
    misses: HashMap<String, (i64, String)>,
}

/// What a round asks with: `gh`, the clock, what it knows of gh's logins, and where its
/// failures are logged (`prw-errors.log`: the mod's `errors.log` is its own).
pub struct Ctx<'a, G: Gh, C: Clock> {
    pub gh: &'a G,
    pub clock: &'a C,
    pub auth: &'a mut Auth,
    pub log: Option<PathBuf>,
}

fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|&a| a.to_owned()).collect()
}

impl<G: Gh, C: Clock> Ctx<'_, G, C> {
    /// gh's logins (`logins`): read again once [`is_due`] says (or with `is_fresh`, before a
    /// PR is called not found); the last good reading stands meanwhile.
    pub fn logins(&mut self, is_fresh: bool, timeout: i64) -> Logins {
        let at = self.clock.now();
        if is_fresh || is_due(self.auth.asked, at) {
            let args = argv(&["auth", "status", "--hostname", "github.com"]);
            let read = self
                .gh
                .run(&args, None, ms(timeout))
                .ok()
                .and_then(|r| logins_of(&format!("{}\n{}", r.stdout, r.stderr)));
            self.auth.asked = Some(Asked {
                at: self.clock.now(),
                is_ok: read.is_some(),
            });
            if let Some(read) = read {
                self.auth.kept = read;
            }
        }
        self.auth.kept.clone()
    }

    /// A login's own token (`tokenFor`), asked of gh again every few minutes; where gh won't
    /// give it then, the last one it gave stands, and where it never gave one, that is said.
    pub fn token(&mut self, login: &str, timeout: i64) -> Token {
        if login.is_empty() {
            return Err(token_error(""));
        }
        let at = self.clock.now();
        let good = self.auth.tokens.get(login).cloned();
        let miss = self.auth.misses.get(login).cloned();
        let is_kept = good.as_ref().is_some_and(|(was, _)| {
            !is_due(
                Some(Asked {
                    at: *was,
                    is_ok: true,
                }),
                at,
            )
        }) || miss.as_ref().is_some_and(|(was, _)| {
            !is_due(
                Some(Asked {
                    at: *was,
                    is_ok: false,
                }),
                at,
            )
        });
        if is_kept {
            return good
                .map(|(_, token)| token)
                .ok_or_else(|| miss.map_or_else(|| token_error(login), |(_, error)| error));
        }
        let args = argv(&["auth", "token", "--hostname", "github.com", "--user", login]);
        let res = self.gh.run(&args, None, ms(timeout));
        let done_at = self.clock.now();
        if let Ok(ran) = &res {
            let said = ran.stdout.trim();
            if ran.is_ok() && !said.is_empty() {
                self.auth
                    .tokens
                    .insert(login.to_owned(), (done_at, said.to_owned()));
                self.auth.misses.remove(login);
                return Ok(said.to_owned());
            }
        }
        let error = token_error(login);
        self.auth
            .misses
            .insert(login.to_owned(), (done_at, error.clone()));
        let why = match &res {
            Err(_) => "gh auth token did not finish".to_owned(),
            Ok(ran) => ran
                .stderr
                .trim()
                .lines()
                .next()
                .filter(|l| !l.is_empty())
                .map_or_else(
                    || format!("gh auth token exited {:?}", ran.code),
                    str::to_owned,
                ),
        };
        let none = Http::default();
        self.log_failure(&CallLog {
            login,
            size: 0,
            ms: done_at - at,
            http: &none,
            failure: CallFailure::Other,
            error: &format!("{error}: {why}"),
        });
        good.map(|(_, token)| token).ok_or(error)
    }

    /// Best effort: a log that can't be read starts over, and one that can't be written is
    /// skipped.
    fn log_failure(&self, call: &CallLog<'_>) {
        let Some(path) = &self.log else { return };
        let log = fs::read_to_string(path).unwrap_or_default();
        let line = log_line(self.clock.now(), call);
        let _ = fs::write(path, append_log(&log, &line, LOG_LINES));
    }

    /// One batch's call, timed, and what GitHub said (`askBatch`). A call past its timeout
    /// fails as a timeout, rather than losing the round.
    fn ask_batch(&self, part: &[PrRef], args: &[String], token: &str, timeout: i64) -> BatchCall {
        let started = self.clock.now();
        let (stdout, stderr) = match self.gh.run(args, Some(token), ms(timeout)) {
            Ok(ran) => (ran.stdout, ran.stderr),
            Err(RunError::Timeout) => (
                String::new(),
                format!(
                    "gh api graphql did not finish: timeout after {}s",
                    (timeout + 500) / 1_000
                ),
            ),
            Err(RunError::Spawn(why)) => (String::new(), format!("gh didn't start: {why}")),
        };
        let http = parse_http(&stdout);
        let reply = parse_reply(&http.body, &stderr, part);
        BatchCall {
            http,
            reply,
            ms: self.clock.now() - started,
        }
    }

    /// Why a failed call failed (`None`: it didn't), logged on the way (`failureIn`).
    fn failure_in(&self, login: &str, size: usize, call: &BatchCall) -> Option<CallFailure> {
        let error = call.reply.error.as_deref()?;
        let failure = failure_of(error, &call.http);
        self.log_failure(&CallLog {
            login,
            size,
            ms: call.ms,
            http: &call.http,
            failure,
            error,
        });
        Some(failure)
    }

    /// `refs` asked as `login`, in batches, inside the lease (`query`). A batch starts only
    /// with time left and takes no more than is left; the rest are `left` for the next round.
    /// A secondary limit stops the asking (`wait_ms`), spent points stop this login's.
    pub fn query(&mut self, refs: &[PrRef], login: &str, until: i64) -> Query {
        let mut out = Query::default();
        let Some(budget) = call_budget(until, self.clock.now(), GH_MS) else {
            out.left.extend_from_slice(refs);
            return out;
        };
        let token = match self.token(login, budget) {
            Ok(token) => token,
            Err(error) => {
                for r in refs {
                    out.failed.insert(r.url.clone(), error.clone());
                }
                return out;
            }
        };
        let parts: Vec<&[PrRef]> = refs.chunks(BATCH_SIZE).collect();
        for (i, part) in parts.iter().enumerate() {
            let Some(budget) = call_budget(until, self.clock.now(), GH_MS) else {
                out.left
                    .extend(parts[i..].iter().flat_map(|p| p.iter().cloned()));
                break;
            };
            let args = vec![
                "api".to_owned(),
                "graphql".to_owned(),
                "-f".to_owned(),
                format!("query={}", build_query(part)),
                "-i".to_owned(),
            ];
            let mut call = self.ask_batch(part, &args, &token, budget);
            let mut failure = self.failure_in(login, part.len(), &call);
            if failure == Some(CallFailure::Timeout)
                && let Some(retry) = call_budget(until, self.clock.now() + RETRY_MS, GH_MS)
            {
                self.clock.sleep(RETRY_MS);
                call = self.ask_batch(part, &args, &token, retry);
                failure = self.failure_in(login, part.len(), &call);
            }
            let is_limit = matches!(
                failure,
                Some(CallFailure::SecondaryLimit | CallFailure::PrimaryLimit)
            );
            if let (Some(error), false) = (&call.reply.error, is_limit) {
                for r in *part {
                    out.failed.insert(r.url.clone(), error.clone());
                }
            }
            out.found.extend(call.reply.found);
            out.missing.extend(call.reply.missing);
            if let Some(rate) = rate_of(&call.http.headers).or(call.reply.rate) {
                out.rate = Some(rate);
                out.rate_at = self.clock.now();
            }
            if failure == Some(CallFailure::SecondaryLimit) {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "a retry-after is seconds, far inside i64 as ms"
                )]
                let wait = retry_after_of(&call.http.headers)
                    .map_or(LIMIT_WAIT_MS, |after| (after * 1_000.0).ceil() as i64);
                out.wait_ms = Some(wait);
                break;
            }
            // Spent points: this one went unanswered too, so it is left as well, never read
            // as "not found".
            if failure == Some(CallFailure::PrimaryLimit) {
                out.left
                    .extend(parts[i..].iter().flat_map(|p| p.iter().cloned()));
                break;
            }
        }
        out
    }

    /// One batched query per gh login; PRs the expected login can't see are retried with
    /// each other login, and the one that sees them is remembered per owner (`fetchRefs`).
    /// On a round, a login whose points are low is left out until its reset.
    pub fn fetch_refs(
        &mut self,
        refs: &[PrRef],
        accounts: &mut BTreeMap<String, String>,
        rates: Rates,
        at: i64,
        until: i64,
        is_round: bool,
    ) -> Fetched {
        let mut out = Fetched {
            rates,
            ..Fetched::default()
        };
        let low_at = is_round.then_some(at);
        let known = self.logins(false, GH_MS);
        let groups = by_login(refs, accounts, &known);
        out.needed = groups.iter().map(|(l, _)| l.clone()).collect();
        let mut misses = Misses::default();
        for (login, group) in &groups {
            if out.wait_ms.is_some() {
                break;
            }
            if low_at.is_some_and(|at| low_until(&out.rates, login, at).is_some()) {
                continue;
            }
            let r = self.query(group, login, until);
            out.wait_ms = r.wait_ms;
            out.saw(login, &r);
            for (url, pr) in r.found {
                // Remembered as a login that has logged out, and seen as this one: this one
                // from now on.
                if let Some(owner) = group.iter().find(|x| x.url == url).map(PrRef::owner)
                    && accounts.get(owner).is_some_and(|l| l != login)
                {
                    accounts.insert(owner.to_owned(), login.clone());
                }
                out.found.insert(url, pr);
            }
            for (url, error) in r.failed {
                out.errors.insert(
                    url,
                    Failure {
                        error,
                        is_miss: false,
                    },
                );
            }
            misses.asked(login, &r.missing);
            misses.refs.extend(r.missing);
        }
        // The PRs with no data first: an owner one of them is found for is remembered, and
        // isn't asked again for rights.
        if !misses.refs.is_empty() && out.wait_ms.is_none() {
            self.ask_others(misses, &known, accounts, &mut out, low_at, until);
        }
        if out.wait_ms.is_none() {
            self.ask_for_rights(refs, &known, accounts, &mut out, low_at, until);
        }
        out
    }

    /// An owner nobody is remembered for, whose PRs came back with less than write (a public
    /// repo of the other account reads as anyone's): its PRs asked once as each other login,
    /// and the owner kept as one that sees them all with more rights, so a merge goes as the
    /// right one. Asked as every other login and none does more, the first stands, and isn't
    /// asked again.
    fn ask_for_rights(
        &mut self,
        refs: &[PrRef],
        known: &Logins,
        accounts: &mut BTreeMap<String, String>,
        out: &mut Fetched,
        low_at: Option<i64>,
        until: i64,
    ) {
        let found = &out.found;
        let seen = refs.iter().filter_map(|r| {
            let pr = found.get(&r.url)?;
            Some((r.owner(), permission_of(pr)))
        });
        for owner in owners_below_write(seen, accounts, known) {
            let asked = login_for(accounts, &owner, known);
            let theirs: Vec<PrRef> = refs
                .iter()
                .filter(|r| r.owner() == owner && out.found.contains_key(&r.url))
                .cloned()
                .collect();
            let mut answers = vec![Answer {
                login: asked.clone(),
                seen: theirs.len(),
                rank: max_rank(theirs.iter().filter_map(|r| out.found.get(&r.url))),
            }];
            let mut got: HashMap<String, BTreeMap<String, Map<String, Value>>> = HashMap::new();
            let mut is_sure = true;
            for login in every_login(known).into_iter().filter(|l| *l != asked) {
                if low_at.is_some_and(|at| low_until(&out.rates, &login, at).is_some()) {
                    is_sure = false;
                    continue;
                }
                let r = self.query(&theirs, &login, until);
                out.wait_ms = r.wait_ms;
                out.saw(&login, &r);
                if !r.failed.is_empty() || !r.left.is_empty() {
                    is_sure = false;
                }
                answers.push(Answer {
                    login: login.clone(),
                    seen: r.found.len(),
                    rank: max_rank(r.found.values()),
                });
                got.insert(login, r.found);
                if out.wait_ms.is_some() {
                    break;
                }
            }
            let best = best_login(&answers, theirs.len());
            if best != asked || (is_sure && out.wait_ms.is_none()) {
                accounts.insert(owner.clone(), best.clone());
            }
            out.found.extend(got.remove(&best).unwrap_or_default());
            if out.wait_ms.is_some() {
                break;
            }
        }
    }

    /// The PRs their expected login couldn't see, asked as each other login. "Not found"
    /// only once every logged-in login was asked and answered: before any is called that,
    /// gh's logins are read again, so one logged in since is asked too.
    fn ask_others(
        &mut self,
        mut misses: Misses,
        known: &Logins,
        accounts: &mut BTreeMap<String, String>,
        out: &mut Fetched,
        low_at: Option<i64>,
        until: i64,
    ) {
        let mut is_sure = true;
        let mut order = every_login(known);
        for is_again in [false, true] {
            if is_again {
                let budget = call_budget(until, self.clock.now(), GH_MS);
                let Some(budget) =
                    budget.filter(|_| !misses.refs.is_empty() && out.wait_ms.is_none() && is_sure)
                else {
                    break;
                };
                order = every_login(&self.logins(true, budget));
            }
            for login in &order {
                let untried: Vec<PrRef> = untried_by(&misses.refs, &misses.by, login)
                    .into_iter()
                    .cloned()
                    .collect();
                if untried.is_empty() {
                    continue;
                }
                if low_at.is_some_and(|at| low_until(&out.rates, login, at).is_some()) {
                    is_sure = false;
                    continue;
                }
                let r = self.query(&untried, login, until);
                out.wait_ms = r.wait_ms;
                out.saw(login, &r);
                if !r.failed.is_empty() || !r.left.is_empty() {
                    is_sure = false;
                }
                // A PR never read says why it still isn't: as a failure, which passes.
                for (url, error) in r.failed {
                    out.errors.entry(url).or_insert(Failure {
                        error,
                        is_miss: false,
                    });
                }
                misses.asked(login, &r.missing);
                for (url, pr) in r.found {
                    out.errors.remove(&url);
                    if let Some(owner) = untried.iter().find(|x| x.url == url).map(PrRef::owner) {
                        accounts.insert(owner.to_owned(), login.clone());
                    }
                    misses.refs.retain(|m| m.url != url);
                    out.found.insert(url, pr);
                }
                if misses.refs.is_empty() || out.wait_ms.is_some() {
                    break;
                }
            }
        }
        // Held by a secondary limit, the rest were not all asked: they wait. With gh's
        // logins unread, the others were never asked: no PR is called not found.
        if out.wait_ms.is_none() && is_sure && !order.is_empty() {
            for m in misses.refs {
                let by = misses.by.get(&m.url).cloned().unwrap_or_default();
                out.errors.insert(
                    m.url,
                    Failure {
                        error: not_found_as(&by),
                        is_miss: true,
                    },
                );
            }
        }
    }

    /// GitHub's `/rate_limit` for each login once [`REST_MS`](prw_core::rate::REST_MS) is
    /// up (`askRest`), in what is left of the lease. Asked counts as checked, answered or not.
    pub fn ask_rest(&mut self, rates: Rates, at: i64, until: i64) -> Rates {
        let mut seen = rates;
        for login in every_login(&self.logins(false, GH_MS)) {
            if !is_rest_due(&seen, &login, at) {
                continue;
            }
            let Some(budget) = call_budget(until, self.clock.now(), REST_CALL_MS) else {
                break;
            };
            // A login gh gives no token for is not asked as another.
            let read = self.token(&login, budget).ok().and_then(|token| {
                let args = argv(&["api", "rate_limit"]);
                let ran = self.gh.run(&args, Some(&token), ms(budget)).ok()?;
                ran.is_ok()
                    .then(|| rest_of(&ran.stdout, self.clock.now()))?
            });
            let mut asked = read.unwrap_or_default();
            asked.asked_at = Some(at);
            seen = with_seen(seen, &login, &asked);
        }
        seen
    }
}

/// The PRs grouped by the login expected to see each, in the order first met.
fn by_login(
    refs: &[PrRef],
    accounts: &BTreeMap<String, String>,
    known: &Logins,
) -> Vec<(String, Vec<PrRef>)> {
    let mut groups: Vec<(String, Vec<PrRef>)> = Vec::new();
    for r in refs {
        let login = login_for(accounts, r.owner(), known);
        match groups.iter_mut().find(|(l, _)| *l == login) {
            Some((_, group)) => group.push(r.clone()),
            None => groups.push((login, vec![r.clone()])),
        }
    }
    groups
}

/// A fetched PR's `repository.viewerPermission`.
fn permission_of(pr: &Map<String, Value>) -> Option<&str> {
    pr.get("repository")?.get("viewerPermission")?.as_str()
}

/// The most rights any of `prs` was seen with ([`rank_of`]), 0 for none.
fn max_rank<'a>(prs: impl Iterator<Item = &'a Map<String, Value>>) -> usize {
    prs.map(|pr| rank_of(permission_of(pr))).max().unwrap_or(0)
}

/// PRs no login has found yet, and the logins each was asked as.
#[derive(Default)]
struct Misses {
    refs: Vec<PrRef>,
    by: HashMap<String, Vec<String>>,
}

impl Misses {
    fn asked(&mut self, login: &str, missing: &[PrRef]) {
        for m in missing {
            self.by
                .entry(m.url.clone())
                .or_default()
                .push(login.to_owned());
        }
    }
}

/// One batch's call (`askBatch`'s result).
struct BatchCall {
    http: Http,
    reply: prw_core::batch::BatchReply,
    ms: i64,
}

/// What [`Ctx::query`] found for one login.
#[derive(Debug, Default)]
pub struct Query {
    pub found: BTreeMap<String, Map<String, Value>>,
    pub missing: Vec<PrRef>,
    pub failed: BTreeMap<String, String>,
    /// Not asked: no time left, or spent points.
    pub left: Vec<PrRef>,
    /// The points as the last reply said, and when.
    pub rate: Option<RateLimit>,
    pub rate_at: i64,
    pub wait_ms: Option<i64>,
}

/// What [`Ctx::fetch_refs`] found.
#[derive(Debug, Default)]
pub struct Fetched {
    pub found: BTreeMap<String, Map<String, Value>>,
    pub errors: BTreeMap<String, Failure>,
    pub rates: Rates,
    pub needed: Vec<String>,
    pub wait_ms: Option<i64>,
}

impl Fetched {
    /// The points a login's replies said, folded in.
    fn saw(&mut self, login: &str, r: &Query) {
        if let Some(graphql) = bucket_of(r.rate.as_ref(), r.rate_at) {
            let seen = LoginRates {
                graphql: Some(graphql),
                ..LoginRates::default()
            };
            self.rates = with_seen(std::mem::take(&mut self.rates), login, &seen);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gh::fake::{Call, FakeClock, FakeGh, failed, ok};
    use prw_core::batch::ref_from_url;
    use serde_json::json;

    const T: i64 = 1_791_374_400_000;

    const STATUS: &str = "github.com
  ✓ Logged in to github.com account octocat (keyring)
  - Active account: true
  ✓ Logged in to github.com account hubot (keyring)
  - Active account: false";

    fn pr(owner: &str, n: u64) -> PrRef {
        ref_from_url(&format!("https://github.com/{owner}/app/pull/{n}")).unwrap()
    }

    fn is(args: &[&str]) -> impl Fn(&Call) -> bool + 'static {
        let want: Vec<String> = args.iter().map(|&a| a.to_owned()).collect();
        move |call| call.argv.starts_with(&want)
    }

    fn as_login(login: &'static str) -> impl Fn(&Call) -> bool + 'static {
        move |call| {
            call.argv.first().is_some_and(|a| a == "api") && call.token.as_deref() == Some(login)
        }
    }

    /// gh with two logins whose tokens are their names.
    fn gh() -> FakeGh {
        let gh = FakeGh::default();
        gh.on(is(&["auth", "status"]), |_| ok(STATUS));
        gh.on(is(&["auth", "token"]), |call| {
            ok(call.argv.last().cloned().unwrap_or_default())
        });
        gh
    }

    /// `gh api graphql -i`'s reply: each asked PR in `seen` found, the rest null, with
    /// write on its repo.
    fn reply(seen: &'static [&'static str]) -> impl Fn(&Call) -> Result<crate::gh::Ran, RunError> {
        reply_with(seen, "WRITE")
    }

    /// [`reply`], with `permission` on each found PR's repo.
    fn reply_with(
        seen: &'static [&'static str],
        permission: &'static str,
    ) -> impl Fn(&Call) -> Result<crate::gh::Ran, RunError> {
        move |call| {
            let query = &call.argv[3];
            let mut data = Map::new();
            for i in 0..BATCH_SIZE {
                let Some(at) = query.find(&format!("p{i}: repository(owner: \"")) else {
                    break;
                };
                let rest = &query[at..];
                let owner = rest.split('"').nth(1).unwrap_or_default();
                let number: u64 = rest
                    .split("number: ")
                    .nth(1)
                    .and_then(|s| s.split(')').next())
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
                let url = format!("https://github.com/{owner}/app/pull/{number}");
                let value = if seen.contains(&url.as_str()) {
                    json!({
                        "pullRequest": {"number": number, "state": "OPEN", "commits": {"nodes": []}},
                        "viewerPermission": permission,
                    })
                } else {
                    Value::Null
                };
                data.insert(format!("p{i}"), value);
            }
            ok(format!(
                "HTTP/2.0 200 OK\nX-Ratelimit-Remaining: 4000\r\nX-Ratelimit-Reset: 1791378000\r\nX-Ratelimit-Limit: 5000\r\n\r\n{}",
                json!({"data": data})
            ))
        }
    }

    fn ctx<'a>(
        gh: &'a FakeGh,
        clock: &'a FakeClock,
        auth: &'a mut Auth,
    ) -> Ctx<'a, FakeGh, FakeClock> {
        Ctx {
            gh,
            clock,
            auth,
            log: None,
        }
    }

    #[test]
    fn each_owner_goes_as_its_login_with_its_own_token() {
        let gh = gh();
        let a = pr("acme", 1);
        let b = pr("globex", 2);
        gh.on(
            as_login("octocat"),
            reply(&["https://github.com/acme/app/pull/1"]),
        );
        gh.on(
            as_login("hubot"),
            reply(&["https://github.com/globex/app/pull/2"]),
        );
        let clock = FakeClock::new(T, 10);
        let mut auth = Auth::default();
        let mut accounts: BTreeMap<String, String> = [("globex".into(), "hubot".into())].into();
        let out = ctx(&gh, &clock, &mut auth).fetch_refs(
            &[a.clone(), b.clone()],
            &mut accounts,
            Rates::new(),
            T,
            T + 29_000,
            true,
        );
        assert_eq!(out.found.keys().collect::<Vec<_>>(), [&a.url, &b.url]);
        assert!(out.errors.is_empty());
        assert_eq!(out.needed, ["octocat", "hubot"]);
        assert_eq!(
            out.rates["octocat"].graphql.map(|b| b.remaining),
            Some(4_000)
        );
        // No call ever went without a token.
        assert!(
            gh.calls
                .borrow()
                .iter()
                .filter(|c| c.argv[0] == "api")
                .all(|c| c.token.is_some())
        );
    }

    #[test]
    fn a_pr_one_login_cant_see_is_asked_of_the_other_and_remembered() {
        let gh = gh();
        let a = pr("acme", 1);
        gh.on(as_login("octocat"), reply(&[]));
        gh.on(
            as_login("hubot"),
            reply(&["https://github.com/acme/app/pull/1"]),
        );
        let clock = FakeClock::new(T, 10);
        let mut auth = Auth::default();
        let mut accounts = BTreeMap::new();
        let out = ctx(&gh, &clock, &mut auth).fetch_refs(
            std::slice::from_ref(&a),
            &mut accounts,
            Rates::new(),
            T,
            T + 29_000,
            true,
        );
        assert!(out.found.contains_key(&a.url));
        assert_eq!(accounts.get("acme").map(String::as_str), Some("hubot"));
    }

    /// Every `gh api` call's login, in order.
    fn api_logins(gh: &FakeGh) -> Vec<String> {
        gh.calls
            .borrow()
            .iter()
            .filter(|c| c.argv[0] == "api")
            .filter_map(|c| c.token.clone())
            .collect()
    }

    #[test]
    fn an_owner_seen_only_to_read_is_asked_as_the_other_and_kept_as_the_one_that_writes() {
        const SEEN: &[&str] = &["https://github.com/acme/app/pull/1"];
        let gh = gh();
        gh.on(as_login("octocat"), reply_with(SEEN, "READ"));
        gh.on(as_login("hubot"), reply_with(SEEN, "ADMIN"));
        let clock = FakeClock::new(T, 10);
        let mut auth = Auth::default();
        let mut accounts = BTreeMap::new();
        let a = pr("acme", 1);
        let mut fetch = |accounts: &mut BTreeMap<String, String>| {
            ctx(&gh, &clock, &mut auth).fetch_refs(
                std::slice::from_ref(&a),
                accounts,
                Rates::new(),
                T,
                T + 29_000,
                true,
            )
        };
        let out = fetch(&mut accounts);
        // The PR as the login that can merge it, and that login from now on.
        assert_eq!(
            permission_of(&out.found[&a.url]),
            Some("ADMIN"),
            "{:?}",
            out.found
        );
        assert_eq!(accounts.get("acme").map(String::as_str), Some("hubot"));
        let out = fetch(&mut accounts);
        assert!(out.found.contains_key(&a.url));
        assert_eq!(api_logins(&gh), ["octocat", "hubot", "hubot"]);
    }

    #[test]
    fn read_as_every_login_the_first_stands_and_is_not_asked_again() {
        const SEEN: &[&str] = &["https://github.com/acme/app/pull/1"];
        let gh = gh();
        gh.on(as_login("octocat"), reply_with(SEEN, "READ"));
        gh.on(as_login("hubot"), reply_with(SEEN, "READ"));
        let clock = FakeClock::new(T, 10);
        let mut auth = Auth::default();
        let mut accounts = BTreeMap::new();
        let a = pr("acme", 1);
        for _ in 0..2 {
            ctx(&gh, &clock, &mut auth).fetch_refs(
                std::slice::from_ref(&a),
                &mut accounts,
                Rates::new(),
                T,
                T + 29_000,
                true,
            );
        }
        assert_eq!(accounts.get("acme").map(String::as_str), Some("octocat"));
        assert_eq!(api_logins(&gh), ["octocat", "hubot", "octocat"]);
    }

    #[test]
    fn an_owner_of_a_logins_name_goes_as_that_login() {
        let gh = gh();
        gh.on(
            as_login("hubot"),
            reply_with(&["https://github.com/hubot/app/pull/1"], "ADMIN"),
        );
        let clock = FakeClock::new(T, 10);
        let mut auth = Auth::default();
        let out = ctx(&gh, &clock, &mut auth).fetch_refs(
            &[pr("hubot", 1)],
            &mut BTreeMap::new(),
            Rates::new(),
            T,
            T + 29_000,
            true,
        );
        assert_eq!(out.needed, ["hubot"]);
        assert_eq!(api_logins(&gh), ["hubot"]);
    }

    #[test]
    fn not_found_only_once_every_login_answered_and_named() {
        let gh = gh();
        let a = pr("acme", 1);
        gh.on(as_login("octocat"), reply(&[]));
        gh.on(as_login("hubot"), reply(&[]));
        let clock = FakeClock::new(T, 10);
        let mut auth = Auth::default();
        let out = ctx(&gh, &clock, &mut auth).fetch_refs(
            std::slice::from_ref(&a),
            &mut BTreeMap::new(),
            Rates::new(),
            T,
            T + 29_000,
            true,
        );
        assert_eq!(
            out.errors[&a.url],
            Failure {
                error: "Not found as octocat or hubot".into(),
                is_miss: true
            }
        );
        // Before calling it that, gh's logins were read again.
        let statuses = gh
            .calls
            .borrow()
            .iter()
            .filter(|c| c.argv[..2] == ["auth", "status"])
            .count();
        assert_eq!(statuses, 2);
    }

    #[test]
    fn a_timeout_is_asked_once_more_and_a_second_one_fails_the_batch() {
        let gh = gh();
        let a = pr("acme", 1);
        gh.once(as_login("octocat"), |_| failed("", "gh: HTTP 502\n"));
        gh.on(
            as_login("octocat"),
            reply(&["https://github.com/acme/app/pull/1"]),
        );
        let clock = FakeClock::new(T, 10);
        let mut auth = Auth::default();
        let out =
            ctx(&gh, &clock, &mut auth).query(std::slice::from_ref(&a), "octocat", T + 29_000);
        assert!(out.found.contains_key(&a.url));
        let gh = self::gh();
        gh.on(as_login("octocat"), |_| failed("", "gh: HTTP 502\n"));
        let out = ctx(&gh, &clock, &mut Auth::default()).query(
            std::slice::from_ref(&a),
            "octocat",
            clock.now() + 29_000,
        );
        assert_eq!(out.failed[&a.url], "gh: HTTP 502");
        assert_eq!(
            gh.calls
                .borrow()
                .iter()
                .filter(|c| c.argv[0] == "api")
                .count(),
            2
        );
    }

    #[test]
    fn a_secondary_limit_stops_the_round_and_names_the_wait() {
        let gh = gh();
        let a = pr("acme", 1);
        let b = pr("globex", 2);
        gh.on(as_login("octocat"), |_| {
            failed(
                "HTTP/2.0 403 Forbidden\nRetry-After: 90\r\n\r\n{}",
                "gh: You have exceeded a secondary rate limit (HTTP 403)\n",
            )
        });
        let clock = FakeClock::new(T, 10);
        let mut auth = Auth::default();
        let mut accounts: BTreeMap<String, String> = [("globex".into(), "hubot".into())].into();
        let out = ctx(&gh, &clock, &mut auth).fetch_refs(
            &[a.clone(), b],
            &mut accounts,
            Rates::new(),
            T,
            T + 29_000,
            true,
        );
        assert_eq!(out.wait_ms, Some(90_000));
        // Not a card's error, and hubot was never asked.
        assert!(out.errors.is_empty());
        assert!(
            !gh.calls
                .borrow()
                .iter()
                .any(|c| c.token.as_deref() == Some("hubot"))
        );
    }

    #[test]
    fn a_token_gh_wont_give_fails_its_prs_and_none_is_called_not_found() {
        let gh = FakeGh::default();
        gh.on(is(&["auth", "status"]), |_| ok(STATUS));
        gh.on(is(&["auth", "token"]), |_| failed("", "no token\n"));
        let a = pr("acme", 1);
        let clock = FakeClock::new(T, 10);
        let mut auth = Auth::default();
        let out = ctx(&gh, &clock, &mut auth).fetch_refs(
            std::slice::from_ref(&a),
            &mut BTreeMap::new(),
            Rates::new(),
            T,
            T + 29_000,
            true,
        );
        assert_eq!(
            out.errors[&a.url],
            Failure {
                error: "Couldn't get octocat's gh token".into(),
                is_miss: false
            }
        );
    }

    #[test]
    fn no_call_starts_without_time_left_in_the_lease() {
        let gh = gh();
        let clock = FakeClock::new(T, 10);
        let mut auth = Auth::default();
        let a = pr("acme", 1);
        let out = ctx(&gh, &clock, &mut auth).query(std::slice::from_ref(&a), "octocat", T + 4_000);
        assert_eq!(out.left, [a]);
        assert!(gh.calls.borrow().is_empty());
    }

    #[test]
    fn a_low_login_is_left_out_of_a_round() {
        let gh = gh();
        let clock = FakeClock::new(T, 10);
        let mut auth = Auth::default();
        let low: Rates = [(
            "octocat".to_owned(),
            LoginRates {
                graphql: Some(prw_core::protocol::Bucket {
                    limit: 5_000,
                    remaining: 10,
                    reset_at: T + 600_000,
                    seen_at: T,
                }),
                ..LoginRates::default()
            },
        )]
        .into();
        let out = ctx(&gh, &clock, &mut auth).fetch_refs(
            &[pr("acme", 1)],
            &mut BTreeMap::new(),
            low,
            T,
            T + 29_000,
            true,
        );
        assert!(out.found.is_empty() && out.errors.is_empty());
        assert_eq!(out.needed, ["octocat"]);
        assert!(!gh.calls.borrow().iter().any(|c| c.argv[0] == "api"));
    }

    #[test]
    fn rest_is_asked_per_login_once_due() {
        let gh = gh();
        gh.on(is(&["api", "rate_limit"]), |_| {
            ok(r#"{"resources": {"core": {"limit": 5000, "remaining": 4900, "reset": 1791378000}}}"#)
        });
        let clock = FakeClock::new(T, 10);
        let mut auth = Auth::default();
        let seen = ctx(&gh, &clock, &mut auth).ask_rest(Rates::new(), T, T + 29_000);
        assert_eq!(seen["octocat"].core.map(|b| b.remaining), Some(4_900));
        assert_eq!(seen["hubot"].asked_at, Some(T));
        let again = ctx(&gh, &clock, &mut auth).ask_rest(seen.clone(), T + 1_000, T + 29_000);
        assert_eq!(again, seen);
    }
}
