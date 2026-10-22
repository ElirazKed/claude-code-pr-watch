//! Watches from outside Claude: `prw watch` and `prw mcp`'s `watch` tool, for Codex and any
//! other agent. Each watch is a file of its own (`companion/watches/<name>.json`), so writers
//! never read, change and write a shared file; `prw unwatch` deletes it. The watches of one
//! agent's session are announced together, as one session file ([`Group`]), so that session
//! gets a list of its own beside the mod's.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::batch::{PrRef, ref_from_url};
use crate::entries::{KEEP_MS, is_finished};
use crate::lenient::ms_of;
use crate::protocol::{SessionFile, Writer};

/// A PR URL anywhere in a text (`PR_URL` in `hooks/pr.ts`).
static PR_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https://github\.com/([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)/pull/([0-9]+)")
        .expect("a valid pattern")
});

/// Every PR a text names, each once, in the order first met (`findPrUrls`): a URL with
/// `/files` or `#…` after it names its PR, and `/pull/007` is `/pull/7`.
#[must_use]
pub fn find_pr_urls(text: &str) -> Vec<PrRef> {
    let mut seen = HashSet::new();
    PR_URL
        .captures_iter(text)
        .filter_map(|m| {
            let number: u64 = m[2].parse().ok()?;
            ref_from_url(&format!("https://github.com/{}/pull/{number}", &m[1]))
        })
        .filter(|r| seen.insert(r.url.clone()))
        .collect()
}

/// `companion/watches/<name>.json`: one PR one agent's session watches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Watch {
    pub url: String,
    /// The agent that asked: `codex`, `claude`, … (`cli` from a shell, unless told).
    pub agent: String,
    /// Its session's name: the folder it runs in, unless told.
    pub session: String,
    /// The folder that named the session, part of which session it is, so two projects in
    /// folders of one name stay apart. `None` where the name was given (`--session`): one
    /// session by that name, wherever it is watched from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(rename = "addedAt", default)]
    pub added_at: i64,
}

/// A watch file's text; `None` where it isn't one (torn, or not a PR URL, or no agent or
/// session), which is then left out.
#[must_use]
pub fn parse_watch(text: &str) -> Option<Watch> {
    let watch: Watch = serde_json::from_str(text).ok()?;
    let is_whole =
        ref_from_url(&watch.url).is_some() && !watch.agent.is_empty() && !watch.session.is_empty();
    is_whole.then_some(watch)
}

/// FNV-1a (64-bit) of the parts, each ended by a NUL, as hex: a file name that is the same
/// for the same parts on every run and every machine.
fn hash(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in parts.iter().flat_map(|p| p.bytes().chain([0])) {
        h ^= u64::from(byte);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

impl Watch {
    /// Its file's name, without `.json`: one file per PR per agent's session.
    #[must_use]
    pub fn name(&self) -> String {
        match self.cwd.as_deref() {
            Some(cwd) => hash(&[&self.url, &self.agent, &self.session, cwd]),
            // A named session hashes as before folders counted, so its files keep their names.
            None => hash(&[&self.url, &self.agent, &self.session]),
        }
    }

    /// Whether it is of `agent`'s `session`, named from `cwd` (see [`Watch::cwd`]).
    #[must_use]
    pub fn is_of(&self, agent: &str, session: &str, cwd: Option<&str>) -> bool {
        self.agent == agent && self.session == session && self.cwd.as_deref() == cwd
    }
}

/// The session id the watches of `agent`'s `session`, named from `cwd`, are announced as.
#[must_use]
pub fn group_id(agent: &str, session: &str, cwd: Option<&str>) -> String {
    let hashed = match cwd {
        Some(cwd) => hash(&[agent, session, cwd]),
        // As before folders counted, so a named session keeps its file.
        None => hash(&[agent, session]),
    };
    format!("{GROUP_PREFIX}{hashed}")
}

const GROUP_PREFIX: &str = "prw-w-";

/// Whether a session id is a group of watches (one [`group_id`] made).
#[must_use]
pub fn is_group_id(id: &str) -> bool {
    id.starts_with(GROUP_PREFIX)
}

/// The watches of one agent's session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub id: String,
    pub agent: String,
    pub session: String,
    /// The folder that named it (see [`Watch::cwd`]).
    pub cwd: Option<String>,
    /// In the order they were added.
    pub urls: Vec<String>,
}

impl Group {
    /// Its session file.
    #[must_use]
    pub fn file(&self) -> SessionFile {
        SessionFile {
            urls: self.urls.clone(),
            writer: Some(Writer::Prw),
            agent: Some(self.agent.clone()),
            name: Some(self.session.clone()),
            cwd: self.cwd.clone(),
        }
    }
}

/// The watches by agent and session (and the folder that named it), in the order each group
/// was first added to; each URL once.
#[must_use]
pub fn groups(watches: &[Watch]) -> Vec<Group> {
    let mut sorted: Vec<&Watch> = watches.iter().collect();
    sorted.sort_by(|a, b| (a.added_at, &a.url).cmp(&(b.added_at, &b.url)));
    let mut out: Vec<Group> = Vec::new();
    for w in sorted {
        let found = out
            .iter()
            .position(|g| w.is_of(&g.agent, &g.session, g.cwd.as_deref()));
        let at = found.unwrap_or_else(|| {
            out.push(Group {
                id: group_id(&w.agent, &w.session, w.cwd.as_deref()),
                agent: w.agent.clone(),
                session: w.session.clone(),
                cwd: w.cwd.clone(),
                urls: Vec::new(),
            });
            out.len() - 1
        });
        let group = &mut out[at];
        if !group.urls.contains(&w.url) {
            group.urls.push(w.url.clone());
        }
    }
    out
}

/// Whether a watch is done with: its PR merged or closed for [`KEEP_MS`], by `results.json`'s
/// `prs`, and the watch itself older than that (a PR finished long ago and watched just now
/// is shown for an hour, not dropped on the next tick).
#[must_use]
pub fn is_done(watch: &Watch, prs: &Map<String, Value>, at: i64) -> bool {
    let entry = prs.get(&watch.url);
    let since = entry.and_then(|e| e.get("at")).and_then(ms_of);
    let is_old = |since: i64| at.saturating_sub(since) > KEEP_MS;
    is_finished(entry) && since.is_some_and(is_old) && is_old(watch.added_at)
}

/// The watches [`is_done`]: their files go.
#[must_use]
pub fn finished<'a>(watches: &'a [Watch], prs: &Map<String, Value>, at: i64) -> Vec<&'a Watch> {
    watches.iter().filter(|w| is_done(w, prs, at)).collect()
}

