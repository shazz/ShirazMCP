//! Integration test: drives the compiled `shiraz-mcp` binary over stdio and
//! exercises the real JSON-RPC handshake — no Hatari, no display, not
//! `#[ignore]`d.

// [clippy::unwrap_used] clippy.toml's `allow-unwrap-in-tests` only exempts
// code nested directly inside a `#[test]`-attributed item; this file's
// helpers (spawn_server/send/recv/...) are plain fns called by the test, so
// they fall outside that detection. This whole binary IS test code — a
// panic on unexpected shape/timeout is the correct failure mode.
#![allow(
    clippy::unwrap_used,
    reason = "integration test helpers, not library code"
)]

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

async fn spawn_server() -> Child {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_shiraz-mcp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap()
}

async fn send(stdin: &mut ChildStdin, line: &str) {
    stdin.write_all(line.as_bytes()).await.unwrap();
    stdin.write_all(b"\n").await.unwrap();
    stdin.flush().await.unwrap();
}

async fn recv(reader: &mut BufReader<ChildStdout>) -> serde_json::Value {
    let mut line = String::new();
    let read = tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    assert!(read > 0, "server closed stdout before responding");
    serde_json::from_str(&line).unwrap()
}

#[tokio::test]
async fn initialize_tools_list_and_status_over_stdio() {
    let mut child = spawn_server().await;
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    let init = recv_after_init(&mut stdin, &mut stdout).await;
    assert_eq!(init["result"]["serverInfo"]["name"], "shiraz-mcp");
    assert_eq!(
        init["result"]["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION")
    );
    assert!(!init["result"]["capabilities"]["tools"].is_null());

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    )
    .await;

    let tools = list_tools(&mut stdin, &mut stdout).await;
    assert_status_tool(&tools);

    let call = call_tool(&mut stdin, &mut stdout, 3, "status", "{}").await;
    assert_status_result(&call);

    let plugins = call_tool(&mut stdin, &mut stdout, 4, "list_ripper_plugins", "{}").await;
    assert_list_ripper_plugins(&plugins);

    let err = call_tool(
        &mut stdin,
        &mut stdout,
        5,
        "extract",
        r#"{"finding_id": "1:0"}"#,
    )
    .await;
    assert_extract_no_findings_error(&err);

    child.kill().await.unwrap();
}

async fn recv_after_init(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
) -> serde_json::Value {
    let req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"handshake-test","version":"0.0.0"}}}"#;
    send(stdin, req).await;
    let resp = recv(stdout).await;
    assert_eq!(resp["id"], 1);
    resp
}

async fn list_tools(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
) -> serde_json::Value {
    send(
        stdin,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
    )
    .await;
    let resp = recv(stdout).await;
    assert_eq!(resp["id"], 2);
    resp
}

fn assert_status_tool(resp: &serde_json::Value) {
    let tools = resp["result"]["tools"].as_array().unwrap();
    let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort_unstable();
    let expected = [
        "assemble",
        "bootsector",
        "breakpoint",
        "cheat_scan",
        "debug",
        "depack",
        "disasm",
        "extract",
        "find_picture",
        "identify_music",
        "list_ripper_plugins",
        "machine_state",
        "patch",
        "pause",
        "profile",
        "profile_frame",
        "read_memory",
        "record_audio",
        "registers",
        "resume",
        "rip",
        "rip_sample",
        "run_frames",
        "save_prg",
        "screenshot",
        "search_memory",
        "spectrogram",
        "start",
        "state",
        "status",
        "stop",
        "view_memory_as_screen",
        "watch",
        "write_memory",
        "xrefs",
        "ym_state",
    ];
    assert_eq!(names, expected);
    let status = tools.iter().find(|t| t["name"] == "status").unwrap();
    assert_eq!(status["inputSchema"]["type"], "object");
}

