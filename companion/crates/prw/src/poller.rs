//! `prw`'s part in the poll, on a thread of its own: every tick it finds each gh login's
//! own open PRs (a search a minute), says them in its session file, and, where the give-way
//! rule lets it and the lease is its, runs the round for every live session as the mod does
//! (`tick` and `publish` in `hooks/register.tsx`). The TUI learns what happened from the
//! files, as it always has, and from a [`Said`] after each tick.
//!
//! Writes: `sessions/prw-<id>.json` and `lease.json` in place (as the mod writes them), and
//! `results.json` whole, through a temp file renamed over it. The failure log is
//! `prw-errors.log`: the mod's `errors.log` is its own.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use prw_core::accounts::every_login;
use prw_core::poll::{
    LEASE_MS, Outcome, Pace, Round, claim, due, may_take, merge, release, should_poll,
};
use prw_core::protocol::{Lease, SessionFile, Shared, Writer};
use prw_core::rate::low_until;
use prw_core::search::{SEARCH_MS, mine_query, parse_hits};
use prw_core::ui::{PollerSaid, Session, is_live};
use prw_core::{parse_lease, parse_session};
use serde_json::{Map, Value};

use crate::cache::{ms_of, session_id};
use crate::gh::{Clock, Gh, RunError, ms};
use crate::round::{Auth, Ctx, GH_MS};

/// How often the poller looks (ms), as the mod's tick.
pub const TICK_MS: i64 = 10_000;
/// Session files and temp files untouched this long are removed (ms), at most every
/// [`PRUNE_EVERY_MS`].
const PRUNE_MS: i64 = 86_400_000;
const PRUNE_EVERY_MS: i64 = 3_600_000;
/// What `prw`'s own list is called.
pub const MINE: &str = "My PRs";

/// The cache directory as `prw`'s poller writes it, as `own` (`prw-<id>`).
#[derive(Debug, Clone)]
pub struct Files {
    dir: PathBuf,
    own: String,
}

impl Files {
    pub fn new(dir: PathBuf, own: String) -> Self {
        Self { dir, own }
    }

    pub fn own(&self) -> &str {
        &self.own
    }

    fn results(&self) -> PathBuf {
        self.dir.join("results.json")
    }

    fn lease_path(&self) -> PathBuf {
        self.dir.join("lease.json")
    }

    fn session_path(&self) -> PathBuf {
        self.dir.join("sessions").join(format!("{}.json", self.own))
    }

    pub fn log_path(&self) -> PathBuf {
        self.dir.join("prw-errors.log")
    }

    /// `results.json` as raw JSON; `None` where it is missing or doesn't parse (torn by an
    /// older mod's in-place write).
    fn read_raw(&self) -> Option<Map<String, Value>> {
        match serde_json::from_str(&fs::read_to_string(self.results()).ok()?).ok()? {
            Value::Object(map) => Some(map),
            _ => None,
        }
    }

    /// `results.json` written whole: to `results.json.<own>.tmp` beside it, then renamed over
    /// it, a rename a reader sees all or nothing of.
    fn write_results(&self, shared: &Map<String, Value>) -> io::Result<()> {
        let path = self.results();
        let temp = self.dir.join(format!("results.json.{}.tmp", self.own));
        fs::create_dir_all(&self.dir)?;
        fs::write(&temp, serde_json::to_vec(shared)?)?;
        fs::rename(&temp, &path)
    }

    fn read_lease(&self) -> Option<Lease> {
        parse_lease(&fs::read_to_string(self.lease_path()).ok()?)
    }

    fn write_lease(&self, lease: &Lease) {
        if let Ok(text) = serde_json::to_string(lease) {
            let _ = fs::write(self.lease_path(), text);
        }
    }

