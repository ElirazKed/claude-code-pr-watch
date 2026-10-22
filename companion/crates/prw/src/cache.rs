//! Reading the shared cache directory (`companion/PROTOCOL.md`): `results.json` and
//! `sessions/*.json`, re-read only when a file's mtime or size changed. Read-only: nothing
//! here writes, renames or deletes anything in the cache.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use prw_core::protocol::SessionFile;
use prw_core::ui::{Msg, ReadFailure, Session};
use prw_core::{parse_session, parse_shared};

/// The cache directory the mod uses: `$HOME/.cache/pr-watch` (`/tmp/.cache/pr-watch` where
/// `HOME` is unset, as the mod's `root()`).
#[must_use]
pub fn default_dir(home: Option<&str>) -> PathBuf {
    let home = home.filter(|h| !h.is_empty()).unwrap_or("/tmp");
    Path::new(home).join(".cache").join("pr-watch")
}

/// Where `config.toml` is by default: `$XDG_CONFIG_HOME/prw/`, else `~/.config/prw/`; none
/// where neither is set.
#[must_use]
pub fn default_config(xdg: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    let set = |v: Option<&str>| v.filter(|v| !v.is_empty()).map(PathBuf::from);
    set(xdg)
        .or_else(|| set(home).map(|home| home.join(".config")))
        .map(|dir| dir.join("prw").join("config.toml"))
}

/// Milliseconds since the epoch (0 before it).
#[must_use]
pub fn ms_of(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// What says a file changed: its mtime and size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    mtime: SystemTime,
    len: u64,
}

impl Stamp {
    fn of(meta: &fs::Metadata) -> Self {
        Self {
            mtime: meta.modified().unwrap_or(UNIX_EPOCH),
            len: meta.len(),
        }
    }
}

/// One session file as last read.
#[derive(Debug, Clone)]
struct Seen {
    /// The stamp of the last read that parsed; `None` while none has.
    stamp: Option<Stamp>,
    mtime: i64,
    /// The last good parse; `None` while none has (the file isn't listed until one does).
    file: Option<SessionFile>,
}

/// The cache directory, and what was last read of it.
#[derive(Debug)]
pub struct Reader {
    dir: PathBuf,
    /// The last `results.json` read: its stamp (a read that parsed), or that it was missing.
    results: Option<Result<Stamp, ()>>,
    sessions: BTreeMap<String, Seen>,
    is_first: bool,
    /// Every session counts as just written (`--demo`).
    is_demo: bool,
}

impl Reader {
    /// A reader of `dir`. With `is_demo`, every session file reads as live.
    #[must_use]
    pub const fn new(dir: PathBuf, is_demo: bool) -> Self {
        Self {
            dir,
            results: None,
            sessions: BTreeMap::new(),
            is_first: true,
            is_demo,
        }
    }

    /// The directory it reads.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Looks at the files at time `at` (ms) and says what changed since the last look: a
    /// `SharedRead` when `results.json` changed (or went), a `SessionsRead` when any session
    /// file did. The first look always says both.
    pub fn poll(&mut self, at: i64) -> Vec<Msg> {
        let mut out = Vec::new();
        if let Some(read) = self.poll_results() {
            out.push(Msg::SharedRead(read, at));
        }
        if self.poll_sessions() || self.is_first || self.is_demo {
            let sessions = self
                .sessions
                .iter()
                .filter_map(|(id, seen)| {
                    Some(Session {
                        id: id.clone(),
                        file: seen.file.clone()?,
                        mtime: if self.is_demo { at } else { seen.mtime },
                    })
                })
                .collect();
            out.push(Msg::SessionsRead(sessions, at));
        }
        self.is_first = false;
        out
    }

    /// `results.json`, where it changed. A read that fails to parse leaves the stamp unset,
    /// so the next look reads it again (a writer may have been mid-write).
    fn poll_results(&mut self) -> Option<Result<prw_core::Shared, ReadFailure>> {
        let path = self.dir.join("results.json");
        let meta = match fs::metadata(&path) {
            Ok(meta) => meta,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                let was_missing = matches!(self.results, Some(Err(())));
                self.results = Some(Err(()));
                return (!was_missing).then_some(Err(ReadFailure::Missing));
            }
            Err(err) => {
                // Read it again next time, even if it comes back unchanged.
                self.results = None;
                return Some(Err(ReadFailure::Unreadable(err.to_string())));
            }
        };
        let stamp = Stamp::of(&meta);
        if self.results == Some(Ok(stamp)) {
            return None;
        }
        let read = fs::read_to_string(&path)
            .map_err(|err| err.to_string())
            .and_then(|text| parse_shared(&text).map_err(|err| err.to_string()));
        match read {
            Ok(shared) => {
                self.results = Some(Ok(stamp));
                Some(Ok(shared))
            }
            Err(err) => {
                self.results = None;
                Some(Err(ReadFailure::Unreadable(err)))
            }
        }
    }

    /// The session files; whether any came, went or changed. A torn file keeps its last good
    /// parse, its mtime still counting, or with none yet isn't listed; either way it is read
    /// again next time, and is news only when a listed file's mtime moved.
    fn poll_sessions(&mut self) -> bool {
        let Ok(entries) = fs::read_dir(self.dir.join("sessions")) else {
            let had = !self.sessions.is_empty();
            self.sessions.clear();
            return had;
        };
        let mut changed = false;
        let mut found = BTreeMap::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(id) = session_id(&path) else {
                continue;
            };
            // Follows a symlink, as reading it does.
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let stamp = Stamp::of(&meta);
            let mtime = ms_of(stamp.mtime);
            let mut seen = self.sessions.remove(&id).unwrap_or(Seen {
                stamp: None,
                mtime,
                file: None,
            });
            if seen.stamp != Some(stamp) {
                let was = seen.mtime;
                seen.mtime = mtime;
                let parsed = fs::read_to_string(&path)
                    .ok()
                    .as_deref()
                    .and_then(parse_session);
                if let Some(file) = parsed {
                    changed = true;
                    seen.file = Some(file);
                    seen.stamp = Some(stamp);
                } else {
                    // Still torn: news only when a listed file's mtime moved (not every look).
                    changed |= seen.file.is_some() && was != mtime;
                    seen.stamp = None;
                }
            }
            found.insert(id, seen);
        }
        // Whatever is left was deleted (pruned).
        changed |= !self.sessions.is_empty();
        self.sessions = found;
        changed
    }
}

