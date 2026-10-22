//! `prw`'s part in the poll, on a thread of its own: every tick it finds each gh login's
//! own open PRs and the saved searches' (`config.toml`; one call a minute per login), says
//! them in its session files, and, where the give-way
//! rule lets it and the lease is its, runs the round for every live session as the mod does
//! (`tick` and `publish` in `hooks/register.tsx`). The TUI learns what happened from the
//! files, as it always has, and from a [`Said`] after each tick.
//!
//! Each tick also announces every agent's watches (`prw watch`, `prw mcp`) as their sessions,
//! and drops the watches of PRs finished for an hour (`watches`).
//!
//! Writes: `sessions/prw-<id>.json` (its own PRs), `sessions/prw-<id>-searches.json` (the
//! searches' finds, `"kind": "searches"`) and `lease.json` in place (as the mod writes them), and
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
use std::time::SystemTime;

use prw_core::accounts::every_login;
use prw_core::batch::PrRef;
use prw_core::config::{Config, SEARCH_CAP, Search, parse_config, search_text};
use prw_core::poll::{
    LEASE_MS, Outcome, Pace, Round, claim, due, may_take, merge, release, should_poll,
};
use prw_core::protocol::{FileKind, Lease, SessionFile, Shared, Writer};
use prw_core::rate::low_until;
use prw_core::search::{
    Ask, Hits, MINE as MINE_ALIAS, SEARCH_MS, gather, mine, parse_searches, saved_alias,
    searches_query,
};
use prw_core::ui::{Listed, PollerSaid, Session, is_live};
use prw_core::{parse_lease, parse_session};
use serde_json::{Map, Value};

use crate::cache::{ms_of, session_id};
use crate::gh::{Clock, Gh, RunError, ms};
use crate::round::{Auth, Ctx, GH_MS};
use crate::watches::{Scope, Watches};