async fn call_tool(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    id: u32,
    name: &str,
    arguments: &str,
) -> serde_json::Value {
    let req = format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{name}","arguments":{arguments}}}}}"#
    );
    send(stdin, &req).await;
    let resp = recv(stdout).await;
    assert_eq!(resp["id"], id);
    resp
}

fn assert_status_result(resp: &serde_json::Value) {
    // rmcp surfaces Json<T> tool output as `structuredContent`.
    let out = &resp["result"]["structuredContent"];
    assert_eq!(out["state"], "stopped");
    assert_eq!(out["hatari_path"], "/usr/bin/hatari");
    assert_eq!(out["version"], env!("CARGO_PKG_VERSION"));
}

fn assert_list_ripper_plugins(resp: &serde_json::Value) {
    // P9 adds three more packer pairs (atomik, powerpacker, rnc — each a
    // Packer-kind region detector + a depacker) after P7's bootsector
    // (registration order, §5.9). P12 appends the zx0 depacker (headerless,
    // ADR-25). P13 inserts the mpp picture detector after neochrome.
    let plugins = resp["result"]["structuredContent"]["plugins"]
        .as_array()
        .unwrap();
    assert_eq!(plugins.len(), 16);
    assert_eq!(plugins[0]["id"], "degas");
    assert_eq!(plugins[0]["role"], "detector");
    assert_eq!(plugins[1]["id"], "neochrome");
    assert_eq!(plugins[1]["role"], "detector");
    assert_eq!(plugins[2]["id"], "mpp");
    assert_eq!(plugins[2]["role"], "detector");
    assert_eq!(plugins[2]["kind"], "picture");
    assert_eq!(plugins[3]["id"], "pack-ice-region");
    assert_eq!(plugins[3]["role"], "detector");
    assert_eq!(plugins[4]["id"], "sndh");
    assert_eq!(plugins[4]["role"], "detector");
    assert_eq!(plugins[4]["kind"], "music");
    assert_eq!(plugins[5]["id"], "ym-player");
    assert_eq!(plugins[5]["role"], "detector");
    assert_eq!(plugins[5]["kind"], "music");
    assert_eq!(plugins[6]["id"], "bootsector");
    assert_eq!(plugins[6]["role"], "detector");
    assert_eq!(plugins[6]["kind"], "boot");
    assert_eq!(plugins[7]["id"], "atomik-region");
    assert_eq!(plugins[7]["role"], "detector");
    assert_eq!(plugins[7]["kind"], "packer");
    assert_eq!(plugins[8]["id"], "powerpacker-region");
    assert_eq!(plugins[8]["role"], "detector");
    assert_eq!(plugins[8]["kind"], "packer");
    assert_eq!(plugins[9]["id"], "rnc-region");
    assert_eq!(plugins[9]["role"], "detector");
    assert_eq!(plugins[9]["kind"], "packer");
    assert_eq!(plugins[10]["id"], "pack-ice");
    assert_eq!(plugins[10]["role"], "depacker");
    assert_eq!(plugins[11]["id"], "atomik");
    assert_eq!(plugins[11]["role"], "depacker");
    assert_eq!(plugins[12]["id"], "powerpacker");
    assert_eq!(plugins[12]["role"], "depacker");
    assert_eq!(plugins[13]["id"], "rnc");
    assert_eq!(plugins[13]["role"], "depacker");
    assert_eq!(plugins[14]["id"], "zx0");
    assert_eq!(plugins[14]["role"], "depacker");
    assert_eq!(plugins[15]["id"], "noop-deprotect");
    assert_eq!(plugins[15]["role"], "deprotect");
}

/// rmcp 2.2 surfaces `invalid_params` as a JSON-RPC error response (verified
/// against the actual wire response, not `isError: true` on a tool result).
fn assert_extract_no_findings_error(resp: &serde_json::Value) {
    let message = resp["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("no findings in memory"),
        "unexpected error message: {message}"
    );
}