    /// Every session file, with its mtime. A torn one (written in place) keeps its last good
    /// parse in `kept`, so a live mod caught mid-write still counts as live; one never parsed
    /// is left out this time. `kept` forgets files that went.
    fn sessions(&self, kept: &mut HashMap<String, SessionFile>) -> Vec<Session> {
        let Ok(entries) = fs::read_dir(self.dir.join("sessions")) else {
            kept.clear();
            return Vec::new();
        };
        let found: Vec<Session> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let id = session_id(&path)?;
                let meta = fs::metadata(&path).ok().filter(fs::Metadata::is_file)?;
                let parsed = fs::read_to_string(&path)
                    .ok()
                    .as_deref()
                    .and_then(parse_session);
                let file = match parsed {
                    Some(file) => {
                        kept.insert(id.clone(), file.clone());
                        file
                    }
                    None => kept.get(&id)?.clone(),
                };
                Some(Session {
                    id,
                    file,
                    mtime: ms_of(meta.modified().ok()?),
                })
            })
            .collect();
        kept.retain(|id, _| found.iter().any(|s| &s.id == id));
        found
    }

    /// `prw`'s session file: its own PRs. Written every tick, so its mtime says it is alive.
    fn announce(&self, urls: &[String]) {
        let file = SessionFile {
            urls: urls.to_vec(),
            writer: Some(Writer::Prw),
            agent: None,
            name: Some(MINE.to_owned()),
            cwd: None,
        };
        let _ = fs::create_dir_all(self.dir.join("sessions"));
        if let Ok(text) = serde_json::to_string(&file) {
            let _ = fs::write(self.session_path(), text);
        }
    }

    /// On the way out: the session file goes (its PRs are no longer watched), and a lease
    /// still held is given back.
    pub fn leave(&self) {
        let _ = fs::remove_file(self.session_path());
        if self.read_lease().is_some_and(|l| l.owner == self.own) {
            self.write_lease(&release(&self.own));
        }
    }

    /// Exited sessions' files, and temp files a write left beside `results.json`, untouched
    /// for [`PRUNE_MS`].
    fn prune(&self, at: i64) {
        let old = |dir: &Path, keep: &dyn Fn(&str) -> bool| {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let mtime = entry.metadata().ok().and_then(|m| m.modified().ok());
                let is_old = mtime.is_some_and(|m| at.saturating_sub(ms_of(m)) > PRUNE_MS);
                if is_old && !keep(&name) && entry.file_type().is_ok_and(|t| t.is_file()) {
                    let _ = fs::remove_file(entry.path());
                }
            }
        };
        old(&self.dir.join("sessions"), &|_| false);
        old(&self.dir, &|name| {
            !Path::new(name)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("tmp"))
        });
    }
}

/// The poller's state between ticks.
pub struct Poller<G: Gh, C: Clock> {
    files: Files,
    gh: G,
    clock: C,
    auth: Auth,
    pace: Pace,
    /// Each login's own open PRs as its last good search found them.
    mine: BTreeMap<String, Vec<String>>,
    searched_at: Option<i64>,
    search_error: Option<String>,
    pruned_at: i64,
    /// When the last round this poller ran started (its `fetchedAt`).
    polled_at: Option<i64>,
    /// Each session file's last good parse, for one caught torn.
    kept: HashMap<String, SessionFile>,
}

impl<G: Gh, C: Clock> Poller<G, C> {
    pub fn new(files: Files, gh: G, clock: C) -> Self {
        Self {
            files,
            gh,
            clock,
            auth: Auth::default(),
            pace: Pace::default(),
            mine: BTreeMap::new(),
            searched_at: None,
            search_error: None,
            pruned_at: 0,
            polled_at: None,
            kept: HashMap::new(),
        }
    }