/// How often the poller looks (ms), as the mod's tick.
pub const TICK_MS: i64 = 10_000;
/// Session files and temp files untouched this long are removed (ms), at most every
/// [`PRUNE_EVERY_MS`].
const PRUNE_MS: i64 = 86_400_000;
const PRUNE_EVERY_MS: i64 = 3_600_000;
/// What `prw`'s own list is called.
pub const MINE: &str = "My PRs";
/// How a running TUI's own id (its session file, the lease's owner) begins: `prw-<id>`.
pub const OWN_PREFIX: &str = "prw-";

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

    fn searches_path(&self) -> PathBuf {
        self.dir
            .join("sessions")
            .join(format!("{}-searches.json", self.own))
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
            kind: None,
        };
        let _ = fs::create_dir_all(self.dir.join("sessions"));
        if let Ok(text) = serde_json::to_string(&file) {
            let _ = fs::write(self.session_path(), text);
        }
    }

    /// The saved searches' finds, for whoever polls: a file of no session's (`kind`), so no
    /// list of sessions shows it. None found: no file.
    fn announce_searches(&self, urls: &[String]) {
        if urls.is_empty() {
            let _ = fs::remove_file(self.searches_path());
            return;
        }
        let file = SessionFile {
            urls: urls.to_vec(),
            writer: Some(Writer::Prw),
            agent: None,
            name: Some("Saved searches".to_owned()),
            cwd: None,
            kind: Some(FileKind::Searches),
        };
        let _ = fs::create_dir_all(self.dir.join("sessions"));
        if let Ok(text) = serde_json::to_string(&file) {
            let _ = fs::write(self.searches_path(), text);
        }
    }

    /// On the way out: the session files go (their PRs are no longer watched), and a lease
    /// still held is given back.
    pub fn leave(&self) {
        let _ = fs::remove_file(self.session_path());
        let _ = fs::remove_file(self.searches_path());
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

/// `config.toml`, read again when it changes.
#[derive(Debug, Default)]
pub struct ConfigFile {
    /// `None`: no file, and no saved searches (the tests'; and with no home to look in).
    path: Option<PathBuf>,
    /// The file's mtime when last read (`None`: there was none), and what it said.
    read: Option<(Option<SystemTime>, Config)>,
}

impl ConfigFile {
    pub const fn at(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            read: None,
        }
    }

    /// The settings as the file is now, and whether they changed since the last look. No
    /// file: [`Config::default`]'s search; one that won't read says why.
    fn load(&mut self) -> (Config, bool) {
        let Some(path) = &self.path else {
            let none = Config {
                searches: Vec::new(),
                problems: Vec::new(),
            };
            return (none, false);
        };
        let mtime = fs::metadata(path).ok().and_then(|m| m.modified().ok());
        if let Some((was, config)) = &self.read
            && *was == mtime
        {
            return (config.clone(), false);
        }
        let config = match fs::read_to_string(path) {
            Ok(text) => parse_config(&text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Config::default(),
            Err(error) => Config {
                searches: Vec::new(),
                problems: vec![format!("config.toml: {error}")],
            },
        };
        let changed = self.read.as_ref().is_none_or(|(_, was)| *was != config);
        self.read = Some((mtime, config.clone()));
        (config, changed)
    }
}

/// What a saved search found as each login, as of its last good search.
#[derive(Debug, Clone, Default)]
struct Saved {
    by_login: BTreeMap<String, Hits>,
    error: Option<String>,
    is_searched: bool,
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
    config: ConfigFile,
    /// The saved searches now in `config.toml`, and what was wrong with it.
    searches: Vec<Search>,
    problems: Vec<String>,
    /// Each search's finds, by the search (a changed query is a new one).
    saved: BTreeMap<(String, String, Option<String>), Saved>,
    searched_at: Option<i64>,
    search_error: Option<String>,
    pruned_at: i64,
    /// When the last round this poller ran started (its `fetchedAt`).
    polled_at: Option<i64>,
    /// Each session file's last good parse, for one caught torn.
    kept: HashMap<String, SessionFile>,
    /// Other agents' watches (`prw watch`, `prw mcp`), announced each tick.
    watches: Watches,
}

impl<G: Gh, C: Clock> Poller<G, C> {
    pub fn new(files: Files, gh: G, clock: C) -> Self {
        Self {
            watches: Watches::new(files.dir.clone()),
            files,
            gh,
            clock,
            auth: Auth::default(),
            pace: Pace::default(),
            mine: BTreeMap::new(),
            config: ConfigFile::default(),
            searches: Vec::new(),
            problems: Vec::new(),
            saved: BTreeMap::new(),
            searched_at: None,
            search_error: None,
            pruned_at: 0,
            polled_at: None,
            kept: HashMap::new(),
        }
    }

    /// Its saved searches from `config` (none, else).
    #[must_use]
    pub fn with_config(mut self, config: ConfigFile) -> Self {
        self.config = config;
        self
    }

    fn ctx(&mut self) -> Ctx<'_, G, C> {
        Ctx {
            gh: &self.gh,
            clock: &self.clock,
            auth: &mut self.auth,
            log: Some(self.files.log_path()),
        }
    }

    /// The saved searches as the lists show them: each login's finds together.
    fn listed(&self) -> Vec<Listed> {
        self.searches
            .iter()
            .map(|search| {
                let saved = self.saved.get(&key(search)).cloned().unwrap_or_default();
                let hits = gather(saved.by_login.values(), SEARCH_CAP);
                Listed {
                    name: search.name.clone(),
                    query: search.query.clone(),
                    urls: hits.urls,
                    total: hits.total,
                    error: saved.error,
                    is_searched: saved.is_searched,
                }
            })
            .collect()
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
        // A changed config.toml is searched at once.
        let (config, changed) = self.config.load();
        if changed {
            self.searches = config.searches;
            self.problems = config.problems;
            self.searched_at = None;
        }
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
        let listed = self.listed();
        let found = unique_urls(listed.iter().flat_map(|l| &l.urls));
        self.files.announce_searches(&found);
        let own: Vec<String> = unique_urls(own.iter().chain(&found));
        let at = self.clock.now();
        // Every agent's watches, as their sessions; those of PRs long finished go.
        let raw = self.files.read_raw().unwrap_or_default();
        let none = Map::new();
        let prs = raw.get("prs").and_then(Value::as_object).unwrap_or(&none);
        self.watches.drop_finished(prs, at);
        self.watches.announce(Scope::All);
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
            searches: self.listed(),
            problems: self.problems.clone(),
        }
    }

    /// Each login's open PRs and the saved searches' finds, in one call per login, as that
    /// login. A login whose points are low keeps what it found last; one gh no longer lists
    /// drops its finds. A search that failed as every login it ran as says why.
    fn search(&mut self, shared: Option<&Shared>, at: i64) {
        self.searched_at = Some(at);
        let logins = every_login(&self.ctx().logins(false, GH_MS));
        self.mine.retain(|login, _| logins.contains(login));
        let keys: Vec<_> = self.searches.iter().map(key).collect();
        self.saved.retain(|k, _| keys.contains(k));
        for saved in self.saved.values_mut() {
            saved.by_login.retain(|login, _| logins.contains(login));
        }
        let mut errors = Vec::new();
        if logins.is_empty() {
            errors.push(prw_core::accounts::token_error(""));
        }
        // Each search's first failure this time, and whether any login's search came back.
        let mut failed: Vec<Option<String>> = vec![None; self.searches.len()];
        let mut is_found = vec![false; self.searches.len()];
        for login in &logins {
            if shared.is_some_and(|s| low_until(&s.rates, login, at).is_some()) {
                continue;
            }
            let asked: Vec<usize> = (0..self.searches.len())
                .filter(|&i| {
                    self.searches[i]
                        .login
                        .as_ref()
                        .is_none_or(|only| only == login)
                })
                .collect();
            let token = match self.ctx().token(login, GH_MS) {
                Ok(token) => token,
                Err(error) => {
                    for &i in &asked {
                        failed[i].get_or_insert_with(|| error.clone());
                    }
                    errors.push(error);
                    continue;
                }
            };
            let asks: Vec<Ask> = std::iter::once(mine(login))
                .chain(asked.iter().map(|&i| Ask {
                    alias: saved_alias(i),
                    text: search_text(&self.searches[i].query),
                    first: SEARCH_CAP,
                }))
                .collect();
            let aliases: Vec<&str> = asks.iter().map(|a| a.alias.as_str()).collect();
            let args = vec![
                "api".to_owned(),
                "graphql".to_owned(),
                "-f".to_owned(),
                format!("query={}", searches_query(&asks)),
            ];
            let said = match self.gh.run(&args, Some(&token), ms(GH_MS)) {
                Ok(ran) => parse_searches(&ran.stdout, &ran.stderr, &aliases),
                Err(RunError::Timeout) => Err("gh api graphql did not finish".to_owned()),
                Err(RunError::Spawn(why)) => Err(format!("gh didn't start: {why}")),
            };
            let mut found = match said {
                Ok(found) => found,
                Err(error) => {
                    let error = format!("{login}: {error}");
                    for &i in &asked {
                        failed[i].get_or_insert_with(|| error.clone());
                    }
                    errors.push(error);
                    continue;
                }
            };
            match found.hits.remove(MINE_ALIAS) {
                Some(Ok(hits)) => {
                    self.mine.insert(login.clone(), hits.urls);
                }
                Some(Err(error)) => errors.push(format!("{login}: {error}")),
                None => {}
            }
            for &i in &asked {
                match found.hits.remove(&saved_alias(i)) {
                    Some(Ok(hits)) => {
                        is_found[i] = true;
                        let saved = self.saved.entry(key(&self.searches[i])).or_default();
                        saved.by_login.insert(login.clone(), hits);
                        saved.is_searched = true;
                    }
                    Some(Err(error)) => {
                        failed[i].get_or_insert_with(|| format!("{login}: {error}"));
                    }
                    None => {}
                }
            }
        }
        for (i, search) in self.searches.iter().enumerate() {
            let saved = self.saved.entry(key(search)).or_default();
            saved.error = match &search.login {
                Some(only) if !logins.contains(only) => {
                    Some(format!("no gh login {only} is signed in"))
                }
                _ if is_found[i] => None,
                _ => failed[i].take().or_else(|| saved.error.take()),
            };
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

/// A saved search's key: the search as written, so a changed query starts again.
fn key(search: &Search) -> (String, String, Option<String>) {
    (
        search.name.clone(),
        search.query.clone(),
        search.login.clone(),
    )
}

/// Each URL once, in order.
fn unique_urls<'a>(urls: impl IntoIterator<Item = &'a String>) -> Vec<String> {
    let mut seen = HashSet::new();
    urls.into_iter()
        .filter(|url| seen.insert(url.as_str()))
        .cloned()
        .collect()
}

/// `refs` asked again now, outside a round (the mod's `fetchNow`, after a merge or a comment):
/// merged into `results.json` as it stands just before the write, with none of a round's
/// bookkeeping. Held by a secondary limit, it asks nothing: the next round does.
pub fn refresh<G: Gh, C: Clock>(files: &Files, ctx: &mut Ctx<'_, G, C>, refs: &[PrRef]) {
    let at = ctx.clock.now();
    let before_raw = files.read_raw().unwrap_or_default();
    let before = shared_of(before_raw.clone());
    if refs.is_empty() || before.held_until.is_some_and(|held| held > at) {
        return;
    }
    let mut accounts = before.accounts.clone();
    let fetched = ctx.fetch_refs(refs, &mut accounts, before.rates, at, at + GH_MS * 2, false);
    let outcome = Outcome {
        found: fetched.found,
        failed: fetched.errors,
        accounts,
        rates: fetched.rates,
        needed: fetched.needed,
        wait_ms: fetched.wait_ms,
    };
    // Read again: a round may have written while this asked GitHub.
    let after = files.read_raw().unwrap_or(before_raw);
    let _ = files.write_results(&merge(after, &outcome, None, at, ctx.clock.now()));
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

    /// What the `i`th saved search finds: one PR of 40.
    fn team_url(i: usize) -> String {
        format!("https://github.com/acme/team/pull/{}", 100 + i)
    }

    /// gh with one login, whose search finds `MINE_URL` (and each saved search asked, its
    /// [`team_url`]), and whose batches find every PR.
    fn gh() -> FakeGh {
        let gh = FakeGh::default();
        gh.on(auth("status"), |_| ok(STATUS));
        gh.on(auth("token"), |_| ok("octocat"));
        gh.on(is("search(query:"), |call| {
            let query = &call.argv[3];
            let mut data = json!({"mine": {"issueCount": 1, "nodes": [{"url": MINE_URL}]}});
            for i in (0..4).filter(|i| query.contains(&format!("s{i}: search("))) {
                data[format!("s{i}")] = json!({"issueCount": 40, "nodes": [{"url": team_url(i)}]});
            }
            ok(json!({"data": data}).to_string())
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

    /// `config.toml` in `dir`, saying `text`.
    fn config_in(dir: &Path, text: &str) -> ConfigFile {
        let path = dir.join("config.toml");
        fs::write(&path, text).unwrap();
        ConfigFile::at(path)
    }

    fn json_at(path: PathBuf) -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn saved_searches_ride_the_logins_call_and_are_announced_apart() {
        let dir = Dir::new("searches");
        let config = config_in(
            &dir.0,
            r#"
[[search]]
name = "Review requests"
query = "is:open review-requested:@me"

[[search]]
name = "Hubot's"
query = "is:open"
login = "hubot"
"#,
        );
        let files = Files::new(dir.0.clone(), "prw-test".into());
        let mut poller = Poller::new(files, gh(), FakeClock::new(now(), 1)).with_config(config);
        let said = poller.tick();
        assert_eq!(said.error, None);
        assert_eq!(
            said.searches,
            [
                Listed {
                    name: "Review requests".into(),
                    query: "is:open review-requested:@me".into(),
                    urls: vec![team_url(0)],
                    total: 40,
                    error: None,
                    is_searched: true,
                },
                Listed {
                    name: "Hubot's".into(),
                    query: "is:open".into(),
                    error: Some("no gh login hubot is signed in".into()),
                    ..Listed::default()
                },
            ]
        );
        // One call for the login: its own PRs and the search it may run, made a PR search.
        let calls = poller.gh.calls.borrow();
        let searches: Vec<&Call> = calls.iter().filter(|c| is("search(query:")(c)).collect();
        assert_eq!(searches.len(), 1);
        assert!(searches[0].argv[3].contains(
            r#"s0: search(query: "is:pr is:open review-requested:@me", type: ISSUE, first: 30)"#
        ));
        assert!(!searches[0].argv[3].contains("s1:"));
        drop(calls);
        // My PRs stays the login's own; the finds go in a file of their own, and are polled.
        assert_eq!(
            json_at(dir.0.join("sessions/prw-test.json"))["urls"],
            json!([MINE_URL])
        );
        assert_eq!(
            json_at(dir.0.join("sessions/prw-test-searches.json")),
            json!({"urls": [team_url(0)], "writer": "prw", "name": "Saved searches", "kind": "searches"})
        );
        assert!(json_at(dir.0.join("results.json"))["prs"][team_url(0)]["pr"].is_object());
        // Leaving takes both files.
        poller.files.leave();
        assert!(!dir.0.join("sessions/prw-test-searches.json").exists());
    }

    #[test]
    fn a_changed_config_is_searched_at_once_and_no_file_is_the_default() {
        let dir = Dir::new("config-change");
        let config = config_in(&dir.0, "[[search]]\nname = \"A\"\nquery = \"is:open\"\n");
        let files = Files::new(dir.0.clone(), "prw-test".into());
        let mut poller = Poller::new(files, gh(), FakeClock::new(now(), 1)).with_config(config);
        let names = |said: &PollerSaid| -> Vec<String> {
            said.searches.iter().map(|s| s.name.clone()).collect()
        };
        assert_eq!(names(&poller.tick()), ["A"]);
        fs::write(
            dir.0.join("config.toml"),
            "[[search]]\nname = \"B\"\nquery = \"is:open\"\n[[search]]\nname = \"C\"\nquery = \"is:closed\"\n",
        )
        .unwrap();
        let said = poller.tick();
        assert_eq!(names(&said), ["B", "C"]);
        assert_eq!(said.searches[1].urls, [team_url(1)]);
        assert_eq!(search_calls(&poller), 2);
        // Unchanged, the next tick inside the minute asks nothing more.
        poller.tick();
        assert_eq!(search_calls(&poller), 2);
        fs::remove_file(dir.0.join("config.toml")).unwrap();
        assert_eq!(names(&poller.tick()), ["Review requests"]);
        // A file that won't read runs nothing, and says why.
        fs::write(dir.0.join("config.toml"), "[[search]\n").unwrap();
        let said = poller.tick();
        assert_eq!(said.searches, []);
        assert_eq!(said.problems.len(), 1);
        assert!(!dir.0.join("sessions/prw-test-searches.json").exists());
    }

    fn search_calls(poller: &Poller<FakeGh, FakeClock>) -> usize {
        poller
            .gh
            .calls
            .borrow()
            .iter()
            .filter(|c| is("search(query:")(c))
            .count()
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
