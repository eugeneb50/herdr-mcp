use super::*;
use herdr_mcp_api::ResolveRequest;
use herdr_mcp_core::RecipeAccess;

/// Every core tool exists (a rename would silently unlist it), the
/// defaults list exactly those, and a category or name lists more.
#[test]
fn mcp_tools_list_core_by_default() {
    let tools = HerdrMcpServer::tool_router().list_all();
    let listed = |entries: &[&str]| -> Vec<String> {
        tools
            .iter()
            .filter(|t| entries.iter().any(|e| lists(e, t, RecipeAccess::Tools)))
            .map(|t| t.name.to_string())
            .collect()
    };
    let defaults = herdr_mcp_core::McpConfig::default().tools;
    let entries: Vec<&str> = defaults.iter().map(String::as_str).collect();
    let core = listed(&entries);
    assert_eq!(core.len(), CORE_TOOLS.len(), "{core:?}");
    let extra = listed(&["core", "scheduler", "llm_call"]);
    assert_eq!(extra.len(), CORE_TOOLS.len() + 5, "{extra:?}");
    assert_eq!(listed(&["all"]).len(), tools.len());
}

/// `[mcp] recipes = "bridge"` drops the recipe tools from `core` only.
#[test]
fn bridge_recipes_leave_core() {
    let tools = HerdrMcpServer::tool_router().list_all();
    let listed = |entry: &str| -> Vec<String> {
        tools
            .iter()
            .filter(|t| lists(entry, t, RecipeAccess::Bridge))
            .map(|t| t.name.to_string())
            .collect()
    };
    let core = listed("core");
    assert_eq!(core.len(), CORE_TOOLS.len() - CORE_RECIPE_TOOLS.len());
    assert!(!core.iter().any(|t| t == "recipe_run"), "{core:?}");
    assert_eq!(listed("recipe_run"), ["recipe_run"]);
}

#[test]
fn slim_schema_drops_null_from_optional_types() {
    let mut schema = json!({
        "type": "object",
        "properties": {
            "label": { "type": ["string", "null"], "description": "d" },
            "type": { "type": "string" },
            "steps": { "type": ["array", "null"], "items": { "type": ["integer", "null"] } },
        },
        "anyOf": [{ "type": ["null", "boolean"] }],
    });
    slim_schema(schema.as_object_mut().unwrap());
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {
                "label": { "type": "string", "description": "d" },
                "type": { "type": "string" },
                "steps": { "type": "array", "items": { "type": "integer" } },
            },
            "anyOf": [{ "type": "boolean" }],
        })
    );
}
use crate::variables::RecipeStep;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::HashMap;

fn sample_results() -> HashMap<String, serde_json::Value> {
    let mut m = HashMap::new();
    m.insert(
        "step1".to_string(),
        serde_json::json!({
            "result": { "content": ["alpha", "beta"], "text": "hello" },
            "pane_id": "p1"
        }),
    );
    m.insert(
        "step2".to_string(),
        serde_json::json!({ "output": "world", "n": 42 }),
    );
    m
}

#[test]
fn pane_vars_overlays_handles_on_live_panes() {
    let panes = HashMap::from([
        (
            "w1:p1".to_string(),
            json!({"pane_id": "w1:p1", "label": "builder", "cwd": "/w", "agent": "claude"}),
        ),
        ("w1:p2".to_string(), json!({"pane_id": "w1:p2"})),
    ]);
    let handle = json!({"pane_id": "w1:p1", "role": "builder", "agent": "", "output": "ok"});
    let closed = json!({"pane_id": "w1:p9", "role": "gone", "tab_id": "w1:t9"});
    let handles = HashMap::from([
        ("w1:p1".to_string(), handle.clone()),
        ("builder".to_string(), handle),
        ("w1:p9".to_string(), closed.clone()),
        ("gone".to_string(), closed),
    ]);
    let vars = pane_vars(panes, handles);
    let want = json!({
        "pane_id": "w1:p1", "label": "builder", "cwd": "/w", "agent": "claude",
        "role": "builder", "output": "ok"
    });
    assert_eq!(vars["w1:p1"], want);
    assert_eq!(vars["builder"], want);
    assert_eq!(vars["w1:p2"], json!({"pane_id": "w1:p2"}));
    assert_eq!(vars.len(), 3);
}

#[test]
fn test_resolve_json_path_top_level() {
    let root = sample_results();
    let v = resolve_json_path(&root, "step2.output");
    assert_eq!(v, Some(serde_json::json!("world")));
}

#[test]
fn test_resolve_json_path_nested() {
    let root = sample_results();
    let v = resolve_json_path(&root, "step1.result.text");
    assert_eq!(v, Some(serde_json::json!("hello")));
}

#[test]
fn test_resolve_json_path_array_index() {
    let root = sample_results();
    let v = resolve_json_path(&root, "step1.result.content[0]");
    assert_eq!(v, Some(serde_json::json!("alpha")));
}

#[test]
fn test_resolve_json_path_array_nested() {
    let root = sample_results();
    let v = resolve_json_path(&root, "step1.result.content[1]");
    assert_eq!(v, Some(serde_json::json!("beta")));
}

