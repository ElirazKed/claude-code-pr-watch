//! `prw watch`, `prw unwatch` and `prw mcp`: the commands that aren't the TUI.

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::path::PathBuf;

use prw_core::batch::PrRef;
use prw_core::watch::{agent_of_env, find_pr_urls, groups, session_of};

use crate::gh::{Clock, Wall};
use crate::mcp::{Asker, Server, serve};
use crate::watches::{Scope, Watches};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Watch,
    Unwatch,
    Mcp,
}

/// A command as parsed, with the cache and the directory it runs in.
#[derive(Debug, Clone)]
pub struct Args {
    pub command: Command,
    pub dir: PathBuf,
    pub urls: Vec<String>,
    pub agent: Option<String>,
    pub session: Option<String>,
    pub is_all: bool,
    pub cwd: Option<String>,
}

impl Args {
    /// The agent: `--agent`, else what the environment says (`agent_of_env`).
    fn agent(&self, var: &impl Fn(&str) -> Option<String>) -> String {
        self.agent.clone().unwrap_or_else(|| agent_of_env(var))
    }

    /// The session: `--session`, else the directory's name.
    fn session(&self) -> String {
        self.session
            .clone()
            .unwrap_or_else(|| session_of(self.cwd.as_deref().unwrap_or("/")))
    }
}

/// Runs a command, printing what it did.
///
/// # Errors
///
/// An argument that names no PR (`InvalidInput`), or a file that can't be written.
pub fn run(args: &Args) -> io::Result<()> {
    let var = |name: &str| std::env::var(name).ok();
    if args.command == Command::Mcp {
        let session = Asker {
            agent: args.agent.clone(),
            session: args.session(),
            cwd: args.cwd.clone(),
        };
        let server = Server::new(
            Watches::new(args.dir.clone()),
            session,
            Wall,
            env!("CARGO_PKG_VERSION"),
        );
        return serve(&server, io::stdin().lock(), io::stdout().lock());
    }
    let text = said(args, Wall.now(), &var)?;
    writeln!(io::stdout().lock(), "{text}")
}

/// What `watch` or `unwatch` did, as it says it.
///
/// # Errors
///
/// As [`run`].
fn said(args: &Args, at: i64, var: &impl Fn(&str) -> Option<String>) -> io::Result<String> {
    let watches = Watches::new(args.dir.clone());
    let refs = refs_of(&args.urls)?;
    let (agent, session) = (args.agent(var), args.session());
    let named = format!("{agent} · {session}");
    match args.command {
        Command::Watch if refs.is_empty() => Ok(listed(&watches)),
        Command::Watch => {
            watches.add(&refs, &agent, &session, args.cwd.as_deref(), at)?;
            watches.announce(Scope::Group {
                agent: &agent,
                session: &session,
            });
            Ok(format!("Watching {} as {named}.", prs(&refs)))
        }
        Command::Unwatch | Command::Mcp => {
            let is_scoped = args.agent.is_some() || args.session.is_some();
            let scope = if is_scoped {
                Scope::Group {
                    agent: &agent,
                    session: &session,
                }
            } else {
                Scope::All
            };
            let urls: Vec<String> = refs.iter().map(|r| r.url.clone()).collect();
            let gone = watches.remove((!args.is_all).then_some(&urls[..]), scope);
            // Each session that lost a watch is said again (or its file goes).
            let touched: BTreeSet<(&str, &str)> = gone
                .iter()
                .map(|w| (w.agent.as_str(), w.session.as_str()))
                .collect();
            for (agent, session) in &touched {
                watches.announce(Scope::Group { agent, session });
            }
            let place = if is_scoped {
                format!(" in {named}")
            } else {
                String::new()
            };
            Ok(match (gone.len(), args.is_all) {
                (0, true) => format!("Nothing was watched{place}."),
                (0, false) => format!("{} wasn't watched{place}.", prs(&refs)),
                (n, _) => format!(
                    "Stopped {n} watch{}{place}.",
                    if n == 1 { "" } else { "es" }
                ),
            })
        }
    }
}

/// The PRs the arguments name, each once; an argument naming none is an error, and nothing
/// is done.
fn refs_of(urls: &[String]) -> io::Result<Vec<PrRef>> {
    if let Some(bad) = urls.iter().find(|u| find_pr_urls(u).is_empty()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("'{bad}' is not a GitHub PR URL (https://github.com/<owner>/<repo>/pull/<n>)"),
        ));
    }
    Ok(find_pr_urls(&urls.join(" ")))
}

