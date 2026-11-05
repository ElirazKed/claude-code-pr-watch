//! `prw web`: the TUI's lists in the browser.
//!
//! A hub (this thread) holds a model fed as the TUI's is: the cache read each tick, the
//! poller's news, `lists.json`. HTTP threads only pass the page's requests to it and its
//! answers back, so the model has one owner and every change goes through `update`. The
//! page itself (`web/`, a Vite build in `web/dist`) is compiled into the binary.
//!
//! What may be asked and by whom is `prw_core::web`'s: this file only carries it out.

use std::fmt::Write as _;
use std::io::{self, Read as _};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use prw_core::ui::{Model, Msg};
use prw_core::web::{Asked, Route, authorize, change_of, state_of};
use rust_embed::Embed;
use tiny_http::{Header, Request, Response, Server};

use crate::{Hands, Options, Parts, SLICE, TICK, VERSION, desktop, feed, lists_file, ticked};

/// The page, as `npm run build` left it.
#[derive(Embed)]
#[folder = "web/dist"]
struct Page;

/// How many requests are answered at once.
const WORKERS: usize = 4;
/// The largest change body taken.
const BODY_MAX: u64 = 16 * 1024;
/// How long a request waits for the hub.
const HUB_WAIT: Duration = Duration::from_secs(5);
/// The model's size: the layout the TUI would pick has no bearing on the page.
const SIZE: (u16, u16) = (160, 48);

/// The page's own files and its API, nothing else: no inline script, no other host.
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; font-src 'self'; \
    img-src 'self' data:; connect-src 'self'; base-uri 'none'; form-action 'none'; \
    frame-ancestors 'none'";

/// What an HTTP thread asks the hub.
enum Ask {
    /// The state, as JSON.
    State(Sender<String>),
    /// A change to the lists, and then the state.
    Change(Box<Msg>, Sender<String>),
}

/// Serves the page until Ctrl-C.
pub fn run(options: &Options) -> io::Result<()> {
    let secret = secret()?;
    let web = options.web.unwrap_or_default();
    let server = Server::http(("127.0.0.1", web.port.unwrap_or(0)))
        .map_err(|e| io::Error::other(format!("can't listen: {e}")))?;
    let port = server.server_addr().to_ip().map_or(0, |addr| addr.port());
    let server = Arc::new(server);
    let mut parts = Parts::new(options);
    let said = parts.poll(options);
    let (asks, asked) = mpsc::channel();
    for _ in 0..WORKERS {
        let (server, asks, secret) = (Arc::clone(&server), asks.clone(), secret.clone());
        thread::spawn(move || {
            for request in server.incoming_requests() {
                answer(request, port, &secret, &asks);
            }
        });
    }
    // Ctrl-C ends the hub's loop, so the cache is left as the TUI leaves it.
    let (quit, quitting) = mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = quit.send(());
    })
    .map_err(io::Error::other)?;

    let link = format!("http://127.0.0.1:{port}/#k={secret}");
    let env = desktop::Env::read();
    eprintln!("prw web: {link}\n(the link holds this run's key; Ctrl-C stops it)");
    if !web.is_no_open
        && let Err(why) = desktop::open(&link, env)
    {
        eprintln!("prw web: couldn't open a browser ({why}): open the link above");
    }
    let (done, finished) = mpsc::channel();
    let hands = Hands {
        runner: parts.runner.as_ref(),
        done,
        env,
        dir: &parts.dir,
        lists: std::cell::RefCell::new(lists_file(options, &parts.dir)),
    };
    let first = hands.lists.borrow_mut().read().map(Msg::ListsRead);
    let now = parts.clock.now();
    let (mut model, _) = feed(
        Model::new(SIZE.0, SIZE.1),
        first.into_iter().chain(parts.reader.poll(now)),
        &hands,
    );
    let mut last_tick = Instant::now();
    while quitting.try_recv().is_err() {
        if last_tick.elapsed() >= TICK {
            last_tick = Instant::now();
            let msgs = ticked(said.as_ref(), &hands, &mut parts.reader, parts.clock.now());
            model = feed(model, msgs, &hands).0;
        }
        model = feed(model, finished.try_iter(), &hands).0;
        let wait = TICK.saturating_sub(last_tick.elapsed()).min(SLICE);
        match asked.recv_timeout(wait) {
            Ok(Ask::State(reply)) => {
                let _ = reply.send(json_of(&model));
            }
            Ok(Ask::Change(msg, reply)) => {
                model = feed(model, [*msg], &hands).0;
                let _ = reply.send(json_of(&model));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    server.unblock();
    parts.leave();
    Ok(())
}

fn json_of(model: &Model) -> String {
    serde_json::to_string(&state_of(model, VERSION)).unwrap_or_default()
}

/// A key no other page can guess: 32 random bytes, in hex.
fn secret() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().fold(String::new(), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    }))
}