#[test]
fn test_resolve_json_path_missing_returns_none() {
    let root = sample_results();
    assert_eq!(resolve_json_path(&root, "step1.nope"), None);
    assert_eq!(resolve_json_path(&root, "nope.field"), None);
}

#[test]
fn test_resolve_json_path_bare_key_is_the_value() {
    let root = sample_results();
    assert_eq!(
        resolve_json_path(&root, "step2"),
        Some(serde_json::json!({ "output": "world", "n": 42 }))
    );
    assert_eq!(resolve_json_path(&root, "nope"), None);
}

/// Tool results read naturally: `{{step}}` / `{{step.text}}` is the
/// text the tool returned, and fields of JSON returned as text resolve
/// directly (`{{exec.output}}`). Dotted scope keys resolve as-is.
#[test]
fn test_resolve_tool_results_by_text_and_payload() {
    let mut root = HashMap::new();
    root.insert(
        "read".to_string(),
        serde_json::json!({ "content": [{ "type": "text", "text": "line 1\nline 2" }] }),
    );
    root.insert(
        "exec".to_string(),
        serde_json::json!({ "content": [{ "type": "text",
            "text": r#"{"exit_code":0,"output":"ok","pane_id":"w1:p2"}"# }] }),
    );
    root.insert("template.pane".to_string(), serde_json::json!("w1:p3"));
    let get = |p: &str| resolve_json_path(&root, p);
    assert_eq!(get("read"), Some(serde_json::json!("line 1\nline 2")));
    assert_eq!(get("read.text"), get("read"));
    assert_eq!(get("exec.output"), Some(serde_json::json!("ok")));
    assert_eq!(get("exec.exit_code"), Some(serde_json::json!(0)));
    assert_eq!(
        get("exec.content[0].type"),
        Some(serde_json::json!("text")),
        "raw result paths still work"
    );
    assert_eq!(get("template.pane"), Some(serde_json::json!("w1:p3")));
    assert_eq!(get("exec.nope"), None);
}

#[test]
fn test_resolve_variables_simple_substitution() {
    let results = sample_results();
    let mut value = serde_json::json!({ "text": "{{ step2.output }}" });
    resolve_variables(&mut value, &results);
    assert_eq!(value, serde_json::json!({ "text": "world" }));
}

#[test]
fn test_resolve_variables_nested_object() {
    let results = sample_results();
    let mut value = serde_json::json!({ "a": { "b": "{{ step1.result.text }}" } });
    resolve_variables(&mut value, &results);
    assert_eq!(value, serde_json::json!({ "a": { "b": "hello" } }));
}

#[test]
fn test_resolve_variables_array() {
    let results = sample_results();
    let mut value = serde_json::json!(["{{ step2.output }}", "literal"]);
    resolve_variables(&mut value, &results);
    assert_eq!(value, serde_json::json!(["world", "literal"]));
}

#[test]
fn test_resolve_variables_unknown_kept() {
    let results = sample_results();
    let mut value = serde_json::json!({ "text": "{{ unknown.path }}" });
    resolve_variables(&mut value, &results);
    // Unknown variable left unchanged
    assert_eq!(value, serde_json::json!({ "text": "{{ unknown.path }}" }));
}

#[test]
fn test_resolve_variables_whitespace_in_braces() {
    let results = sample_results();
    let mut value = serde_json::json!({ "text": "{{  step2.output  }}" });
    resolve_variables(&mut value, &results);
    assert_eq!(value, serde_json::json!({ "text": "world" }));
}

#[test]
fn test_resolve_variables_multiple_in_string() {
    let results = sample_results();
    let mut value = serde_json::json!({ "text": "{{ step1.result.text }} {{ step2.output }}" });
    resolve_variables(&mut value, &results);
    assert_eq!(value, serde_json::json!({ "text": "hello world" }));
}

#[test]
fn test_resolve_variables_string_value_inserted() {
    let results = sample_results();
    let mut value = serde_json::json!({ "t": "hi {{ step2.output }}" });
    resolve_variables(&mut value, &results);
    assert_eq!(value, serde_json::json!({ "t": "hi world" }));
}

#[test]
fn unresolved_placeholders_lists_each_once() {
    let mut v = serde_json::json!({"a": "{{ x }} {{ y.z }}", "b": ["{{ x }}", 1], "c": "{{ ok }}"});
    resolve_variables(
        &mut v,
        &HashMap::from([("ok".to_string(), serde_json::json!("1"))]),
    );
    assert_eq!(unresolved_placeholders(&v), ["{{ x }}", "{{ y.z }}"]);
}

#[test]
fn test_resolve_variables_number_value_inserted() {
    let results = sample_results();
    let mut value = serde_json::json!({ "t": "n={{ step2.n }}" });
    resolve_variables(&mut value, &results);
    assert_eq!(value, serde_json::json!({ "t": "n=42" }));
}

#[test]
fn test_resolve_variables_object_value_to_string() {
    let results = sample_results();
    let mut value = serde_json::json!({ "t": "{{ step1.result }}" });
    resolve_variables(&mut value, &results);
    // Object serializes to its JSON string form
    assert_eq!(
        value,
        serde_json::json!({ "t": "{\"content\":[\"alpha\",\"beta\"],\"text\":\"hello\"}" })
    );
}

#[test]
fn test_resolve_variables_no_substitution() {
    let results = sample_results();
    let mut value = serde_json::json!({ "t": "no placeholders here" });
    resolve_variables(&mut value, &results);
    assert_eq!(value, serde_json::json!({ "t": "no placeholders here" }));
}

#[test]
fn test_json_value_to_string_null() {
    assert_eq!(json_value_to_string(&serde_json::Value::Null), "");
}

#[test]
fn test_json_value_to_string_string() {
    assert_eq!(json_value_to_string(&serde_json::json!("x")), "x");
}

#[test]
fn test_json_value_to_string_number() {
    assert_eq!(json_value_to_string(&serde_json::json!(7)), "7");
}

// ── Clipboard passthrough ────────────────────────────────────────────

/// Build a server with default config (no explicit clipboard commands).
fn clipboard_test_server() -> HerdrMcpServer {
    let tmp = tempfile::tempdir().unwrap();
    HerdrMcpServer::new(
        Persistence::new(tmp.path().join("data")),
        AgentRegistry::new(),
    )
}

#[tokio::test]
async fn agent_spawn_fails_on_unknown_need_before_spawning() {
    let server = clipboard_test_server();
    let args = serde_json::json!({"agent": "shell", "needs": ["ghost"]});
    let (_, err) = dispatch_tool(&server, "agent_spawn", args, Origin::Api)
        .await
        .unwrap_err();
    assert!(err.contains("needs: no agent 'ghost'"), "{err}");
}

/// Two saves at once (two rows, or the web UI and the TUI) both land,
/// live and in the file.
#[tokio::test]
async fn concurrent_setting_saves_keep_both() {
    use herdr_mcp_api::settings::{PANE_EXEC_MS, TUI_REFRESH_MS, Value};
    let mut server = clipboard_test_server();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("herdr-mcp.toml");
    server.config_path = path.clone();
    for _ in 0..20 {
        let (a, b) = tokio::join!(
            server.save_setting(PANE_EXEC_MS.into(), Some(Value::Number(5_000))),
            server.save_setting(TUI_REFRESH_MS.into(), Some(Value::Number(1_000))),
        );
        a.unwrap().unwrap();
        b.unwrap().unwrap();
        let live = server.settings();
        assert_eq!(live.number(PANE_EXEC_MS), 5_000);
        assert_eq!(live.number(TUI_REFRESH_MS), 1_000);
        let file: herdr_mcp_core::Config =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(file.settings().0, live);
        server.reset_settings().await.unwrap();
    }
}

#[tokio::test]
async fn save_clipboard_config_writes_file_and_applies_live() {
    let mut server = clipboard_test_server();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("herdr-mcp.toml");
    server.config_path = path.clone();
    std::fs::write(&path, "# keep me\n[http]\nport = 1\n").unwrap();

    server
        .save_clipboard_config(Some(("wl-copy".into(), "wl-paste".into())))
        .await
        .unwrap();

    let live = server.clipboard_config();
    assert_eq!(live.copy_command.as_deref(), Some("wl-copy"));
    assert_eq!(live.paste_command.as_deref(), Some("wl-paste"));
    let file = std::fs::read_to_string(&path).unwrap();
    assert!(file.contains("# keep me") && file.contains("copy-command = \"wl-copy\""));

    // A file that doesn't parse is left alone and the live config is kept.
    std::fs::write(&path, "[http\n").unwrap();
    assert!(
        server
            .save_clipboard_config(Some(("a".into(), "b".into())))
            .await
            .is_err()
    );
    assert_eq!(server.clipboard_config(), live);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "[http\n");

    // None clears the override: back to the built-in clipboard.
    std::fs::write(
        &path,
        "[clipboard]\ncopy-command = \"a\"\npaste-command = \"b\"\n",
    )
    .unwrap();
    server.save_clipboard_config(None).await.unwrap();
    assert_eq!(server.clipboard_config().commands(), None);
    assert!(
        !std::fs::read_to_string(&path)
            .unwrap()
            .contains("clipboard")
    );
}

// The tools run the configured commands (the built-in OS clipboard needs
// a display, so it is not exercised here).
#[tokio::test]
async fn clipboard_tools_use_configured_commands() {
    let tmp = tempfile::tempdir().unwrap();
    let clip = tmp.path().join("clip.txt");
    let mut cfg = herdr_mcp_core::Config::default();
    cfg.clipboard.copy_command = Some(format!("tee {}", clip.display()));
    cfg.clipboard.paste_command = Some(format!("cat {}", clip.display()));
    let reg = AgentRegistry::new();
    let server = HerdrMcpServer::with_config(Persistence::new(tmp.path().join("d2")), reg, &cfg);

    server
        .clipboard_set(Parameters(ClipboardSetParams {
            text: "hello clipboard".into(),
        }))
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&clip).unwrap(), "hello clipboard");

    let got = server
        .clipboard_get(Parameters(ClipboardGetParams {}))
        .await
        .unwrap();
    let text = got
        .content
        .iter()
        .find_map(|c| c.as_text())
        .map(|t| t.text.as_str());
    assert_eq!(text, Some("hello clipboard"));
}

// ── resolve_label_in_panes tests ──────────────────────────────────

fn pane(id: &str, label: Option<&str>) -> herdr_mcp_api::HerdrPane {
    serde_json::from_value(serde_json::json!({
        "pane_id": id, "workspace_id": "w1", "tab_id": "w1:t1", "label": label
    }))
    .unwrap()
}

#[test]
fn test_resolve_label_matches_pane_id() {
    let panes = [pane("p0", Some("bar")), pane("p1", Some("foo"))];
    assert_eq!(resolve_label_in_panes(&panes, "foo"), Ok("p1".into()));
}

#[test]
fn test_resolve_label_no_match() {
    let err = resolve_label_in_panes(&[pane("p1", Some("bar"))], "foo").unwrap_err();
    assert!(err.contains("available labels: bar"), "{err}");
}

#[test]
fn test_resolve_label_duplicate() {
    let panes = [pane("p1", Some("foo")), pane("p2", Some("foo"))];
    let err = resolve_label_in_panes(&panes, "foo").unwrap_err();
    assert!(err.contains("Multiple panes"), "{err}");
}

#[test]
fn test_resolve_label_no_labels_at_all() {
    let err = resolve_label_in_panes(&[pane("p1", None)], "foo").unwrap_err();
    assert!(err.contains("no labeled panes found"), "{err}");
    let err = resolve_label_in_panes(&[], "foo").unwrap_err();
    assert!(err.contains("no labeled panes found"), "{err}");
}

/// Contract test: the shared `herdr_mcp_api::ApiClient` (used by the web
/// UI and TUI) round-trips against the real router, so a drift between
/// server handlers and client types fails here, not in the browser.
#[test]
fn tool_is_error_matches_rmcp_wire_format() {
    let err = serde_json::to_value(CallToolResult::error(vec![Content::text("boom")])).unwrap();
    assert!(herdr_mcp_api::tool_is_error(&err));
    assert_eq!(herdr_mcp_api::tool_text(&err), "boom");
    let ok = serde_json::to_value(CallToolResult::success(vec![Content::text("x")])).unwrap();
    assert!(!herdr_mcp_api::tool_is_error(&ok));
}

#[test]
fn host_allowlist_blocks_rebinding_names() {
    for ok in [
        "localhost:7676",
        "LOCALHOST",
        "ui.localhost:1",
        "127.0.0.1:7676",
        "192.168.1.5",
        "[::1]:7676",
        "box:7676",
    ] {
        assert!(host_allowed(ok, "box"), "{ok}");
    }
    for bad in [
        "evil.example:7676",
        "localhost.evil.example",
        "",
        "[evil]:1",
    ] {
        assert!(!host_allowed(bad, "127.0.0.1"), "{bad}");
    }
}

#[test]
fn origin_allowlist_is_loopback_pages_only() {
    for ok in [
        "http://localhost:7676",
        "http://ui.localhost:8080",
        "https://127.0.0.1",
        "http://127.0.0.2:3000",
        "http://[::1]:7676",
        "http://box:7676",
    ] {
        assert!(origin_allowed(ok, "box"), "{ok}");
    }
    for bad in [
        "https://evil.example",
        "http://localhost.evil.example",
        "http://192.168.1.5",
        "http://[2001:db8::1]",
        "null",
        "file://",
        "",
    ] {
        assert!(!origin_allowed(bad, "box"), "{bad}");
    }
}

/// The playgrounds group tools by `_meta.category`; none may go missing.
#[test]
fn every_tool_has_a_category() {
    for tool in HerdrMcpServer::tool_router().list_all() {
        let json = serde_json::to_value(&tool).unwrap();
        assert!(
            json.pointer("/_meta/category")
                .and_then(|c| c.as_str())
                .is_some(),
            "{} has no _meta.category",
            tool.name
        );
    }
}

/// Command lines match herdr 0.9's CLI (checked against `herdr … --help`).
/// Bundled templates only call tools the server has, with params those
/// tools declare (catches renamed tools and stray or missing fields).
/// Checked against the schemas: nothing is executed.
#[test]
fn bundled_templates_use_real_tools() {
    let tools: HashMap<String, serde_json::Value> = HerdrMcpServer::tool_router()
        .list_all()
        .into_iter()
        .map(|t| {
            let schema = serde_json::Value::Object((*t.input_schema).clone());
            (t.name.to_string(), schema)
        })
        .collect();
    for t in tmpl::list_templates() {
        for step in &t.steps {
            let at = format!("template {} step {}", t.id, step.id);
            let schema = tools
                .get(&step.tool)
                .unwrap_or_else(|| panic!("{at}: no tool {}", step.tool));
            let props = schema["properties"]
                .as_object()
                .cloned()
                .unwrap_or_default();
            let params = step.params.as_object().cloned().unwrap_or_default();
            for key in params.keys() {
                assert!(props.contains_key(key), "{at}: unknown param {key}");
            }
            for req in schema["required"].as_array().into_iter().flatten() {
                let req = req.as_str().unwrap_or_default();
                assert!(params.contains_key(req), "{at}: missing param {req}");
            }
        }
    }
}

#[test]
fn pane_exec_captures_output_and_exit_code() {
    let line = exec_line("cargo test", "abc123");
    assert_eq!(
        line,
        r"cargo test; printf '\n%s_%s:%s\n' HERDR_DONE abc123 $?"
    );
    let done = Regex::new(&exec_done_regex("abc123")).unwrap();
    // The echoed command line must not satisfy the wait.
    assert!(!done.is_match(&format!("$ {line}")));

    let screen = format!(
        "$ old stuff\n$ {line}\nrunning 3 tests\ntest result: ok\n\nHERDR_DONE_abc123:101\n$ "
    );
    assert_eq!(
        exec_result(&screen, "abc123"),
        Some(("running 3 tests\ntest result: ok".into(), 101))
    );
    // No trailing newline from the command; echo scrolled out of view.
    let screen = "partial\nHERDR_DONE_abc123:0\n";
    assert_eq!(exec_result(screen, "abc123"), Some(("partial".into(), 0)));
    // Another run's marker doesn't count.
    assert_eq!(exec_result("HERDR_DONE_zzz:0\n", "abc123"), None);
    assert_eq!(tail("a\nb\nc", 2), "b\nc");
}

#[test]
fn herdr_09_argv() {
    assert_eq!(
        agent_submit_args("w1:p2", "hi"),
        ["agent", "prompt", "w1:p2", "hi"]
    );
    assert_eq!(
        agent_wait_args("rev", "idle", Some(5000)),
        [
            "agent",
            "wait",
            "rev",
            "--until",
            "idle",
            "--timeout",
            "5000"
        ]
    );
    assert_eq!(
        wait_output_args("p", "done", false, None, None),
        ["pane", "wait-output", "p", "--match", "done"]
    );
    assert_eq!(
        wait_output_args("p", "ok|err", true, Some("recent"), Some(10)),
        [
            "pane",
            "wait-output",
            "p",
            "--regex",
            "ok|err",
            "--source",
            "recent",
            "--timeout",
            "10"
        ]
    );
}

#[test]
fn watch_matches_counts_occurrences_and_finds_the_last_line() {
    let text = "say DONE when finished\nworking\nDONE\n";
    assert_eq!(watch_matches(text, "DONE", None), (2, Some("DONE")));
    assert_eq!(watch_matches(text, "nope", None), (0, None));
    // Lines are matched one at a time, so `^`/`$` anchor each line.
    let re = Regex::new(r"^DONE$").unwrap();
    assert_eq!(watch_matches(text, "", Some(&re)), (1, Some("DONE")));
}

#[tokio::test]
async fn security_policy_holds_for_the_bridge_runs_and_mcp_clients() {
    use herdr_mcp_api::{ApiClient, ApiError};

    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().join("data");
    let config = herdr_mcp_core::Config {
        data_dir: data_dir.clone(),
        security: herdr_mcp_core::SecurityConfig {
            disabled_tools: vec!["run_command".into()],
            require_token: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let registry = AgentRegistry::new();
    let server = HerdrMcpServer::with_config(Persistence::new(data_dir.clone()), registry, &config);
    let token = server
        .security
        .token
        .clone()
        .expect("require-token makes one");
    assert_eq!(
        std::fs::read_to_string(data_dir.join(herdr_mcp_api::TOKEN_FILE)).unwrap(),
        token
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = http_router(server.clone(), None, "127.0.0.1");
    tokio::spawn(async move { axum::serve(listener, app).await });
    let status = |r: Result<Vec<serde_json::Value>, ApiError>| match r {
        Err(ApiError::Status { status, .. }) => status,
        other => panic!("{other:?}"),
    };

    // No token: only the health probe answers.
    let anon = ApiClient::new(&base).unwrap();
    anon.health().await.unwrap();
    assert_eq!(status(anon.list_tools().await), 401);
    let wrong = ApiClient::new(&base)
        .unwrap()
        .with_token(Some("nope".into()));
    assert_eq!(status(wrong.list_tools().await), 401);

    let api = ApiClient::new(&base).unwrap().with_token(Some(token));
    let tools = api.list_tools().await.unwrap();
    assert!(tools.iter().any(|t| t["name"] == "var_set"));
    assert!(!tools.iter().any(|t| t["name"] == "run_command"));
    match api
        .call_tool("run_command", &serde_json::json!({ "command": "true" }))
        .await
    {
        Err(ApiError::Status { status: 403, body }) => {
            assert!(body.contains("disabled-tools"), "{body}");
        }
        other => panic!("{other:?}"),
    }

    // Triggers: the bridge API may add them; a recipe run and an MCP
    // client may not.
    let triggers = serde_json::json!([{ "on": "pane.agent_status_changed" }]);
    let saved = api
        .call_tool(
            "recipe_save",
            &serde_json::json!({ "name": "t", "steps": [], "triggers": triggers }),
        )
        .await
        .unwrap();
    let saved = herdr_mcp_api::tool_payload(&saved).unwrap();
    let run = dispatch_tool(
        &server,
        "recipe_run",
        serde_json::json!({
            "steps": [{
                "id": "s",
                "tool": "recipe_save",
                "params": { "recipe_id": saved["id"], "steps": [] },
            }],
            "wait": true,
        }),
        Origin::Api,
    )
    .await
    .unwrap();
    let run = herdr_mcp_api::tool_payload(&serde_json::to_value(run).unwrap()).unwrap();
    assert_eq!(run["status"], "failed", "{run}");
    assert!(run.to_string().contains("agent-triggers"), "{run}");
    let err = server
        .permit(Origin::Mcp, "schedule_recipe", None)
        .await
        .unwrap_err();
    assert!(err.message.contains("an MCP client"), "{}", err.message);
    assert!(server.permit(Origin::Mcp, "var_set", None).await.is_ok());
}

#[tokio::test]
async fn settings_crud_over_http_with_known_good_fallback() {
    use herdr_mcp_api::settings::{PANE_EXEC_MS, Value};
    use herdr_mcp_api::{ApiClient, ApiError};

    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().join("data");
    let registry = AgentRegistry::new();
    let mut server = HerdrMcpServer::new(Persistence::new(data_dir), registry);
    let cfg = tmp.path().join("herdr-mcp.toml");
    std::fs::write(&cfg, "# keep me\n").unwrap();
    server.config_path = cfg.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = http_router(server.clone(), None, "127.0.0.1");
    tokio::spawn(async move { axum::serve(listener, app).await });
    let api = ApiClient::new(&format!("http://{addr}")).unwrap();
    let info = |r: &herdr_mcp_api::SettingsResponse, key: &str| {
        r.settings.iter().find(|s| s.key == key).cloned().unwrap()
    };

    // Read: every spec, all on defaults.
    let all = api.settings().await.unwrap();
    assert_eq!(all.settings.len(), herdr_mcp_api::settings::SPECS.len());
    assert!(
        all.settings
            .iter()
            .all(|s| !s.overridden && s.value == s.default)
    );

    // Update: applied live and written to the file.
    let r = api
        .set_setting(PANE_EXEC_MS, &Value::Number(9_000))
        .await
        .unwrap();
    assert!(info(&r, PANE_EXEC_MS).overridden);
    assert_eq!(server.settings().number(PANE_EXEC_MS), 9_000);
    let keys = Value::Keys(vec!["Alt+]".into()]);
    let r = api.set_setting("keys.tab-next", &keys).await.unwrap();
    assert_eq!(
        info(&r, "keys.tab-next").value,
        Value::Keys(vec!["alt+]".into()])
    );
    let text = std::fs::read_to_string(&cfg).unwrap();
    assert!(text.contains("# keep me\n"), "{text}");
    assert!(text.contains("pane-exec-ms = 9000"), "{text}");

    // Bad values are refused with the reason, and nothing changes.
    let bad = [
        (PANE_EXEC_MS, Value::Number(1)),
        ("keys.tab-prev", Value::Keys(vec!["alt+]".into()])),
        ("keys.quit", Value::Keys(vec!["q".into()])),
        ("timeouts.nope", Value::Number(5_000)),
    ];
    for (key, value) in bad {
        match api.set_setting(key, &value).await {
            Err(ApiError::Status { status: 400, body }) => assert!(!body.is_empty()),
            other => panic!("{key}: {other:?}"),
        }
    }
    assert_eq!(std::fs::read_to_string(&cfg).unwrap(), text);

    // Delete one: back to its default; reset all: the known-good config.
    let r = api.reset_setting(PANE_EXEC_MS).await.unwrap();
    assert!(!info(&r, PANE_EXEC_MS).overridden);
    assert_eq!(server.settings().number(PANE_EXEC_MS), 120_000);
    let r = api.reset_settings().await.unwrap();
    assert!(r.settings.iter().all(|s| !s.overridden));
    assert_eq!(std::fs::read_to_string(&cfg).unwrap().trim(), "# keep me");
}

#[tokio::test]
async fn http_contract_roundtrip_with_api_client() {
    use herdr_mcp_api::{ApiClient, ApiError, CreateRecipeRequest};

    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().join("data");
    let registry = AgentRegistry::new();
    let server = HerdrMcpServer::new(Persistence::new(data_dir), registry);
    let web_dir = tmp.path().join("web");
    std::fs::create_dir_all(&web_dir).unwrap();
    std::fs::write(web_dir.join("index.html"), "<html>ui</html>").unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = http_router(server, Some(&web_dir), "127.0.0.1");
    tokio::spawn(async move { axum::serve(listener, app).await });
    let api = ApiClient::new(&format!("http://{addr}")).unwrap();

    // A body-less POST is a CORS "simple request": a website can send it
    // without a preflight, so a foreign Origin is refused outright.
    let http = reqwest::Client::new();
    let tool = format!("http://{addr}/api/tools/list_templates");
    let forged = http.post(&tool).header("origin", "https://evil.example");
    assert_eq!(forged.send().await.unwrap().status(), 403);
    let own = http.post(&tool).header("origin", format!("http://{addr}"));
    assert_eq!(own.send().await.unwrap().status(), 200);
    assert_eq!(http.post(&tool).send().await.unwrap().status(), 200);

    // A fake herdr (nextest runs each test in its own process, so the
    // env var stays here). Its pane labels echo the socket it was given.
    let fake = tmp.path().join("herdr");
    std::fs::write(
        &fake,
        r#"#!/bin/sh
case "$1 $2" in
"session list") echo '{"sessions":[{"name":"default","default":true,"running":true,"socket_path":"/s/d.sock","session_dir":"/s/d"},{"name":"work","default":false,"running":true,"socket_path":"/s/w.sock","session_dir":"/s/w"},{"name":"old","default":false,"running":false,"socket_path":"/s/o.sock","session_dir":"/s/o"}]}' ;;
"pane list") echo "{\"result\":{\"panes\":[{\"pane_id\":\"w1:p1\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t1\",\"focused\":true,\"agent_status\":\"idle\",\"agent\":\"claude\",\"label\":\"$HERDR_SOCKET_PATH\"},{\"pane_id\":\"w1:p2\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t2\",\"focused\":false,\"agent_status\":\"unknown\"}]}}" ;;
*) echo "fake herdr: $*" >&2; exit 1 ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    // SAFETY: this test owns its process (nextest); nothing else reads env concurrently.
    unsafe {
        std::env::set_var("HERDR_BIN", &fake);
        std::env::set_var("HERDR_SOCKET_PATH", "/s/d.sock");
    }

    let sessions = api.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 3);
    assert!(sessions[0].default && !sessions[2].running);
    assert_eq!(sessions.iter().filter(|s| s.current).count(), 1);
    assert!(sessions[0].current);
    let all = api.list_panes(&Default::default()).await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].label.as_deref(), Some("/s/d.sock"));
    let filter =
        |v: serde_json::Value| -> herdr_mcp_api::PaneFilter { serde_json::from_value(v).unwrap() };
    let idle = api
        .list_panes(&filter(
            serde_json::json!({"agent_status": "idle", "focused": true}),
        ))
        .await
        .unwrap();
    assert_eq!(idle.len(), 1);
    assert_eq!(idle[0].agent.as_deref(), Some("claude"));
    assert!(
        api.list_panes(&filter(serde_json::json!({"tab_id": "w1:t9"})))
            .await
            .unwrap()
            .is_empty()
    );
    let work = api
        .list_panes(&filter(serde_json::json!({"session": "work"})))
        .await
        .unwrap();
    assert_eq!(work[0].label.as_deref(), Some("/s/w.sock"));
    for bad in ["old", "nope"] {
        assert!(matches!(
            api.list_panes(&filter(serde_json::json!({"session": bad})))
                .await,
            Err(ApiError::Status { status: 502, .. })
        ));
    }
    // The tool returns the same typed list.
    let tool = api
        .call_tool("list_panes", &serde_json::json!({"label": "CLAUDE"}))
        .await
        .unwrap();
    let listed: herdr_mcp_api::PanesResponse = herdr_mcp_api::tool_payload(&tool)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap();
    assert_eq!(listed.panes.len(), 1);

    api.health().await.unwrap();
    let cfg = api.config().await.unwrap();
    assert!(cfg.data_dir.ends_with("data"));
    assert!(!cfg.config_path.is_empty());
    // Validation runs before any file write, so this never touches disk.
    let half = herdr_mcp_api::ClipboardConfig {
        copy_command: Some("  ".into()),
        paste_command: Some("xsel -o".into()),
    };
    assert!(matches!(
        api.set_clipboard_config(&half).await,
        Err(ApiError::Status { status: 400, .. })
    ));
    assert!(!api.list_tools().await.unwrap().is_empty());
    assert!(api.list_agents(None).await.unwrap().is_empty());
    assert!(!api.list_templates().await.unwrap().is_empty());
    let req = herdr_mcp_api::InstantiateTemplateRequest {
        variables: HashMap::from([("pane_id".into(), serde_json::json!("w1:p3"))]),
        name: None,
    };
    let filled = api.instantiate_template("git-status", &req).await.unwrap();
    assert!(filled.steps[0].params.to_string().contains("w1:p3"));
    assert!(
        api.list_recipes()
            .await
            .unwrap()
            .iter()
            .all(|r| r.id != filled.id),
        "instantiate does not save"
    );
    assert!(matches!(
        api.instantiate_template("nope", &req).await,
        Err(ApiError::Status { status: 404, .. })
    ));
    assert_eq!(
        api.auto_variables(Some("w9")).await.unwrap().workspace_id,
        "w9"
    );

    // Variables: 201 / 204 empty bodies must decode as Ok(()).
    let req = herdr_mcp_api::SaveVariableRequest {
        key: "greeting".into(),
        value: serde_json::json!("hi"),
        ..Default::default()
    };
    api.save_variable(&req).await.unwrap();
    assert_eq!(api.get_variable("greeting").await.unwrap().value, "hi");
    assert_eq!(api.list_variables().await.unwrap().len(), 1);
    // Resolve: the scope a run starts with; the request's own variables
    // win, globals fill in, the rest is reported.
    let resolved = api
        .resolve(&ResolveRequest {
            variables: HashMap::from([("who".into(), serde_json::json!("you"))]),
            value: serde_json::json!({ "text": "{{ greeting }} {{ who }}", "n": "{{ later.out }}" }),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(resolved.value["text"], "hi you");
    assert_eq!(resolved.unresolved, ["later.out"]);
    api.delete_variable("greeting").await.unwrap();

    // Runs: start in the background, poll to completion, legacy views.
    let step = |id: &str| herdr_mcp_api::RecipeStep {
        id: id.into(),
        tool: "list_templates".into(),
        params: serde_json::json!({}),
        description: None,
        extract: Vec::new(),
    };
    let started = api
        .start_run(&herdr_mcp_api::StartRunRequest {
            steps: Some(vec![step("a"), step("b")]),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut run = started.clone();
    for _ in 0..200 {
        run = api.get_run(started.id).await.unwrap();
        if run.status.is_finished() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(run.status, herdr_mcp_api::RunStatus::Completed);
    assert_eq!(run.results.len(), 2);
    let events = api.run_events(run.id, 0).await.unwrap();
    assert_eq!(events.len(), 6);
    assert!(events.last().unwrap().kind.is_terminal());
    assert_eq!(api.run_events(run.id, 5).await.unwrap().len(), 1);
    assert!(matches!(
        api.run_events(uuid::Uuid::new_v4(), 0).await,
        Err(ApiError::Status { status: 404, .. })
    ));
    // SSE: replay of a finished run arrives as `id: <seq>` + JSON data.
    let mut sse = reqwest::get(format!("http://{addr}/api/events?run={}&since=4", run.id))
        .await
        .unwrap();
    assert_eq!(
        sse.headers()["content-type"].to_str().unwrap(),
        "text/event-stream"
    );
    let mut body = String::new();
    while !body.contains("\"completed\"") {
        let chunk = tokio::time::timeout(std::time::Duration::from_secs(5), sse.chunk())
            .await
            .expect("SSE stalled")
            .unwrap()
            .expect("SSE ended early");
        body.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(body.contains("id: 4") && body.contains("id: 5") && !body.contains("id: 3"));
    assert!(
        api.list_runs(10)
            .await
            .unwrap()
            .iter()
            .any(|r| r.id == run.id)
    );
    assert!(matches!(
        api.control_run(run.id, herdr_mcp_api::RunAction::Cancel)
            .await,
        Err(ApiError::Status { status: 409, .. })
    ));
    assert!(matches!(
        api.get_run(uuid::Uuid::new_v4()).await,
        Err(ApiError::Status { status: 404, .. })
    ));
    // Profiles: bundled list, user override, invalid rejected, delete.
    let profiles = api.list_profiles().await.unwrap();
    assert!(
        profiles
            .iter()
            .any(|p| p.id == "zeroclaw" && !p.is_native())
    );
    assert!(profiles.iter().any(|p| p.id == "hermes" && p.is_native()));
    let mut mine = api.get_profile("shell").await.unwrap();
    mine.id = "python".into();
    mine.command = Some("python3".into());
    api.save_profile(&mine).await.unwrap();
    assert_eq!(api.get_profile("python").await.unwrap(), mine);
    mine.kind = Some("claude".into());
    assert!(matches!(
        api.save_profile(&mine).await,
        Err(ApiError::Status { status: 400, .. })
    ));
    api.delete_profile("python").await.unwrap();
    assert!(matches!(
        api.get_profile("python").await,
        Err(ApiError::Status { status: 404, .. })
    ));
    assert!(matches!(
        api.delete_profile("claude").await,
        Err(ApiError::Status { status: 404, .. })
    ));

    // ?wait=true returns the finished run.
    let waited = api
        .run_and_wait(&herdr_mcp_api::StartRunRequest {
            steps: Some(vec![step("only")]),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(waited.status, herdr_mcp_api::RunStatus::Completed);
    assert_eq!(waited.results.len(), 1);
    assert!(matches!(
        api.get_variable("greeting").await,
        Err(ApiError::Status { status: 404, .. })
    ));

    // Recipes CRUD.
    let created = api
        .create_recipe(&CreateRecipeRequest {
            name: "r1".into(),
            description: None,
            steps: vec![RecipeStep {
                id: "s1".into(),
                tool: "status".into(),
                params: serde_json::json!({}),
                description: None,
                extract: Vec::new(),
            }],
            variables: HashMap::new(),
            category: None,
            triggers: Vec::new(),
            gate: None,
            trail: Default::default(),
        })
        .await
        .unwrap();
    assert_eq!(api.get_recipe(created.id).await.unwrap().name, "r1");
    let trigger = herdr_mcp_api::RecipeTrigger {
        on: "pane.agent_status_changed".into(),
        when: serde_json::json!({"agent_status": "done"})
            .as_object()
            .cloned()
            .unwrap(),
    };
    let updated = api
        .update_recipe(
            created.id,
            &herdr_mcp_api::UpdateRecipeRequest {
                name: Some("r2".into()),
                triggers: Some(vec![trigger.clone()]),
                category: Some("smoke".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.name, "r2");
    assert_eq!(updated.category.as_deref(), Some("smoke"));
    assert_eq!(
        api.get_recipe(created.id).await.unwrap().triggers,
        vec![trigger]
    );
    assert_eq!(api.list_recipes().await.unwrap().len(), 1);
    api.delete_recipe(created.id).await.unwrap();
    assert!(api.list_recipes().await.unwrap().is_empty());

    // Unknown tool → typed 404, and the SPA fallback still serves the UI.
    assert!(matches!(
        api.call_tool("no_such_tool", &serde_json::json!({})).await,
        Err(ApiError::Status { status: 404, .. })
    ));
    let page = reqwest::get(format!("http://{addr}/playground"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(page, "<html>ui</html>");
}