/// `acme/app#12, acme/web#3`.
fn prs(refs: &[PrRef]) -> String {
    refs.iter()
        .map(|r| format!("{}#{}", r.repo, r.number))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every watch, by session.
fn listed(watches: &Watches) -> String {
    let groups = groups(&watches.all());
    if groups.is_empty() {
        return "Nothing is watched. `prw watch <PR URL>` watches one.".to_owned();
    }
    groups
        .iter()
        .flat_map(|g| {
            let head = format!("{} · {}", g.agent, g.session);
            std::iter::once(head).chain(g.urls.iter().map(|u| format!("  {u}")))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Dir(PathBuf);

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn dir(name: &str) -> Dir {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Dir(std::env::temp_dir().join(format!("prw-cli-{name}-{nanos}")))
    }

    fn args(d: &Dir, command: Command, urls: &[&str]) -> Args {
        Args {
            command,
            dir: d.0.clone(),
            urls: urls.iter().map(|&u| u.to_owned()).collect(),
            agent: None,
            session: None,
            is_all: false,
            cwd: Some("/work/billing".into()),
        }
    }

    fn codex(name: &str) -> Option<String> {
        (name == "CODEX_SANDBOX").then(|| "seatbelt".to_owned())
    }

    const A: &str = "https://github.com/acme/app/pull/12";
    const B: &str = "https://github.com/acme/web/pull/3";

    #[test]
    fn watch_adds_in_the_agents_session_and_lists() {
        let d = dir("watch");
        let none = |_: &str| None;
        assert_eq!(
            said(&args(&d, Command::Watch, &[]), 1, &none).unwrap(),
            "Nothing is watched. `prw watch <PR URL>` watches one."
        );
        let text = said(
            &args(&d, Command::Watch, &[A, &format!("{B}/files")]),
            1,
            &codex,
        )
        .unwrap();
        assert_eq!(text, "Watching acme/app#12, acme/web#3 as codex · billing.");
        let mut flagged = args(&d, Command::Watch, &[A]);
        flagged.agent = Some("aider".into());
        flagged.session = Some("web".into());
        said(&flagged, 2, &codex).unwrap();
        assert_eq!(
            said(&args(&d, Command::Watch, &[]), 3, &none).unwrap(),
            format!("codex · billing\n  {A}\n  {B}\naider · web\n  {A}")
        );
        let id = prw_core::watch::group_id("codex", "billing");
        assert!(d.0.join(format!("sessions/{id}.json")).exists());
    }

    #[test]
    fn a_word_that_isnt_a_pr_does_nothing() {
        let d = dir("bad");
        let err = said(&args(&d, Command::Watch, &[A, "12"]), 1, &codex).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("'12' is not a GitHub PR URL"));
        assert_eq!(Watches::new(d.0.clone()).all(), []);
    }

    #[test]
    fn unwatch_is_everywhere_unless_a_session_is_named() {
        let d = dir("unwatch");
        said(&args(&d, Command::Watch, &[A, B]), 1, &codex).unwrap();
        let mut other = args(&d, Command::Watch, &[A]);
        other.session = Some("web".into());
        said(&other, 1, &codex).unwrap();
        let mut scoped = args(&d, Command::Unwatch, &[A]);
        scoped.session = Some("web".into());
        assert_eq!(
            said(&scoped, 2, &codex).unwrap(),
            "Stopped 1 watch in codex · web."
        );
        assert_eq!(
            said(&scoped, 2, &codex).unwrap(),
            "acme/app#12 wasn't watched in codex · web."
        );
        // The web session's file goes with its last watch; billing's stays.
        let web = prw_core::watch::group_id("codex", "web");
        assert!(!d.0.join(format!("sessions/{web}.json")).exists());
        let mut all = args(&d, Command::Unwatch, &[]);
        all.is_all = true;
        assert_eq!(said(&all, 3, &codex).unwrap(), "Stopped 2 watches.");
        assert_eq!(said(&all, 3, &codex).unwrap(), "Nothing was watched.");
        let billing = prw_core::watch::group_id("codex", "billing");
        assert!(!d.0.join(format!("sessions/{billing}.json")).exists());
    }
}
