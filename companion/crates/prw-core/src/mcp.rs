//! `prw mcp`'s side of the Model Context Protocol: JSON-RPC 2.0, one message a line on
//! stdio. Only what a tool server needs: `initialize`, `ping`, `tools/list` and `tools/call`,
//! with the mod's `watch` tool (same name, same `urls` argument) and an `unwatch`.

use serde_json::{Value, json};

use crate::batch::PrRef;
use crate::watch::find_pr_urls;

/// The protocol versions this server speaks, newest last: a client asking for one gets it,
/// any other gets the newest.
pub const VERSIONS: [&str; 3] = ["2024-11-05", "2025-03-26", "2025-06-18"];

/// One message from the client.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Initialize {
        id: Value,
        /// `clientInfo.name`: which agent this is.
        client: Option<String>,
        version: Option<String>,
    },
    Ping {
        id: Value,
    },
    ListTools {
        id: Value,
    },
    Call {
        id: Value,
        name: String,
        args: Value,
    },
    /// A request this server has no method for: answered with an error.
    Unknown {
        id: Value,
        method: String,
    },
    /// A notification (`notifications/initialized`, …) or a reply to nothing asked: no
    /// answer.
    Quiet,
}

/// A line from the client, or the error to answer it with (a line that isn't JSON-RPC).
///
/// # Errors
///
/// The JSON-RPC error reply: not JSON (`-32700`), or not a request (`-32600`).
pub fn parse(line: &str) -> Result<Request, Value> {
    let message: Value =
        serde_json::from_str(line).map_err(|_| error(&Value::Null, -32_700, "Parse error"))?;
    let Some(object) = message.as_object() else {
        return Err(error(&Value::Null, -32_600, "Invalid Request"));
    };
    let id = object
        .get("id")
        .filter(|id| id.is_string() || id.is_number());
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        // A reply (`result` or `error`) to a request this server never makes.
        return Ok(Request::Quiet);
    };
    let Some(id) = id.cloned() else {
        return Ok(Request::Quiet);
    };
    let params = object.get("params");
    let param = |key: &str| params.and_then(|p| p.get(key));
    Ok(match method {
        "initialize" => Request::Initialize {
            id,
            client: params
                .and_then(|p| p.pointer("/clientInfo/name"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            version: param("protocolVersion")
                .and_then(Value::as_str)
                .map(str::to_owned),
        },
        "ping" => Request::Ping { id },
        "tools/list" => Request::ListTools { id },
        "tools/call" => Request::Call {
            id,
            name: param("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            args: param("arguments").cloned().unwrap_or(Value::Null),
        },
        _ => Request::Unknown {
            id,
            method: method.to_owned(),
        },
    })
}

/// A success reply.
#[must_use]
pub fn result(id: &Value, result: Value) -> Value {
    let mut reply = json!({"jsonrpc": "2.0", "id": id});
    reply["result"] = result;
    reply
}

/// An error reply.
#[must_use]
pub fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// The reply to `initialize`: the version agreed, and a server with tools.
#[must_use]
pub fn initialized(id: &Value, asked: Option<&str>, server_version: &str) -> Value {
    let version = asked
        .filter(|v| VERSIONS.contains(v))
        .unwrap_or(VERSIONS[VERSIONS.len() - 1]);
    result(
        id,
        json!({
            "protocolVersion": version,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "prw", "version": server_version},
            "instructions": "Watch the GitHub pull requests you open or work on, once the user agrees: \
                prw shows their live CI, review and merge status beside every other agent's.",
        }),
    )
}

const URLS: &str = "PR URLs: https://github.com/<owner>/<repo>/pull/<n>";

/// The reply to `tools/list`.
#[must_use]
pub fn tools(id: &Value) -> Value {
    let schema = json!({
        "type": "object",
        "properties": {"urls": {"type": "array", "items": {"type": "string"}, "description": URLS}},
        "required": ["urls"],
    });
    result(
        id,
        json!({"tools": [
            {
                "name": "watch",
                "description": "Adds GitHub pull requests to the user's prw watch list, which shows their live CI, \
                    review and merge status. Call it only after the user agreed to watch the PR.",
                "inputSchema": schema,
            },
            {
                "name": "unwatch",
                "description": "Stops watching GitHub pull requests this session watches in prw.",
                "inputSchema": schema,
            },
        ]}),
    )
}

/// A tool's answer: text for the agent, and whether the call failed.
#[must_use]
pub fn said(id: &Value, text: &str, is_error: bool) -> Value {
    result(
        id,
        json!({"content": [{"type": "text", "text": text}], "isError": is_error}),
    )
}

/// The PRs a call's `urls` name, as the mod reads them: only strings, each PR once.
#[must_use]
pub fn urls_of(args: &Value) -> Vec<PrRef> {
    let urls: Vec<&str> = args
        .get("urls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    find_pr_urls(&urls.join(" "))
}

/// What a call with no PR URL in it says.
pub const NO_URL: &str =
    "No GitHub pull request URL given; expected https://github.com/<owner>/<repo>/pull/<n>.";

/// `#1, #2`.
#[must_use]
pub fn numbers(refs: &[PrRef]) -> String {
    refs.iter()
        .map(|r| format!("#{}", r.number))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_requests_a_tool_server_answers() {
        let init = r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"codex-mcp-client","version":"0.1"}}}"#;
        assert_eq!(
            parse(init),
            Ok(Request::Initialize {
                id: json!(0),
                client: Some("codex-mcp-client".into()),
                version: Some("2025-03-26".into())
            })
        );
        let call = r#"{"jsonrpc":"2.0","id":"c1","method":"tools/call","params":{"name":"watch","arguments":{"urls":["x"]}}}"#;
        assert_eq!(
            parse(call),
            Ok(Request::Call {
                id: json!("c1"),
                name: "watch".into(),
                args: json!({"urls": ["x"]})
            })
        );
        assert_eq!(
            parse(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#),
            Ok(Request::ListTools { id: json!(2) })
        );
        assert_eq!(
            parse(r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#),
            Ok(Request::Ping { id: json!(3) })
        );
        assert_eq!(
            parse(r#"{"jsonrpc":"2.0","id":4,"method":"resources/list"}"#),
            Ok(Request::Unknown {
                id: json!(4),
                method: "resources/list".into()
            })
        );
        assert_eq!(
            parse(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
            Ok(Request::Quiet)
        );
        assert_eq!(
            parse(r#"{"jsonrpc":"2.0","id":5,"result":{}}"#),
            Ok(Request::Quiet)
        );
    }

    #[test]
    fn a_line_that_isnt_a_request_is_answered_with_an_error() {
        assert_eq!(parse("{nope").unwrap_err()["error"]["code"], -32_700);
        assert_eq!(parse("[1]").unwrap_err()["error"]["code"], -32_600);
        assert_eq!(parse("[1]").unwrap_err()["id"], Value::Null);
    }

    #[test]
    fn initialize_agrees_a_version_it_speaks() {
        let reply = initialized(&json!(1), Some("2024-11-05"), "0.1.0");
        assert_eq!(reply["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(reply["result"]["serverInfo"]["name"], "prw");
        assert!(reply["result"]["capabilities"]["tools"].is_object());
        let reply = initialized(&json!(1), Some("1999-01-01"), "0.1.0");
        assert_eq!(reply["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(
            initialized(&json!(1), None, "0.1.0")["result"]["protocolVersion"],
            "2025-06-18"
        );
    }

    #[test]
    fn the_tools_are_the_mods_watch_and_an_unwatch() {
        let reply = tools(&json!(7));
        let names: Vec<&str> = reply["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert_eq!(names, ["watch", "unwatch"]);
        assert_eq!(
            reply["result"]["tools"][0]["inputSchema"]["required"],
            json!(["urls"])
        );
    }

    #[test]
    fn a_calls_urls_are_its_strings_prs() {
        let args = json!({"urls": ["https://github.com/acme/app/pull/3/files", 4, "https://github.com/acme/app/pull/3"]});
        let refs = urls_of(&args);
        assert_eq!(refs.len(), 1);
        assert_eq!(numbers(&refs), "#3");
        assert_eq!(urls_of(&json!({})), []);
        assert_eq!(urls_of(&Value::Null), []);
        let reply = said(&json!(1), NO_URL, true);
        assert_eq!(reply["result"]["isError"], true);
        assert_eq!(reply["result"]["content"][0]["type"], "text");
    }
}