/// The agent an MCP client is, from the name its `initialize` gives (`codex-mcp-client`,
/// `claude-code`, …): a known agent's name, else the client's own name made plain, else
/// `agent`.
#[must_use]
pub fn agent_of_client(name: &str) -> String {
    const KNOWN: [&str; 6] = ["codex", "claude", "gemini", "cursor", "copilot", "opencode"];
    let lower = name.to_lowercase();
    if let Some(known) = KNOWN.iter().find(|k| lower.contains(*k)) {
        return (*known).to_owned();
    }
    let plain: String = lower
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let plain: String = plain.chars().take(24).collect();
    if plain.is_empty() {
        "agent".to_owned()
    } else {
        plain
    }
}

/// The agent a shell's `prw watch` runs under, by its environment: `PRW_AGENT` if set, Claude
/// Code (`CLAUDECODE`), Codex (its sandbox's variables), else `cli`.
#[must_use]
pub fn agent_of_env(var: impl Fn(&str) -> Option<String>) -> String {
    let is_set = |name: &str| var(name).is_some_and(|v| !v.is_empty());
    if let Some(agent) = var("PRW_AGENT").filter(|a| !a.is_empty()) {
        agent
    } else if is_set("CLAUDECODE") {
        "claude".to_owned()
    } else if is_set("CODEX_SANDBOX") || is_set("CODEX_SANDBOX_NETWORK_DISABLED") {
        "codex".to_owned()
    } else {
        "cli".to_owned()
    }
}

