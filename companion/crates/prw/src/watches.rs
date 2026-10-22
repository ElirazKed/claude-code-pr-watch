//! The watch files on disk (`companion/watches/<name>.json`, see `prw_core::watch`), and the
//! session file each agent's session of them is announced as (`sessions/prw-w-<hash>.json`).
//!
//! Every write goes to a temp file in `companion/` and is renamed into place, so a reader
//! never sees one torn, and two writers of the same group (the TUI and a `prw mcp`) write the
//! same text: whichever rename comes last wins, and either is right.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use prw_core::batch::PrRef;
use prw_core::watch::{Group, Watch, group_id, groups, is_done, is_group_id, parse_watch};
use serde_json::{Map, Value};

use crate::cache::session_id;

/// The watches under a cache directory.
#[derive(Debug, Clone)]
pub struct Watches {
    dir: PathBuf,
}

/// Which watches a change is to: one agent's session's, or every one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope<'a> {
    Group { agent: &'a str, session: &'a str },
    All,
}

impl Scope<'_> {
    fn has(self, w: &Watch) -> bool {
        match self {
            Scope::Group { agent, session } => w.agent == agent && w.session == session,
            Scope::All => true,
        }
    }
}

impl Watches {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn companion(&self) -> PathBuf {
        self.dir.join("companion")
    }

    fn watch_dir(&self) -> PathBuf {
        self.companion().join("watches")
    }

    fn sessions(&self) -> PathBuf {
        self.dir.join("sessions")
    }

