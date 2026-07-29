//! End-to-end test of `dbpylot mcp` over real pipes: spawn the actual binary,
//! run the MCP handshake the way OpenPylot's client does (including its quirk
//! of sending `notifications/initialized` WITH an id and blocking for a
//! reply), and assert stdout carries nothing but JSON-RPC lines.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

#[test]
fn mcp_stdio_handshake_and_tools_list() {
    let home = std::env::temp_dir().join(format!("odbp-mcp-stdio-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_dbpylot"))
        .arg("mcp")
        // Point at an empty home so the server is guaranteed unconfigured and
        // never touches the developer's real ~/.opendbpylot.
        .env("OPENDBPYLOT_HOME", &home)
        .env("OPENDBPYLOT_LOG", "debug") // logs must go to stderr, not stdout
        // Run from the empty home and strip provider keys, so neither the
        // developer's shell env nor a repo-root `.env` (loaded by dotenvy from
        // the CWD) can configure the engine behind the test's back.
        .current_dir(&home)
        .env_remove("OPENAI_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn dbpylot mcp");

    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    let mut send = |msg: Value| {
        stdin.write_all(msg.to_string().as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        // The invariant: every line on stdout parses as JSON-RPC.
        serde_json::from_str::<Value>(&line).expect("stdout line was not valid JSON")
    };

    // 1. initialize
    let init = send(json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05", "capabilities": {},
            "clientInfo": { "name": "test-host", "version": "0" }
        }
    }));
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["serverInfo"]["name"], "opendbpylot");

    // 2. initialized notification, OpenPylot-style (with an id) — the server
    //    must reply or OpenPylot's blocking transport would hang forever.
    let initialized = send(json!({
        "jsonrpc": "2.0", "id": 2, "method": "notifications/initialized"
    }));
    assert_eq!(initialized["id"], 2);
    assert!(initialized["result"].is_object());

    // 3. tools/list
    let tools = send(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }));
    assert_eq!(tools["id"], 3);
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .expect("tools/list result has no tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["ask_database", "run_sql", "list_schema", "refresh_schema", "train", "health"]
    );

    // 4. unconfigured tools/call reports the init hint as an isError result.
    let ask = send(json!({
        "jsonrpc": "2.0", "id": 4, "method": "tools/call",
        "params": { "name": "ask_database", "arguments": { "question": "hi" } }
    }));
    assert_eq!(ask["result"]["isError"], true);
    assert!(ask["result"]["content"][0]["text"].as_str().unwrap().contains("dbpylot init"));

    // 5. EOF on stdin → clean exit.
    drop(stdin);
    let status = child.wait().expect("failed to wait for dbpylot mcp");
    assert!(status.success(), "dbpylot mcp exited nonzero: {status:?}");

    let _ = std::fs::remove_dir_all(&home);
}
