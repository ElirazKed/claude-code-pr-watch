//! Running `gh`, and the time: the poll's only ways out of the process, behind traits so a
//! round runs against a fake `gh` and a fake clock in tests.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::cache::ms_of;

/// What `gh` printed, and how it exited (`None`: killed by a signal).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ran {
    pub stdout: String,
    pub stderr: String,
    pub code: Option<i32>,
}

impl Ran {
    pub fn is_ok(&self) -> bool {
        self.code == Some(0)
    }
}

/// Why `gh` gave no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// There's no `gh` to start.
    Missing,
    /// It couldn't start for another reason.
    Spawn(String),
    /// It ran past its time and was killed.
    Timeout,
}

/// A way to run `gh`.
pub trait Gh {
    /// `gh <argv>`, as `token`'s login where one is given (`GH_TOKEN`), `input` on its stdin
    /// (none: nothing to read), killed past `timeout`.
    ///
    /// # Errors
    ///
    /// Where `gh` didn't start, or didn't finish in time.
    fn run_input(
        &self,
        argv: &[String],
        token: Option<&str>,
        input: Option<&str>,
        timeout: Duration,
    ) -> Result<Ran, RunError>;

    /// [`Gh::run_input`] with nothing on stdin.
    ///
    /// # Errors
    ///
    /// Where `gh` didn't start, or didn't finish in time.
    fn run(
        &self,
        argv: &[String],
        token: Option<&str>,
        timeout: Duration,
    ) -> Result<Ran, RunError> {
        self.run_input(argv, token, None, timeout)
    }
}

/// The real `gh`, from `PATH`.
pub struct System;

impl Gh for System {
    fn run_input(
        &self,
        argv: &[String],
        token: Option<&str>,
        input: Option<&str>,
        timeout: Duration,
    ) -> Result<Ran, RunError> {
        let mut command = Command::new("gh");
        command
            .args(argv)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Never a prompt: nobody is there to answer it.
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_NO_UPDATE_NOTIFIER", "1");
        if let Some(token) = token {
            command.env("GH_TOKEN", token);
        }
        let mut child = command.spawn().map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => RunError::Missing,
            _ => RunError::Spawn(err.to_string()),
        })?;
        // Written on a thread of its own, then closed, so gh reads to the end while its
        // output is drained.
        if let (Some(mut stdin), Some(input)) = (child.stdin.take(), input) {
            let input = input.to_owned();
            thread::spawn(move || {
                let _ = stdin.write_all(input.as_bytes());
            });
        }
        // Each pipe drained on its own thread, so a full one never stalls gh.
        let drain = |pipe: Option<Box<dyn Read + Send>>| {
            thread::spawn(move || {
                let mut text = Vec::new();
                if let Some(mut pipe) = pipe {
                    let _ = pipe.read_to_end(&mut text);
                }
                String::from_utf8_lossy(&text).into_owned()
            })
        };
        let stdout = drain(
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        );
        let stderr = drain(
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        );
        let deadline = Instant::now() + timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        // Killed: its pipes are left to their threads, as a child gh started may still hold
        // them open, and a join would wait on it.
        let status = status.ok_or(RunError::Timeout)?;
        let (stdout, stderr) = (
            stdout.join().unwrap_or_default(),
            stderr.join().unwrap_or_default(),
        );
        Ok(Ran {
            stdout,
            stderr,
            code: status.code(),
        })
    }
}

/// The time, and waiting.
pub trait Clock {
    /// Milliseconds since the epoch.
    fn now(&self) -> i64;
    fn sleep(&self, ms: i64);
}

/// The wall clock.
pub struct Wall;

impl Clock for Wall {
    fn now(&self) -> i64 {
        ms_of(SystemTime::now())
    }

    fn sleep(&self, ms: i64) {
        thread::sleep(Duration::from_millis(u64::try_from(ms).unwrap_or(0)));
    }
}

/// A timeout of `ms`, for [`Gh::run`].
pub fn ms(ms: i64) -> Duration {
    Duration::from_millis(u64::try_from(ms).unwrap_or(0))
}

#[cfg(test)]
pub mod fake {
    //! A `gh` that answers from a script, and a clock that moves only when told.

    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::time::Duration;

    use super::{Clock, Gh, Ran, RunError};

    /// One call as the fake saw it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Call {
        pub argv: Vec<String>,
        pub token: Option<String>,
        /// What it was given on stdin.
        pub input: Option<String>,
    }

    /// Answers each call with the first rule whose test it passes; a rule may answer once.
    #[derive(Default)]
    pub struct FakeGh {
        rules: RefCell<VecDeque<Rule>>,
        pub calls: RefCell<Vec<Call>>,
    }

    type Test = Box<dyn Fn(&Call) -> bool>;
    type Answer = Box<dyn Fn(&Call) -> Result<Ran, RunError>>;

    struct Rule {
        test: Test,
        answer: Answer,
        is_once: bool,
    }

    impl FakeGh {
        pub fn on(
            &self,
            test: impl Fn(&Call) -> bool + 'static,
            answer: impl Fn(&Call) -> Result<Ran, RunError> + 'static,
        ) -> &Self {
            self.rules.borrow_mut().push_back(Rule {
                test: Box::new(test),
                answer: Box::new(answer),
                is_once: false,
            });
            self
        }

        pub fn once(
            &self,
            test: impl Fn(&Call) -> bool + 'static,
            answer: impl Fn(&Call) -> Result<Ran, RunError> + 'static,
        ) -> &Self {
            self.rules.borrow_mut().push_front(Rule {
                test: Box::new(test),
                answer: Box::new(answer),
                is_once: true,
            });
            self
        }
    }

    impl Gh for FakeGh {
        fn run_input(
            &self,
            argv: &[String],
            token: Option<&str>,
            input: Option<&str>,
            _: Duration,
        ) -> Result<Ran, RunError> {
            let call = Call {
                argv: argv.to_vec(),
                token: token.map(str::to_owned),
                input: input.map(str::to_owned),
            };
            self.calls.borrow_mut().push(call.clone());
            let mut rules = self.rules.borrow_mut();
            let Some(i) = rules.iter().position(|r| (r.test)(&call)) else {
                return Err(RunError::Spawn(format!("no rule for {:?}", call.argv)));
            };
            let answer = (rules[i].answer)(&call);
            if rules[i].is_once {
                rules.remove(i);
            }
            answer
        }
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "a rule's answer, as `Gh::run` returns it"
    )]
    pub fn ok(stdout: impl Into<String>) -> Result<Ran, RunError> {
        Ok(Ran {
            stdout: stdout.into(),
            stderr: String::new(),
            code: Some(0),
        })
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "a rule's answer, as `Gh::run` returns it"
    )]
    pub fn failed(stdout: impl Into<String>, stderr: impl Into<String>) -> Result<Ran, RunError> {
        Ok(Ran {
            stdout: stdout.into(),
            stderr: stderr.into(),
            code: Some(1),
        })
    }

    /// A clock that starts at `at` and moves on only by sleeping (and by `tick` a call).
    pub struct FakeClock {
        at: Cell<i64>,
        tick: i64,
    }

    impl FakeClock {
        pub fn new(at: i64, tick: i64) -> Self {
            Self {
                at: Cell::new(at),
                tick,
            }
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> i64 {
            let at = self.at.get();
            self.at.set(at + self.tick);
            at
        }

        fn sleep(&self, ms: i64) {
            self.at.set(self.at.get() + ms);
        }
    }
}