    fn ctx(&mut self) -> Ctx<'_, G, C> {
        Ctx {
            gh: &self.gh,
            clock: &self.clock,
            auth: &mut self.auth,
            log: Some(self.files.log_path()),
        }
    }

    /// The PRs `prw` announces: every login's, each once, in login order.
    fn own_urls(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.mine
            .values()
            .flatten()
            .filter(|url| seen.insert(url.as_str()))
            .cloned()
            .collect()
    }

    /// One tick (`tick`): search when due, say what `prw` watches, then take the round where
    /// it is due, the give-way rule allows it, and the lease is this poller's.
    pub fn tick(&mut self) -> PollerSaid {
        let at = self.clock.now();
        let shared = self.files.read_raw().map(shared_of);
        let held = shared.as_ref().and_then(|s| s.held_until).unwrap_or(0);
        let is_search_due = self
            .searched_at
            .is_none_or(|was| at.saturating_sub(was) >= SEARCH_MS);
        let shared = if is_search_due && held <= at {
            self.search(shared.as_ref(), at);
            // The search may have taken a while (each gh call up to GH_MS), and another
            // writer may have run a round meanwhile: the time and the file as they are now.
            self.files.read_raw().map(shared_of)
        } else {
            shared
        };
        let own = self.own_urls();
        self.files.announce(&own);
        let at = self.clock.now();
        let next_at = shared.as_ref().map_or(0, |s| s.next_at);
        if at >= next_at {
            let sessions = self.files.sessions(&mut self.kept);
            let lease = self.files.read_lease();
            if should_poll(&sessions, lease.as_ref(), next_at, at) && self.take_lease(at) {
                self.round(&sessions, &own, at);
                self.files.write_lease(&release(self.files.own()));
            }
        }
        self.said()
    }

    fn said(&self) -> PollerSaid {
        PollerSaid {
            own: self.files.own().to_owned(),
            polled_at: self.polled_at,
            error: self.search_error.clone(),
        }
    }

    /// Each login's open PRs, as that login. A login whose points are low keeps what it
    /// found last; one gh no longer lists drops its PRs.
    fn search(&mut self, shared: Option<&Shared>, at: i64) {
        self.searched_at = Some(at);
        let logins = every_login(&self.ctx().logins(false, GH_MS));
        self.mine.retain(|login, _| logins.contains(login));
        let mut errors = Vec::new();
        if logins.is_empty() {
            errors.push(prw_core::accounts::token_error(""));
        }
        for login in &logins {
            if shared.is_some_and(|s| low_until(&s.rates, login, at).is_some()) {
                continue;
            }
            let token = match self.ctx().token(login, GH_MS) {
                Ok(token) => token,
                Err(error) => {
                    errors.push(error);
                    continue;
                }
            };
            let args = vec![
                "api".to_owned(),
                "graphql".to_owned(),
                "-f".to_owned(),
                format!("query={}", mine_query(login)),
            ];
            let said = match self.gh.run(&args, Some(&token), ms(GH_MS)) {
                Ok(ran) => parse_hits(&ran.stdout, &ran.stderr),
                Err(RunError::Timeout) => Err("gh api graphql did not finish".to_owned()),
                Err(RunError::Spawn(why)) => Err(format!("gh didn't start: {why}")),
            };
            match said {
                Ok(hits) => {
                    self.mine.insert(login.clone(), hits.urls);
                }
                Err(error) => errors.push(format!("{login}: {error}")),
            }
        }
        self.search_error = errors.into_iter().next();
    }

    /// The lease, as the protocol takes it (`takeLease`): claim, wait 150 ms, read again.
    fn take_lease(&self, at: i64) -> bool {
        let own = self.files.own();
        if !may_take(self.files.read_lease().as_ref(), own, at) {
            return false;
        }
        self.files.write_lease(&claim(own, at));
        self.clock.sleep(150);
        self.files.read_lease().is_some_and(|l| l.owner == own)
    }

    /// A round (`tick` inside the lease, then `publish`): every live session's PRs and
    /// `prw`'s own, the least recently asked first; the answers merged into the file as it
    /// stands just before the write.
    fn round(&mut self, sessions: &[Session], own: &[String], at: i64) {
        let mut seen = HashSet::new();
        let urls: Vec<String> = sessions
            .iter()
            .filter(|s| is_live(s.mtime, at))
            .flat_map(|s| s.file.urls.iter())
            .chain(own)
            .filter(|url| seen.insert(url.as_str()))
            .cloned()
            .collect();
        if at.saturating_sub(self.pruned_at) > PRUNE_EVERY_MS {
            self.pruned_at = at;
            self.files.prune(at);
        }
        let before_raw = self.files.read_raw().unwrap_or_default();
        let before = shared_of(before_raw.clone());
        // Held by a secondary limit: nobody asks GitHub anything until the wait is over. A
        // round another writer ran just before the lease was taken isn't run again.
        if before.held_until.is_some_and(|held| held > at) || before.next_at > at {
            return;
        }
        let refs = due(&urls, &object(before_raw.get("prs")), at);
        let until = at + LEASE_MS - 1_000;
        let mut accounts = before.accounts.clone();
        let mut ctx = self.ctx();
        let fetched = ctx.fetch_refs(&refs, &mut accounts, before.rates.clone(), at, until, true);
        let rates = if fetched.wait_ms.is_none() {
            ctx.ask_rest(fetched.rates, at, until)
        } else {
            fetched.rates
        };
        let outcome = Outcome {
            found: fetched.found,
            failed: fetched.errors,
            accounts,
            rates,
            needed: fetched.needed,
            wait_ms: fetched.wait_ms,
        };
        // Read again: another writer may have written while this one asked GitHub.
        let after = self.files.read_raw().unwrap_or(before_raw);
        let live: HashSet<String> = urls.into_iter().collect();
        let round = Round {
            live: &live,
            pace: self.pace,
        };
        let merged = merge(after, &outcome, Some(&round), at, self.clock.now());
        if self.files.write_results(&merged).is_ok() {
            self.polled_at = Some(at);
        }
    }
}

