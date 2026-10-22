//! `prw mcp`: a tool server on stdio for an agent that isn't Claude (Codex, …), with the mod's
//! `watch` tool. What it watches is written as watch files (`watches`), so the TUI lists it
//! too, and announced every tick as this agent's session while the agent runs.

use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use prw_core::batch::PrRef;
use prw_core::mcp::{NO_URL, Request, error, initialized, numbers, parse, said, tools, urls_of};
use prw_core::watch::agent_of_client;
use serde_json::Value;

use crate::gh::Clock;
use crate::poller::TICK_MS;
use crate::watches::{Scope, Watches};

/// Who this server watches for.
#[derive(Debug, Clone)]
pub struct Asker {
    /// `--agent`: else the client's name, once it has said it.
    pub agent: Option<String>,
    pub session: String,
    pub cwd: Option<String>,
}

pub struct Server<C: Clock> {
    watches: Watches,
    session: Asker,
    /// The agent as decided at `initialize`, for the heartbeat too.
    agent: Arc<Mutex<Option<String>>>,
    clock: C,
    version: &'static str,
}

impl<C: Clock> Server<C> {
    pub fn new(watches: Watches, session: Asker, clock: C, version: &'static str) -> Self {
        let agent = Arc::new(Mutex::new(session.agent.clone()));
        Self {
            watches,
            session,
            agent,
            clock,
            version,
        }
    }

    /// The agent: as decided at `initialize`, or, for a client that called before it said
    /// who it is, decided now (and kept, so the heartbeat announces its watches too).
    fn agent(&self) -> String {
        self.agent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert_with(|| agent_of_client(""))
            .clone()
    }

    fn scope<'a>(&'a self, agent: &'a str) -> Scope<'a> {
        Scope::Group {
            agent,
            session: &self.session.session,
        }
    }

    /// The reply to one line from the client, if it gets one.
    pub fn answer(&self, line: &str) -> Option<Value> {
        let request = match parse(line) {
            Ok(request) => request,
            Err(reply) => return Some(reply),
        };
        match request {
            Request::Initialize {
                id,
                client,
                version,
            } => {
                let mut agent = self.agent.lock().unwrap_or_else(PoisonError::into_inner);
                if agent.is_none() {
                    *agent = Some(agent_of_client(client.as_deref().unwrap_or_default()));
                }
                let known = agent.clone().unwrap_or_default();
                drop(agent);
                // Watches this agent's session left last time are its again.
                self.watches.announce(self.scope(&known));
                Some(initialized(&id, version.as_deref(), self.version))
            }
            Request::Ping { id } => Some(prw_core::mcp::result(&id, serde_json::json!({}))),
            Request::ListTools { id } => Some(tools(&id)),
            Request::Call { id, name, args } => Some(self.call(&id, &name, &args)),
            Request::Unknown { id, method } => {
                Some(error(&id, -32_601, &format!("Method not found: {method}")))
            }
            Request::Quiet => None,
        }
    }

    fn call(&self, id: &Value, name: &str, args: &Value) -> Value {
        if !matches!(name, "watch" | "unwatch") {
            return error(id, -32_602, &format!("Unknown tool: {name}"));
        }
        let agent = self.agent();
        let refs = urls_of(args);
        if refs.is_empty() {
            return said(id, NO_URL, true);
        }
        let scope = self.scope(&agent);
        let text = if name == "watch" {
            let added = self.watches.add(
                &refs,
                &agent,
                &self.session.session,
                self.session.cwd.as_deref(),
                self.clock.now(),
            );
            if let Err(err) = added {
                return said(id, &format!("prw couldn't write the watch: {err}"), true);
            }
            format!("Watching {} in prw.", numbers(&refs))
        } else {
            let urls: Vec<String> = refs.iter().map(|r| r.url.clone()).collect();
            let gone = self.watches.remove(Some(&urls), scope);
            let (stopped, unwatched): (Vec<PrRef>, Vec<PrRef>) = refs
                .into_iter()
                .partition(|r| gone.iter().any(|w| w.url == r.url));
            match (stopped.is_empty(), unwatched.is_empty()) {
                (true, _) => format!(
                    "{} wasn't watched in prw by this session.",
                    numbers(&unwatched)
                ),
                (false, true) => format!("Stopped watching {} in prw.", numbers(&stopped)),
                (false, false) => format!(
                    "Stopped watching {} in prw; {} wasn't watched by this session.",
                    numbers(&stopped),
                    numbers(&unwatched)
                ),
            }
        };
        self.watches.announce(scope);
        said(id, &text, false)
    }
}