/// A session's name from its directory: the last part of the path (`/` for the root).
#[must_use]
pub fn session_of(cwd: &str) -> String {
    cwd.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("/")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const A: &str = "https://github.com/acme/app/pull/1";
    const B: &str = "https://github.com/acme/web/pull/2";

    fn watch(url: &str, agent: &str, session: &str, at: i64) -> Watch {
        Watch {
            url: url.into(),
            agent: agent.into(),
            session: session.into(),
            cwd: Some(format!("/work/{session}")),
            added_at: at,
        }
    }

    #[test]
    fn finds_each_pr_once_as_the_mod_does() {
        let text = "see https://github.com/acme/app/pull/1/files and \
                    https://github.com/acme/app/pull/001#x, then https://github.com/acme/web/pull/2 \
                    and https://github.com/acme/web/issues/3";
        let urls: Vec<String> = find_pr_urls(text).into_iter().map(|r| r.url).collect();
        assert_eq!(urls, [A, B]);
        assert_eq!(find_pr_urls("nothing here"), []);
    }

    #[test]
    fn a_watch_file_parses_only_whole() {
        let w = watch(A, "codex", "billing", 5);
        let text = serde_json::to_string(&w).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            json!({"url": A, "agent": "codex", "session": "billing", "cwd": "/work/billing", "addedAt": 5})
        );
        assert_eq!(parse_watch(&text), Some(w));
        assert_eq!(parse_watch(r#"{"url": "#), None);
        assert_eq!(
            parse_watch(
                r#"{"url": "https://github.com/acme/app/issues/1", "agent": "a", "session": "s"}"#
            ),
            None
        );
        assert_eq!(
            parse_watch(&json!({"url": A, "agent": "", "session": "s"}).to_string()),
            None
        );
    }

    #[test]
    fn names_are_stable_and_tell_watches_and_groups_apart() {
        let w = watch(A, "codex", "billing", 5);
        assert_eq!(w.name(), watch(A, "codex", "billing", 9).name());
        assert_ne!(w.name(), watch(A, "codex", "web", 5).name());
        assert_ne!(w.name(), watch(B, "codex", "billing", 5).name());
        // The parts are kept apart: "ab" + "c" is not "a" + "bc".
        assert_ne!(group_id("ab", "c", None), group_id("a", "bc", None));
        // Two folders of one name are two sessions; a named session is one.
        assert_ne!(
            group_id("codex", "app", Some("/work/a/app")),
            group_id("codex", "app", Some("/work/b/app"))
        );
        assert_ne!(
            group_id("codex", "app", None),
            group_id("codex", "app", Some("/work/a/app"))
        );
        assert!(is_group_id(&group_id("codex", "billing", None)));
        assert!(!is_group_id("prw-1a2b"));
        assert_eq!(
            group_id("codex", "billing", None).len(),
            "prw-w-".len() + 16
        );
    }

    #[test]
    fn groups_gather_a_sessions_watches_in_the_order_added() {
        let watches = [
            watch(B, "codex", "billing", 3),
            watch(A, "codex", "billing", 1),
            watch(A, "cli", "app", 2),
        ];
        let groups = groups(&watches);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].agent, "codex");
        assert_eq!(groups[0].urls, [A, B]);
        assert_eq!(
            groups[0].id,
            group_id("codex", "billing", Some("/work/billing"))
        );
        assert_eq!(groups[1].urls, [A]);
        // Folders of one name are sessions of their own.
        let mut other = watch(A, "codex", "billing", 4);
        other.cwd = Some("/elsewhere/billing".into());
        assert_eq!(super::groups(&[watches[0].clone(), other]).len(), 2);
        let file = groups[0].file();
        assert_eq!(file.writer, Some(Writer::Prw));
        assert_eq!(file.agent.as_deref(), Some("codex"));
        assert_eq!(file.name.as_deref(), Some("billing"));
        assert_eq!(file.cwd.as_deref(), Some("/work/billing"));
    }

    #[test]
    fn a_finished_prs_watches_go_after_an_hour() {
        let watches = [watch(A, "codex", "b", 0), watch(B, "codex", "b", 0)];
        let prs: Map<String, Value> = [
            (A.to_owned(), json!({"at": 0, "pr": {"state": "MERGED"}})),
            (B.to_owned(), json!({"at": 0, "pr": {"state": "OPEN"}})),
        ]
        .into_iter()
        .collect();
        assert_eq!(finished(&watches, &prs, KEEP_MS), Vec::<&Watch>::new());
        assert_eq!(finished(&watches, &prs, KEEP_MS + 1), [&watches[0]]);
        // Watched again long after it merged: kept for an hour from the watch.
        let late = [watch(A, "codex", "b", 5 * KEEP_MS)];
        assert_eq!(finished(&late, &prs, 5 * KEEP_MS + 1), Vec::<&Watch>::new());
        assert_eq!(finished(&late, &prs, 6 * KEEP_MS + 1), [&late[0]]);
        assert_eq!(
            finished(&watches, &Map::new(), KEEP_MS + 1),
            Vec::<&Watch>::new()
        );
    }

    #[test]
    fn agents_are_named_by_client_and_environment() {
        assert_eq!(agent_of_client("codex-mcp-client"), "codex");
        assert_eq!(agent_of_client("Claude Code"), "claude");
        assert_eq!(agent_of_client("My Tool 2!"), "my-tool-2");
        assert_eq!(agent_of_client("…"), "agent");
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        assert_eq!(agent_of_env(env(&[])), "cli");
        assert_eq!(agent_of_env(env(&[("CLAUDECODE", "1")])), "claude");
        assert_eq!(agent_of_env(env(&[("CODEX_SANDBOX", "seatbelt")])), "codex");
        assert_eq!(
            agent_of_env(env(&[("PRW_AGENT", "aider"), ("CLAUDECODE", "1")])),
            "aider"
        );
        assert_eq!(session_of("/work/billing/"), "billing");
        assert_eq!(session_of("/"), "/");
    }
}