/// Answers one request: a file of the page, or the API through the hub.
fn answer(mut request: Request, port: u16, secret: &str, asks: &Sender<Ask>) {
    let route = route_of(&request);
    let header = |name: &'static str| {
        request
            .headers()
            .iter()
            .find(|h| h.field.equiv(name))
            .map(|h| h.value.as_str().to_owned())
    };
    let (host, origin, content_type, key) = (
        header("Host"),
        header("Origin"),
        header("Content-Type"),
        header("X-Prw-Key"),
    );
    let asked = Asked {
        host: host.as_deref(),
        origin: origin.as_deref(),
        content_type: content_type.as_deref(),
        key: key.as_deref(),
    };
    if let Err(denied) = authorize(&route, &asked, port, secret) {
        let _ = request.respond(plain(denied.status(), "denied"));
        return;
    }
    let response = match route {
        Route::Asset(path) => match Page::get(&path) {
            Some(file) => {
                let response = Response::from_data(file.data.into_owned())
                    .with_header(header_of("Content-Type", content_type_of(&path)));
                // Vite names each build's scripts by their hash; the page itself, never.
                let cache = if path.starts_with("assets/") {
                    "public, max-age=31536000, immutable"
                } else {
                    "no-cache"
                };
                secured(response.with_header(header_of("Cache-Control", cache)))
            }
            None => plain(404, "not found"),
        },
        Route::State => from_hub(asks, Ask::State),
        Route::Lists => {
            let mut body = String::new();
            let read = request.as_reader().take(BODY_MAX).read_to_string(&mut body);
            match read.ok().and_then(|_| change_of(&body)) {
                Some(msg) => from_hub(asks, |reply| Ask::Change(Box::new(msg), reply)),
                None => plain(400, "not a change"),
            }
        }
        Route::NotFound => plain(404, "not found"),
        Route::Method => plain(405, "method not allowed"),
    };
    let _ = request.respond(response);
}

fn route_of(request: &Request) -> Route {
    prw_core::web::route(request.method().as_str(), request.url())
}

/// The state, as the hub answers.
fn from_hub(
    asks: &Sender<Ask>,
    ask: impl FnOnce(Sender<String>) -> Ask,
) -> Response<io::Cursor<Vec<u8>>> {
    let (reply, replied): (Sender<String>, Receiver<String>) = mpsc::channel();
    if asks.send(ask(reply)).is_err() {
        return plain(503, "stopping");
    }
    match replied.recv_timeout(HUB_WAIT) {
        Ok(json) => secured(
            Response::from_string(json)
                .with_header(header_of("Content-Type", "application/json"))
                .with_header(header_of("Cache-Control", "no-store")),
        ),
        Err(_) => plain(503, "busy"),
    }
}

fn plain(status: u16, text: &str) -> Response<io::Cursor<Vec<u8>>> {
    secured(
        Response::from_string(text)
            .with_status_code(status)
            .with_header(header_of("Content-Type", "text/plain; charset=utf-8")),
    )
}

/// Every answer: the page's CSP, no sniffing, no referrer, no framing.
fn secured<R: io::Read>(response: Response<R>) -> Response<R> {
    response
        .with_header(header_of("Content-Security-Policy", CSP))
        .with_header(header_of("X-Content-Type-Options", "nosniff"))
        .with_header(header_of("Referrer-Policy", "no-referrer"))
        .with_header(header_of("X-Frame-Options", "DENY"))
}

fn header_of(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("an ASCII header")
}

/// A file's type, by its extension: what a Vite build holds.
fn content_type_of(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default() {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}