/// The typed view of `results.json`'s raw JSON, for the decisions.
fn shared_of(raw: Map<String, Value>) -> Shared {
    serde_json::from_value(Value::Object(raw)).unwrap_or_default()
}

fn object(value: Option<&Value>) -> Map<String, Value> {
    value
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// The poller running on its own thread, a tick every [`TICK_MS`] until `stop`. Each tick's
/// [`PollerSaid`] comes on the channel.
pub fn spawn<G, C>(mut poller: Poller<G, C>, stop: Arc<AtomicBool>) -> Receiver<PollerSaid>
where
    G: Gh + Send + 'static,
    C: Clock + Send + 'static,
{
    let (send, receive) = mpsc::channel();
    thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let started = poller.clock.now();
            if send.send(poller.tick()).is_err() {
                break;
            }
            // Sleep out the tick in short steps, so a stop is seen soon.
            while !stop.load(Ordering::Relaxed)
                && poller.clock.now().saturating_sub(started) < TICK_MS
            {
                poller.clock.sleep(200);
            }
        }
    });
    receive
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gh::fake::{Call, FakeClock, FakeGh, ok};
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!("prw-poller-{name}-{nanos}"));
            fs::create_dir_all(dir.join("sessions")).unwrap();
            Self(dir)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const STATUS: &str = "github.com
  ✓ Logged in to github.com account octocat (keyring)
  - Active account: true";
    const MINE_URL: &str = "https://github.com/acme/app/pull/1";
    const THEIRS: &str = "https://github.com/acme/web/pull/2";

    fn is(word: &'static str) -> impl Fn(&Call) -> bool {
        move |call| call.argv.iter().any(|a| a.contains(word))
    }

    /// `gh auth <what>`: matched on the command, as a query's text may hold any word.
    fn auth(what: &'static str) -> impl Fn(&Call) -> bool {
        move |call| call.argv.first().is_some_and(|a| a == "auth") && call.argv[1] == what
    }

    /// gh with one login, whose search finds `MINE_URL`, and whose batches find every PR.
    fn gh() -> FakeGh {
        let gh = FakeGh::default();
        gh.on(auth("status"), |_| ok(STATUS));
        gh.on(auth("token"), |_| ok("octocat"));
        gh.on(is("search(query:"), |_| {
            ok(
                json!({"data": {"search": {"issueCount": 1, "nodes": [{"url": MINE_URL}]}}})
                    .to_string(),
            )
        });
        gh.on(is("rate_limit"), |_| ok("{}"));
        gh.on(is("...pr"), |call| {
            let query = &call.argv[3];
            let data: Map<String, Value> = (0..2)
                .filter(|i| query.contains(&format!("p{i}:")))
                .map(|i| {
                    let state = json!({"pullRequest": {"state": "OPEN", "commits": {"nodes": []}}});
                    (format!("p{i}"), state)
                })
                .collect();
            ok(json!({"data": data}).to_string())
        });
        gh
    }

    fn mod_session(dir: &Path, urls: &[&str]) {
        let file = json!({"urls": urls, "writer": "mod", "agent": "claude", "name": "web"});
        fs::write(dir.join("sessions/m1.json"), file.to_string()).unwrap();
    }

    fn now() -> i64 {
        ms_of(SystemTime::now())
    }

    #[test]
    fn with_no_mod_prw_announces_its_prs_and_polls_every_live_sessions() {
        let dir = Dir::new("alone");
        // A session that is no mod's (another prw's, say) is live and watched all the same.
        let other = json!({"urls": [THEIRS], "writer": "prw", "name": "My PRs"});
        fs::write(dir.0.join("sessions/prw-other.json"), other.to_string()).unwrap();
        let files = Files::new(dir.0.clone(), "prw-test".into());
        let mut poller = Poller::new(files, gh(), FakeClock::new(now(), 1));
        let said = poller.tick();
        assert_eq!(said.error, None);
        let file: Value = serde_json::from_str(
            &fs::read_to_string(dir.0.join("sessions/prw-test.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            file,
            json!({"urls": [MINE_URL], "writer": "prw", "name": "My PRs"})
        );
        let results: Value =
            serde_json::from_str(&fs::read_to_string(dir.0.join("results.json")).unwrap()).unwrap();
        assert!(results["prs"][MINE_URL]["pr"].is_object());
        assert!(results["prs"][THEIRS]["pr"].is_object());
        assert_eq!(said.polled_at, results["fetchedAt"].as_i64());
        // The turn is given back, and no temp file is left.
        let lease: Value =
            serde_json::from_str(&fs::read_to_string(dir.0.join("lease.json")).unwrap()).unwrap();
        assert_eq!(lease, json!({"owner": "prw-test", "until": 0}));
        assert!(!dir.0.join("results.json.prw-test.tmp").exists());
    }

    #[test]
    fn a_live_mod_polls_and_prw_only_announces() {
        let dir = Dir::new("mod");
        mod_session(&dir.0, &[THEIRS]);
        // The mod's last round, its next one not yet due.
        let at = now();
        let results = json!({"fetchedAt": at - 5_000, "nextAt": at + 15_000, "prs": {}});
        fs::write(dir.0.join("results.json"), results.to_string()).unwrap();
        let files = Files::new(dir.0.clone(), "prw-test".into());
        let mut poller = Poller::new(files, gh(), FakeClock::new(at, 1));
        let said = poller.tick();
        assert_eq!(said.polled_at, None);
        let after: Value =
            serde_json::from_str(&fs::read_to_string(dir.0.join("results.json")).unwrap()).unwrap();
        assert_eq!(after, results);
        assert!(dir.0.join("sessions/prw-test.json").exists());
        assert!(
            !poller
                .gh
                .calls
                .borrow()
                .iter()
                .any(|c| c.argv.iter().any(|a| a.contains("...pr")))
        );
    }

    #[test]
    fn a_round_keeps_what_others_wrote_and_waits_for_next_at() {
        let dir = Dir::new("keep");
        let at = now();
        let before =
            json!({"fetchedAt": at - 70_000, "nextAt": at - 10_000, "future": [1], "prs": {}});
        fs::write(dir.0.join("results.json"), before.to_string()).unwrap();
        let files = Files::new(dir.0.clone(), "prw-test".into());
        let mut poller = Poller::new(files, gh(), FakeClock::new(at, 1));
        poller.tick();
        let after: Value =
            serde_json::from_str(&fs::read_to_string(dir.0.join("results.json")).unwrap()).unwrap();
        assert_eq!(after["future"], json!([1]));
        assert!(after["nextAt"].as_i64().unwrap() > at);
        // Not due again yet: the next tick asks GitHub nothing.
        let asked = poller.gh.calls.borrow().len();
        poller.tick();
        let batches = |calls: &[Call]| {
            calls
                .iter()
                .filter(|c| c.argv.iter().any(|a| a.contains("...pr")))
                .count()
        };
        assert_eq!(batches(&poller.gh.calls.borrow()[asked..]), 0);
    }

    #[test]
    fn a_lease_another_holds_is_left_alone() {
        let dir = Dir::new("lease");
        let at = now();
        fs::write(
            dir.0.join("lease.json"),
            json!({"owner": "someone", "until": at + 20_000}).to_string(),
        )
        .unwrap();
        let files = Files::new(dir.0.clone(), "prw-test".into());
        let mut poller = Poller::new(files, gh(), FakeClock::new(at, 1));
        assert_eq!(poller.tick().polled_at, None);
        let lease: Value =
            serde_json::from_str(&fs::read_to_string(dir.0.join("lease.json")).unwrap()).unwrap();
        assert_eq!(lease["owner"], "someone");
    }

    #[test]
    fn a_round_starts_from_the_time_after_the_search() {
        let dir = Dir::new("late");
        let files = Files::new(dir.0.clone(), "prw-test".into());
        let start = now();
        // Every look at the clock moves it on a second: the search's gh calls take time.
        let mut poller = Poller::new(files, gh(), FakeClock::new(start, 1_000));
        let said = poller.tick();
        assert!(said.polled_at.is_some_and(|at| at > start + 2_000));
    }

    #[test]
    fn a_torn_session_file_keeps_its_last_good_parse() {
        let dir = Dir::new("torn");
        mod_session(&dir.0, &[THEIRS]);
        let files = Files::new(dir.0.clone(), "prw-test".into());
        let mut kept = HashMap::new();
        assert_eq!(files.sessions(&mut kept)[0].file.urls, [THEIRS]);
        fs::write(dir.0.join("sessions/m1.json"), r#"{"urls": ["#).unwrap();
        let sessions = files.sessions(&mut kept);
        assert!(sessions[0].is_mod());
        assert_eq!(sessions[0].file.urls, [THEIRS]);
        fs::remove_file(dir.0.join("sessions/m1.json")).unwrap();
        assert_eq!(files.sessions(&mut kept), Vec::<Session>::new());
        assert_eq!(kept, HashMap::new());
    }

    #[test]
    fn leaving_removes_the_session_file_and_gives_back_a_held_lease() {
        let dir = Dir::new("leave");
        let files = Files::new(dir.0.clone(), "prw-test".into());
        files.announce(&[]);
        files.write_lease(&claim("prw-test", now()));
        files.leave();
        assert!(!dir.0.join("sessions/prw-test.json").exists());
        assert_eq!(files.read_lease().map(|l| l.until), Some(0));
    }

    #[test]
    fn no_logins_says_so_and_announces_nothing() {
        let dir = Dir::new("nologin");
        let gh = FakeGh::default();
        gh.on(
            |_| true,
            |_| ok("You are not logged into any GitHub hosts."),
        );
        let files = Files::new(dir.0.clone(), "prw-test".into());
        let mut poller = Poller::new(files, gh, FakeClock::new(now(), 1));
        let said = poller.tick();
        assert_eq!(
            said.error.as_deref(),
            Some("Couldn't read gh's logins (gh auth status)")
        );
    }
}
