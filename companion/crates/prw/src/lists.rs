//! `lists.json`: Focus and Later (`prw_core::watchlist`), kept in prw's state folder, not the
//! cache, as they are yours. Read every tick (only a changed text is news, so another `prw`'s
//! change shows here), written whole through a temp file renamed over it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use prw_core::watchlist::{Lists, parse_lists};

/// Where `lists.json` is by default: `$XDG_STATE_HOME/prw/`, else `~/.local/state/prw/`; none
/// where neither is set.
#[must_use]
pub fn default_lists(xdg: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    let set = |v: Option<&str>| v.filter(|v| !v.is_empty()).map(PathBuf::from);
    set(xdg)
        .or_else(|| set(home).map(|home| home.join(".local").join("state")))
        .map(|dir| dir.join("prw").join("lists.json"))
}

/// The lists as `lists.json` is read and written.
#[derive(Debug, Default)]
pub struct ListsFile {
    /// `None`: no file, and the lists live only as long as the run.
    path: Option<PathBuf>,
    /// Read, never written (a demo's, whose files never change).
    is_read_only: bool,
    /// The text last read or written (`Some("")`: no file), so only a change is news.
    text: Option<String>,
}

impl ListsFile {
    pub const fn at(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            is_read_only: false,
            text: None,
        }
    }

    pub const fn read_only(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            is_read_only: true,
            text: None,
        }
    }

    /// The lists, where the file says something new since the last read or write (the first
    /// read always does: no file is no lists).
    pub fn read(&mut self) -> Option<Lists> {
        let text = match &self.path {
            Some(path) => match fs::read_to_string(path) {
                Ok(text) => text,
                Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
                // Unreadable this time: what was read last stands.
                Err(_) => return None,
            },
            None => String::new(),
        };
        if self.text.as_ref() == Some(&text) {
            return None;
        }
        let lists = parse_lists(&text);
        self.text = Some(text);
        Some(lists)
    }

    /// Writes `lists` whole; a demo's are kept for the run only. A write that fails leaves
    /// the file as it was (the lists stay as they are on screen, and the next change tries
    /// again).
    pub fn save(&mut self, lists: &Lists) {
        let Some(path) = self.path.as_ref().filter(|_| !self.is_read_only) else {
            return;
        };
        let Ok(text) = serde_json::to_string_pretty(lists) else {
            return;
        };
        if write_whole(path, &text).is_ok() {
            self.text = Some(text);
        }
    }
}

/// `text` into `path`, through a temp file beside it renamed over it, so a reader never
/// catches it half-written.
fn write_whole(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "https://github.com/acme/app/pull/1";

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("prw-lists-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("prw").join("lists.json")
    }

    #[test]
    fn the_default_place_is_the_xdg_state_folder() {
        assert_eq!(
            default_lists(Some("/x/state"), Some("/home/me")),
            Some(PathBuf::from("/x/state/prw/lists.json"))
        );
        assert_eq!(
            default_lists(Some(""), Some("/home/me")),
            Some(PathBuf::from("/home/me/.local/state/prw/lists.json"))
        );
        assert_eq!(default_lists(None, None), None);
    }

    #[test]
    fn a_save_reads_back_and_only_a_change_is_news() {
        let path = scratch("save");
        let mut file = ListsFile::at(path.clone());
        assert_eq!(file.read(), Some(Lists::default()));
        assert_eq!(file.read(), None);
        let mut lists = Lists::default();
        lists.toggle_focus(A);
        file.save(&lists);
        // Its own write is no news; another's is.
        assert_eq!(file.read(), None);
        assert_eq!(ListsFile::at(path.clone()).read(), Some(lists));
        fs::write(&path, "{}").unwrap();
        assert_eq!(file.read(), Some(Lists::default()));
        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn a_read_only_file_is_never_written() {
        let path = scratch("demo");
        let mut file = ListsFile::read_only(path.clone());
        let mut lists = Lists::default();
        lists.toggle_focus(A);
        file.save(&lists);
        assert!(!path.exists());
    }
}
