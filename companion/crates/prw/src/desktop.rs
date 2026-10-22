//! The clipboard and the browser. A copy goes by the system's own tool where there is one
//! (`pbcopy`; `wl-copy`, `xclip` or `xsel`), and by OSC 52 over SSH, where that tool would
//! copy on the wrong machine. OSC 52 can't say whether the terminal took it, so the toast
//! names the way used and `v` shows the text to copy by hand.

use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// How long a clipboard tool may take.
const COPY_MS: u64 = 3_000;

/// What `prw` runs on, as far as copying and opening go.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is one variable the system sets"
)]
pub struct Env {
    /// Over SSH (`$SSH_TTY`, `$SSH_CONNECTION`): the clipboard and browser are elsewhere.
    pub is_ssh: bool,
    pub is_mac: bool,
    /// `$WAYLAND_DISPLAY`.
    pub is_wayland: bool,
    /// `$DISPLAY`.
    pub is_x: bool,
}

impl Env {
    /// The environment `prw` runs in.
    pub fn read() -> Self {
        let has = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
        Self {
            is_ssh: has("SSH_TTY") || has("SSH_CONNECTION"),
            is_mac: cfg!(target_os = "macos"),
            is_wayland: has("WAYLAND_DISPLAY"),
            is_x: has("DISPLAY"),
        }
    }
}

/// A way to copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    /// A program that reads the text on stdin.
    Tool(&'static str, &'static [&'static str]),
    /// The terminal's own clipboard escape.
    Osc52,
}

/// The ways to try, in turn (`method_for`): the system's tool locally, OSC 52 over SSH, and
/// OSC 52 last wherever no tool works.
#[must_use]
pub fn ways(env: Env) -> Vec<Way> {
    let tools: &[Way] = if env.is_ssh {
        &[]
    } else if env.is_mac {
        &[Way::Tool("pbcopy", &[])]
    } else if env.is_wayland {
        &[Way::Tool("wl-copy", &[])]
    } else if env.is_x {
        &[
            Way::Tool("xclip", &["-selection", "clipboard"]),
            Way::Tool("xsel", &["--clipboard", "--input"]),
        ]
    } else {
        &[]
    };
    tools.iter().copied().chain([Way::Osc52]).collect()
}

/// Copies `text` the first way that works; says which ("pbcopy"), or why none did.
///
/// # Errors
///
/// Where no way would take it.
pub fn copy(text: &str, env: Env) -> Result<String, String> {
    let mut problems = Vec::new();
    for way in ways(env) {
        match way {
            Way::Tool(name, args) => match by_tool(name, args, text) {
                Ok(()) => return Ok(name.to_owned()),
                Err(error) => problems.push(format!("{name}: {error}")),
            },
            Way::Osc52 => {
                let mut out = io::stdout().lock();
                return match out
                    .write_all(osc52(text).as_bytes())
                    .and_then(|()| out.flush())
                {
                    Ok(()) if problems.is_empty() => {
                        Ok("OSC 52, if the terminal allows it".to_owned())
                    }
                    Ok(()) => Ok(format!(
                        "OSC 52, if the terminal allows it; {}",
                        problems.join("; ")
                    )),
                    Err(error) => {
                        problems.push(format!("OSC 52: {error}"));
                        Err(problems.join("; "))
                    }
                };
            }
        }
    }
    Err(problems.join("; "))
}

/// The text through a clipboard tool's stdin.
fn by_tool(name: &str, args: &[&str], text: &str) -> io::Result<()> {
    let mut child = Command::new(name)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }
    let deadline = Instant::now() + Duration::from_millis(COPY_MS);
    loop {
        match child.try_wait()? {
            Some(status) if status.success() => return Ok(()),
            Some(status) => return Err(io::Error::other(format!("exited {status}"))),
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::other("didn't finish"));
            }
        }
    }
}

/// The escape that asks the terminal to put `text` on the clipboard.
#[must_use]
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

/// Standard base64, padded.
#[must_use]
pub fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ABC[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Opens `url` in the browser.
///
/// # Errors
///
/// Over SSH (the browser is on the other machine), or where the opener won't start.
pub fn open(url: &str, env: Env) -> Result<(), String> {
    if env.is_ssh {
        return Err("over SSH, the browser is on the other machine".to_owned());
    }
    let opener = if env.is_mac { "open" } else { "xdg-open" };
    let mut child = Command::new(opener)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("{opener}: {error}"))?;
    // Waited for elsewhere, so it leaves no zombie: some openers stay until the browser goes.
    thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn the_systems_tool_locally_and_osc_52_over_ssh() {
        let mac = Env {
            is_mac: true,
            ..Env::default()
        };
        assert_eq!(ways(mac), [Way::Tool("pbcopy", &[]), Way::Osc52]);
        assert_eq!(
            ways(Env {
                is_ssh: true,
                ..mac
            }),
            [Way::Osc52]
        );
        let x = Env {
            is_x: true,
            ..Env::default()
        };
        assert_eq!(ways(x).len(), 3);
        assert_eq!(
            ways(Env {
                is_wayland: true,
                ..x
            })[0],
            Way::Tool("wl-copy", &[])
        );
        assert_eq!(ways(Env::default()), [Way::Osc52]);
    }

    #[test]
    fn base64_as_the_rfc_has_it() {
        for (text, coded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(text.as_bytes()), coded, "{text}");
        }
        assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07");
    }

    #[test]
    fn over_ssh_no_browser_is_opened() {
        let env = Env {
            is_ssh: true,
            ..Env::default()
        };
        assert!(open("https://github.com/acme/app/pull/7", env).is_err());
    }

    proptest! {
        #[test]
        fn base64_is_four_letters_for_every_three_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
            let coded = base64(&bytes);
            prop_assert_eq!(coded.len(), bytes.len().div_ceil(3) * 4);
            prop_assert!(coded.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b)));
        }
    }
}