    /// `text` to `path`, through a temp file of this write's own in `companion/` (never in
    /// `sessions/`, where a reader would take it for a session). The temp name has a counter
    /// as well as the process id: `prw mcp`'s heartbeat thread and its main thread can write
    /// the same group's file at once.
    fn write(&self, path: &Path, text: &str) -> io::Result<()> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let temp = self
            .companion()
            .join(format!("{name}.{}-{n}.tmp", process::id()));
        fs::create_dir_all(self.companion())?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&temp, text)?;
        fs::rename(&temp, path)
    }

    /// Every watch file that parses.
    pub fn all(&self) -> Vec<Watch> {
        self.files().into_iter().map(|(_, w)| w).collect()
    }

    /// Every watch file that parses, with its path: a file is removed by the path it was
    /// read from, not by the name its contents would give it.
    fn files(&self) -> Vec<(PathBuf, Watch)> {
        let Ok(entries) = fs::read_dir(self.watch_dir()) else {
            return Vec::new();
        };
        entries
            .flatten()
            .map(|e| e.path())
            .filter(|path| session_id(path).is_some())
            .filter_map(|path| {
                let watch = parse_watch(&fs::read_to_string(&path).ok()?)?;
                Some((path, watch))
            })
            .collect()
    }

    /// `refs` watched by `agent`'s `session`; one already watched keeps when it was added.
    ///
    /// # Errors
    ///
    /// Where a watch file can't be written.
    pub fn add(
        &self,
        refs: &[PrRef],
        agent: &str,
        session: &str,
        cwd: Option<&str>,
        at: i64,
    ) -> io::Result<()> {
        let all = self.all();
        for r in refs {
            let mut watch = Watch {
                url: r.url.clone(),
                agent: agent.to_owned(),
                session: session.to_owned(),
                cwd: cwd.map(str::to_owned),
                added_at: at,
            };
            if let Some(was) = all.iter().find(|w| w.name() == watch.name()) {
                watch.added_at = was.added_at;
            }
            let path = self.watch_dir().join(format!("{}.json", watch.name()));
            self.write(&path, &serde_json::to_string(&watch)?)?;
        }
        Ok(())
    }

    /// The watches in `scope` of `urls` (every URL where `urls` is `None`) removed; the
    /// ones that were.
    pub fn remove(&self, urls: Option<&[String]>, scope: Scope) -> Vec<Watch> {
        self.remove_if(urls, |w| scope.has(w))
    }

    /// The watches `has` picks of `urls` (every URL where `urls` is `None`) removed; the
    /// ones that were.
    pub fn remove_if(&self, urls: Option<&[String]>, has: impl Fn(&Watch) -> bool) -> Vec<Watch> {
        self.files()
            .into_iter()
            .filter(|(_, w)| has(w) && urls.is_none_or(|urls| urls.contains(&w.url)))
            .filter(|(path, _)| fs::remove_file(path).is_ok())
            .map(|(_, w)| w)
            .collect()
    }

    /// Watches whose PR was merged or closed over an hour ago, by `results.json`'s `prs`,
    /// removed.
    pub fn drop_finished(&self, prs: &Map<String, Value>, at: i64) {
        for (path, w) in self.files() {
            if is_done(&w, prs, at) {
                let _ = fs::remove_file(path);
            }
        }
    }

    /// The session files of the groups in `scope` written (their mtime is their heartbeat),
    /// and those of groups in it with no watch left removed. The groups written.
    pub fn announce(&self, scope: Scope) -> Vec<Group> {
        let all: Vec<Watch> = self.all().into_iter().filter(|w| scope.has(w)).collect();
        let live = groups(&all);
        for group in &live {
            let path = self.sessions().join(format!("{}.json", group.id));
            if let Ok(text) = serde_json::to_string(&group.file()) {
                let _ = self.write(&path, &text);
            }
        }
        let gone: Vec<String> = match scope {
            Scope::Group { agent, session } => vec![group_id(agent, session)],
            Scope::All => fs::read_dir(self.sessions())
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| session_id(&e.path()))
                .filter(|id| is_group_id(id))
                .collect(),
        };
        for id in gone.iter().filter(|id| live.iter().all(|g| &g.id != *id)) {
            let _ = fs::remove_file(self.sessions().join(format!("{id}.json")));
        }
        live
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prw_core::batch::ref_from_url;
    use prw_core::parse_session;
    use prw_core::protocol::Writer;
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self(std::env::temp_dir().join(format!("prw-watches-{name}-{nanos}")))
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const A: &str = "https://github.com/acme/app/pull/1";
    const B: &str = "https://github.com/acme/web/pull/2";

    fn refs(urls: &[&str]) -> Vec<PrRef> {
        urls.iter().map(|u| ref_from_url(u).unwrap()).collect()
    }

    const BILLING: Scope = Scope::Group {
        agent: "codex",
        session: "billing",
    };

    #[test]
    fn a_watch_is_announced_as_its_sessions_file_and_goes_with_unwatch() {
        let dir = Dir::new("round");
        let watches = Watches::new(dir.0.clone());
        watches
            .add(&refs(&[A, B]), "codex", "billing", Some("/work/billing"), 5)
            .unwrap();
        watches.add(&refs(&[A]), "cli", "app", None, 6).unwrap();
        // Watched again: kept as first added.
        watches
            .add(&refs(&[A]), "codex", "billing", None, 9)
            .unwrap();
        assert_eq!(watches.all().len(), 3);
        let announced = watches.announce(BILLING);
        assert_eq!(announced.len(), 1);
        let path = dir
            .0
            .join(format!("sessions/{}.json", group_id("codex", "billing")));
        let file = parse_session(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(file.urls, [A, B]);
        assert_eq!(file.writer, Some(Writer::Prw));
        assert_eq!(file.agent.as_deref(), Some("codex"));
        assert_eq!(file.name.as_deref(), Some("billing"));
        // No temp file is left where a reader looks.
        let names: Vec<String> = fs::read_dir(dir.0.join("sessions"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().all(|n| {
            Path::new(n)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        }));
        assert_eq!(watches.remove(Some(&[A.to_owned()]), BILLING).len(), 1);
        watches.announce(BILLING);
        let file = parse_session(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(file.urls, [B]);
        assert_eq!(watches.remove(None, BILLING).len(), 1);
        watches.announce(BILLING);
        assert!(!path.exists());
        // The other session's watch is still there.
        assert_eq!(watches.all().len(), 1);
    }

    #[test]
    fn announcing_every_group_drops_files_of_groups_gone() {
        let dir = Dir::new("all");
        let watches = Watches::new(dir.0.clone());
        watches
            .add(&refs(&[A]), "codex", "billing", None, 1)
            .unwrap();
        watches.add(&refs(&[B]), "cli", "web", None, 2).unwrap();
        assert_eq!(watches.announce(Scope::All).len(), 2);
        // A group whose watches went elsewhere, and another session's file, untouched.
        fs::write(dir.0.join("sessions/m1.json"), r#"{"urls":[]}"#).unwrap();
        assert_eq!(watches.remove(None, Scope::All).len(), 2);
        assert_eq!(watches.announce(Scope::All), []);
        let left: Vec<String> = fs::read_dir(dir.0.join("sessions"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, ["m1.json"]);
    }

    #[test]
    fn a_finished_prs_watch_is_dropped() {
        let dir = Dir::new("done");
        let watches = Watches::new(dir.0.clone());
        watches
            .add(&refs(&[A, B]), "codex", "billing", None, 0)
            .unwrap();
        let prs: Map<String, Value> = [(A.to_owned(), json!({"at": 0, "pr": {"state": "CLOSED"}}))]
            .into_iter()
            .collect();
        watches.drop_finished(&prs, 3_600_001);
        let left: Vec<String> = watches.all().into_iter().map(|w| w.url).collect();
        assert_eq!(left, [B]);
        // A torn watch file is left out, not an error.
        fs::write(dir.0.join("companion/watches/torn.json"), "{").unwrap();
        assert_eq!(watches.all().len(), 1);
    }
}