/// A session file's `<id>`: its name without `.json`. `None` for anything else (a `.tmp`).
pub fn session_id(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let id = name.strip_suffix(".json")?;
    (!id.is_empty() && !id.starts_with('.')).then(|| id.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_toml_is_where_xdg_says_else_under_home() {
        assert_eq!(
            default_config(Some("/x/conf"), Some("/home/ada")),
            Some(PathBuf::from("/x/conf/prw/config.toml"))
        );
        assert_eq!(
            default_config(Some(""), Some("/home/ada")),
            Some(PathBuf::from("/home/ada/.config/prw/config.toml"))
        );
        assert_eq!(default_config(None, None), None);
    }

    /// A fresh directory under the system's temp dir, removed on drop.
    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("prw-test-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(path.join("sessions")).unwrap();
            Self(path)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn kinds(msgs: &[Msg]) -> Vec<String> {
        msgs.iter()
            .map(|m| match m {
                Msg::SharedRead(Ok(_), _) => "shared".into(),
                Msg::SharedRead(Err(ReadFailure::Missing), _) => "missing".into(),
                Msg::SharedRead(Err(ReadFailure::Unreadable(_)), _) => "unreadable".into(),
                Msg::SessionsRead(s, _) => format!("sessions {}", s.len()),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn says_what_changed_and_only_that() {
        let dir = Dir::new("changes");
        let mut reader = Reader::new(dir.0.clone(), false);
        assert_eq!(kinds(&reader.poll(1)), ["missing", "sessions 0"]);
        assert_eq!(kinds(&reader.poll(2)), Vec::<String>::new());
        fs::write(dir.0.join("results.json"), r#"{"fetchedAt": 5}"#).unwrap();
        fs::write(dir.0.join("sessions/a.json"), r#"{"urls": ["u"]}"#).unwrap();
        fs::write(dir.0.join("sessions/a.json.tmp"), "x").unwrap();
        assert_eq!(kinds(&reader.poll(3)), ["shared", "sessions 1"]);
        assert_eq!(kinds(&reader.poll(4)), Vec::<String>::new());
        // Torn: the reader says so (the model keeps its last good copy) and reads it again.
        fs::write(dir.0.join("results.json"), r#"{"fetchedAt": 5, "pr"#).unwrap();
        assert_eq!(kinds(&reader.poll(5)), ["unreadable"]);
        assert_eq!(kinds(&reader.poll(6)), ["unreadable"]);
        // A torn session file keeps its last good parse; one that has never parsed isn't
        // listed, and isn't news look after look.
        fs::write(dir.0.join("sessions/a.json"), r#"{"urls": ["#).unwrap();
        fs::write(dir.0.join("sessions/b.json"), r#"{"urls": ["#).unwrap();
        reader.poll(7);
        assert_eq!(kinds(&reader.poll(7)), ["unreadable"]);
        fs::write(dir.0.join("sessions/c.json"), r#"{"urls": ["v"]}"#).unwrap();
        let msgs = reader.poll(7);
        let Some(Msg::SessionsRead(sessions, _)) = msgs.last() else {
            panic!("{msgs:?}");
        };
        let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["a", "c"]);
        assert_eq!(sessions[0].file.urls, ["u"]);
        fs::remove_file(dir.0.join("sessions/b.json")).unwrap();
        fs::remove_file(dir.0.join("sessions/c.json")).unwrap();
        fs::remove_file(dir.0.join("sessions/a.json")).unwrap();
        assert_eq!(
            kinds(&reader.poll(8)).last().map(String::as_str),
            Some("sessions 0")
        );
    }

    #[test]
    fn a_demo_reads_every_session_as_just_written() {
        let dir = Dir::new("demo");
        fs::write(dir.0.join("sessions/a.json"), r#"{"urls": []}"#).unwrap();
        let mut reader = Reader::new(dir.0.clone(), true);
        let msgs = reader.poll(99);
        let Some(Msg::SessionsRead(sessions, _)) = msgs.last() else {
            panic!("{msgs:?}");
        };
        assert_eq!(sessions[0].mtime, 99);
    }

    #[test]
    fn the_default_dir_is_the_mods() {
        assert_eq!(
            default_dir(Some("/home/ada")),
            PathBuf::from("/home/ada/.cache/pr-watch")
        );
        assert_eq!(default_dir(None), PathBuf::from("/tmp/.cache/pr-watch"));
    }
}