/// Serves `input` until it ends (the agent quit), announcing this agent's session every
/// [`TICK_MS`] meanwhile.
///
/// # Errors
///
/// Where stdin can't be read or stdout written.
pub fn serve<C>(server: &Server<C>, input: impl BufRead, mut output: impl Write) -> io::Result<()>
where
    C: Clock,
{
    let stop = Arc::new(AtomicBool::new(false));
    let beat = {
        let (stop, agent) = (Arc::clone(&stop), Arc::clone(&server.agent));
        let (watches, session) = (server.watches.clone(), server.session.session.clone());
        thread::spawn(move || {
            let nap = Duration::from_millis(200);
            let mut slept = 0;
            while !stop.load(Ordering::Relaxed) {
                if slept >= TICK_MS {
                    slept = 0;
                    let agent = agent.lock().unwrap_or_else(PoisonError::into_inner).clone();
                    if let Some(agent) = agent {
                        watches.announce(Scope::Group {
                            agent: &agent,
                            session: &session,
                        });
                    }
                }
                thread::sleep(nap);
                slept += 200;
            }
        })
    };
    let result = (|| {
        for line in input.split(b'\n') {
            // A line that isn't UTF-8 is a parse error for the client, not the end of the
            // server.
            let line = String::from_utf8_lossy(&line?).into_owned();
            if line.trim().is_empty() {
                continue;
            }
            if let Some(reply) = server.answer(&line) {
                writeln!(output, "{reply}")?;
                output.flush()?;
            }
        }
        Ok(())
    })();
    stop.store(true, Ordering::Relaxed);
    let _ = beat.join();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gh::fake::FakeClock;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
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
        Dir(std::env::temp_dir().join(format!("prw-mcp-{name}-{nanos}")))
    }

    fn server(dir: &Dir, agent: Option<&str>) -> Server<FakeClock> {
        let session = Asker {
            agent: agent.map(str::to_owned),
            session: "billing".into(),
            cwd: Some("/work/billing".into()),
        };
        Server::new(
            Watches::new(dir.0.clone()),
            session,
            FakeClock::new(7, 0),
            "0.1.0",
        )
    }

    const INIT: &str = r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"codex-mcp-client","version":"0.1"}}}"#;

    fn call(id: i64, tool: &str, urls: &[&str]) -> String {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": tool, "arguments": {"urls": urls}}})
            .to_string()
    }

    #[test]
    fn a_session_runs_initialize_list_watch_unwatch() {
        let d = dir("round");
        let input = [
            INIT.to_owned(),
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.to_owned(),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#.to_owned(),
            call(2, "watch", &["https://github.com/acme/app/pull/12/files"]),
            call(3, "watch", &["not a url"]),
            call(4, "unwatch", &["https://github.com/acme/app/pull/12"]),
            r#"{"jsonrpc":"2.0","id":5,"method":"ping"}"#.to_owned(),
        ]
        .join("\n");
        let mut out = Vec::new();
        serve(&server(&d, None), input.as_bytes(), &mut out).unwrap();
        let replies: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        // The notification gets no reply.
        assert_eq!(replies.len(), 6);
        assert_eq!(replies[0]["result"]["serverInfo"]["name"], "prw");
        assert_eq!(replies[1]["result"]["tools"][0]["name"], "watch");
        assert_eq!(
            replies[2]["result"]["content"][0]["text"],
            "Watching #12 in prw."
        );
        assert_eq!(replies[3]["result"]["isError"], true);
        assert_eq!(
            replies[4]["result"]["content"][0]["text"],
            "Stopped watching #12 in prw."
        );
        assert_eq!(replies[5]["result"], json!({}));
        assert_eq!(replies[5]["id"], 5);
    }

    #[test]
    fn a_watch_is_this_agents_session_and_lasts_past_it() {
        let d = dir("lasts");
        let s = server(&d, None);
        s.answer(INIT);
        s.answer(&call(1, "watch", &["https://github.com/acme/app/pull/12"]));
        let watches = Watches::new(d.0.clone());
        let all = watches.all();
        assert_eq!(all.len(), 1);
        assert_eq!(
            (all[0].agent.as_str(), all[0].session.as_str()),
            ("codex", "billing")
        );
        assert_eq!(all[0].added_at, 7);
        let id = prw_core::watch::group_id("codex", "billing");
        assert!(d.0.join(format!("sessions/{id}.json")).exists());
        // A new run in the same session takes its watches up again.
        fs::remove_file(d.0.join(format!("sessions/{id}.json"))).unwrap();
        server(&d, None).answer(INIT);
        assert!(d.0.join(format!("sessions/{id}.json")).exists());
    }

    #[test]
    fn the_agent_flag_wins_over_the_client_and_odd_calls_say_why() {
        let d = dir("flag");
        let s = server(&d, Some("aider"));
        s.answer(INIT);
        s.answer(&call(1, "watch", &["https://github.com/acme/app/pull/1"]));
        assert_eq!(Watches::new(d.0.clone()).all()[0].agent, "aider");
        let reply = s.answer(&call(2, "merge", &[])).unwrap();
        assert_eq!(reply["error"]["code"], -32_602);
        let reply = s
            .answer(&call(3, "unwatch", &["https://github.com/acme/app/pull/9"]))
            .unwrap();
        assert_eq!(
            reply["result"]["content"][0]["text"],
            "#9 wasn't watched in prw by this session."
        );
        let reply = s
            .answer(r#"{"jsonrpc":"2.0","id":4,"method":"prompts/list"}"#)
            .unwrap();
        assert_eq!(reply["error"]["code"], -32_601);
        assert_eq!(s.answer("{").unwrap()["error"]["code"], -32_700);
    }
}
