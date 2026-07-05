//! End-to-end tests: spawn the server binary in a temp fixture project and
//! drive it over stdio with raw JSON-RPC lines.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const VALIDATE_FN: &str = "export function validateUser(u: string): boolean {\n  // reject empty ids\n  return u.length > 0;\n}";

fn write_fixture(root: &Path) {
    std::fs::create_dir_all(root.join("src/utils")).unwrap();
    std::fs::create_dir_all(root.join("src/components")).unwrap();
    std::fs::create_dir_all(root.join("node_modules/somepkg")).unwrap();
    std::fs::write(root.join(".gitignore"), "node_modules/\ndist/\n").unwrap();
    std::fs::write(
        root.join("src/utils/api.ts"),
        format!(
            "{}\n\nexport class UserService {{\n  getUser(id: string): string {{\n    return id;\n  }}\n}}\n",
            VALIDATE_FN
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("src/components/Form.tsx"),
        r#"import React from 'react';
import { validateUser } from '../utils/api';

export interface FormProps {
  onSubmit: (data: UserData) => void;
}

export type UserData = { name: string };

export const Form = ({ onSubmit }: FormProps) => {
  validateUser('x');
  return <button />;
};
"#,
    )
    .unwrap();
    std::fs::write(
        root.join("node_modules/somepkg/index.ts"),
        "export const hidden = 1;\n",
    )
    .unwrap();
}

struct Server {
    child: Child,
    stdin: std::process::ChildStdin,
    rx: mpsc::Receiver<Value>,
    next_id: u64,
}

impl Server {
    fn start(root: &Path) -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_semantic_skeletonizer"))
            .arg("--root")
            .arg(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn server binary");
        let stdout = child.stdout.take().unwrap();
        let stdin = child.stdin.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    if tx.send(v).is_err() {
                        break;
                    }
                }
            }
        });
        let mut server = Server {
            child,
            stdin,
            rx,
            next_id: 1,
        };
        // The sweep runs in the background (initialize answers immediately)
        // and announces completion with list_changed — the two can arrive in
        // either order.
        let id = server.next_id;
        server.next_id += 1;
        server.send_raw(
            &json!({
                "jsonrpc": "2.0", "id": id, "method": "initialize",
                "params": {"protocolVersion": "2025-03-26", "capabilities": {}}
            })
            .to_string(),
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        let (mut got_init, mut got_sweep) = (false, false);
        while !(got_init && got_sweep) {
            assert!(Instant::now() < deadline, "initialize or initial sweep never completed");
            let Some(v) = server.recv(Duration::from_millis(200)) else {
                continue;
            };
            if v.get("id").and_then(|i| i.as_u64()) == Some(id) {
                assert_eq!(v["result"]["protocolVersion"], "2025-03-26");
                got_init = true;
            } else if v["method"] == "notifications/resources/list_changed" {
                got_sweep = true;
            }
        }
        server
    }

    fn send_raw(&mut self, line: &str) {
        writeln!(self.stdin, "{}", line).unwrap();
        self.stdin.flush().unwrap();
    }

    fn recv(&self, timeout: Duration) -> Option<Value> {
        self.rx.recv_timeout(timeout).ok()
    }

    /// Send a request and wait for its response, buffering nothing: any
    /// notifications that arrive while waiting are discarded.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.send_raw(&msg.to_string());
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Some(v) = self.recv(Duration::from_millis(200)) {
                if v.get("id").and_then(|i| i.as_u64()) == Some(id) {
                    return v;
                }
            }
        }
        panic!("timed out waiting for response to {}", method);
    }

    fn call_tool(&mut self, name: &str, args: Value) -> Value {
        self.request("tools/call", json!({"name": name, "arguments": args}))["result"].clone()
    }

    fn graph_keys(&mut self) -> Vec<String> {
        let res = self.request("resources/read", json!({"uri": "skeleton://project/global"}));
        let text = res["result"]["contents"][0]["text"].as_str().unwrap();
        let graph: Value = serde_json::from_str(text).unwrap();
        let mut keys: Vec<String> = graph.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    }

    /// Drain every message that arrives within `window`.
    fn drain(&self, window: Duration) -> Vec<Value> {
        let deadline = Instant::now() + window;
        let mut out = Vec::new();
        while Instant::now() < deadline {
            if let Some(v) = self.recv(Duration::from_millis(100)) {
                out.push(v);
            }
        }
        out
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn fixture_root(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().canonicalize().unwrap()
}

#[test]
fn protocol_basics_ping_version_and_parse_errors() {
    let dir = tempfile::tempdir().unwrap();
    let root = fixture_root(&dir);
    write_fixture(&root);
    let mut server = Server::start(&root);

    // ping returns an empty result
    let pong = server.request("ping", json!({}));
    assert_eq!(pong["result"], json!({}));

    // unsupported version -> server answers with its newest supported one
    let init = server.request("initialize", json!({"protocolVersion": "1999-01-01"}));
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");

    // unknown notification is silently ignored (no response, no crash)
    server.send_raw(r#"{"jsonrpc":"2.0","method":"notifications/whatever","params":{}}"#);
    assert!(server.recv(Duration::from_millis(400)).is_none());

    // malformed JSON gets a parse error
    server.send_raw("this is not json");
    let err = server.recv(Duration::from_secs(2)).expect("parse error response");
    assert_eq!(err["error"]["code"], -32700);

    // server still alive afterwards
    let pong = server.request("ping", json!({}));
    assert_eq!(pong["result"], json!({}));
}

#[test]
fn graph_lifecycle_watcher_and_subscriptions() {
    let dir = tempfile::tempdir().unwrap();
    let root = fixture_root(&dir);
    write_fixture(&root);
    let mut server = Server::start(&root);

    let expected = vec![
        "src/components/Form.tsx".to_string(),
        "src/utils/api.ts".to_string(),
    ];
    assert_eq!(server.graph_keys(), expected, "initial sweep uses canonical keys");

    // subscribe to the global URI and one file URI
    server.request("resources/subscribe", json!({"uri": "skeleton://project/global"}));
    server.request(
        "resources/subscribe",
        json!({"uri": "skeleton://project/file/src/utils/api.ts"}),
    );

    // D1: modifying a file must not create an absolute-path duplicate
    let api = root.join("src/utils/api.ts");
    let mut content = std::fs::read_to_string(&api).unwrap();
    content.push_str("\n// trailing comment\n");
    std::fs::write(&api, content).unwrap();

    let msgs = server.drain(Duration::from_secs(2));
    let updated_uris: Vec<&str> = msgs
        .iter()
        .filter(|m| m["method"] == "notifications/resources/updated")
        .filter_map(|m| m["params"]["uri"].as_str())
        .collect();
    assert!(
        updated_uris.contains(&"skeleton://project/file/src/utils/api.ts"),
        "expected per-file updated notification, got {:?}",
        msgs
    );
    assert!(updated_uris.contains(&"skeleton://project/global"));
    assert_eq!(server.graph_keys(), expected, "no duplicate node after save");

    // new file appears + list_changed (but no updated push: URI not subscribed)
    std::fs::write(root.join("src/new.ts"), "export const n = 1;\n").unwrap();
    let msgs = server.drain(Duration::from_secs(2));
    assert!(
        msgs.iter().any(|m| m["method"] == "notifications/resources/list_changed"),
        "expected list_changed after create, got {:?}",
        msgs
    );
    assert!(
        !msgs.iter().any(|m| m["method"] == "notifications/resources/updated"
            && m["params"]["uri"] == "skeleton://project/file/src/new.ts"),
        "unsubscribed URI must not receive updated pushes"
    );
    assert!(server.graph_keys().contains(&"src/new.ts".to_string()));

    // deleting removes the node
    std::fs::remove_file(root.join("src/new.ts")).unwrap();
    let msgs = server.drain(Duration::from_secs(2));
    assert!(msgs.iter().any(|m| m["method"] == "notifications/resources/list_changed"));
    assert_eq!(server.graph_keys(), expected);

    // gitignored churn never reaches the graph
    std::fs::write(
        root.join("node_modules/somepkg/index.ts"),
        "export const hidden = 2;\n",
    )
    .unwrap();
    let msgs = server.drain(Duration::from_secs(1));
    assert!(msgs.is_empty(), "node_modules event leaked: {:?}", msgs);
    assert_eq!(server.graph_keys(), expected);
}

#[test]
fn tools_cover_symbols_implementation_and_dependencies() {
    let dir = tempfile::tempdir().unwrap();
    let root = fixture_root(&dir);
    write_fixture(&root);
    let mut server = Server::start(&root);

    // list_symbols finds the arrow component and types
    let res = server.call_tool("list_symbols", json!({"file_path": "src/components/Form.tsx"}));
    let symbols: Vec<Value> =
        serde_json::from_str(res["content"][0]["text"].as_str().unwrap()).unwrap();
    let find = |n: &str| symbols.iter().find(|s| s["name"] == n).cloned();
    let form = find("Form").expect("Form missing");
    assert_eq!(form["kind"], "component");
    assert_eq!(form["exported"], true);
    assert_eq!(find("FormProps").unwrap()["kind"], "interface");
    assert_eq!(find("UserData").unwrap()["kind"], "type");

    // search_symbols spans the graph
    let res = server.call_tool("search_symbols", json!({"query": "validate"}));
    let hits: Vec<Value> =
        serde_json::from_str(res["content"][0]["text"].as_str().unwrap()).unwrap();
    assert!(hits
        .iter()
        .any(|h| h["file"] == "src/utils/api.ts" && h["name"] == "validateUser"));

    // get_implementation returns the exact original slice
    let res = server.call_tool(
        "get_implementation",
        json!({"file_path": "./src/utils/api.ts", "target_node": "validateUser"}),
    );
    let text = res["content"][0]["text"].as_str().unwrap();
    assert_eq!(text, VALIDATE_FN);
    assert!((text.len() as f64) < 1.2 * VALIDATE_FN.len() as f64);

    // ClassName.method addressing
    let res = server.call_tool(
        "get_implementation",
        json!({"file_path": "src/utils/api.ts", "target_node": "UserService.getUser"}),
    );
    let text = res["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("getUser(id: string)"));

    // unknown node -> isError with candidates
    let res = server.call_tool(
        "get_implementation",
        json!({"file_path": "src/utils/api.ts", "target_node": "nosuch"}),
    );
    assert_eq!(res["isError"], true);
    let text = res["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("validateUser") && text.contains("UserService.getUser"));

    // dependencies: both directions + externals
    let res = server.call_tool("get_dependencies", json!({"file_path": "src/utils/api.ts"}));
    let deps: Value = serde_json::from_str(res["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(deps["imported_by"], json!(["src/components/Form.tsx"]));
    let res = server.call_tool(
        "get_dependencies",
        json!({"file_path": "src/components/Form.tsx"}),
    );
    let deps: Value = serde_json::from_str(res["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(deps["imports"], json!(["src/utils/api.ts"]));
    assert_eq!(deps["external"], json!(["react"]));

    // tool-level failure is isError, not a JSON-RPC error
    let res = server.call_tool("list_symbols", json!({"file_path": "src/missing.ts"}));
    assert_eq!(res["isError"], true);
}

#[test]
fn empty_graph_returns_empty_object_with_note() {
    let dir = tempfile::tempdir().unwrap();
    let root = fixture_root(&dir);
    let mut server = Server::start(&root);

    let res = server.request("resources/read", json!({"uri": "skeleton://project/global"}));
    assert!(res.get("error").is_none(), "empty graph must not be a JSON-RPC error");
    let contents = res["result"]["contents"].as_array().unwrap();
    assert_eq!(contents[0]["text"], "{}");
    assert!(contents[1]["text"].as_str().unwrap().contains("empty"));
}

#[test]
fn resource_uris_are_percent_encoded_and_decoded() {
    let dir = tempfile::tempdir().unwrap();
    let root = fixture_root(&dir);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/odd name.ts"), "export const x = 1;\n").unwrap();
    let mut server = Server::start(&root);

    let res = server.request("resources/list", json!({}));
    let uris: Vec<&str> = res["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["uri"].as_str())
        .collect();
    assert!(
        uris.contains(&"skeleton://project/file/src/odd%20name.ts"),
        "got {:?}",
        uris
    );

    let res = server.request(
        "resources/read",
        json!({"uri": "skeleton://project/file/src/odd%20name.ts"}),
    );
    assert!(res.get("error").is_none());
    let text = res["result"]["contents"][0]["text"].as_str().unwrap();
    assert!(text.contains("x = 1"));
}
