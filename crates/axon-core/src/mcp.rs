//! MCP servers a session starts: their tools are offered next to the
//! built-in ones and run on the server that offered them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_openai::types::chat::ChatCompletionTools;
use axon_protocol::McpServer;
use futures::future::join_all;
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock, Tool};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::TokioChildProcess;
use serde_json::Value;

use crate::tools;

/// How long a server gets to start, and then to list its tools.
const START_TIMEOUT: Duration = Duration::from_secs(30);

/// How long one tool call may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// A connected server.
pub(crate) type Client = RunningService<RoleClient, ()>;

/// A started server with the tools it lists, or why it couldn't start.
type Listed = Result<(Client, Vec<Tool>), String>;

/// The MCP tools a session offers and the servers that run them.
/// Dropping it stops the servers.
#[derive(Default)]
pub(crate) struct Mcp {
    servers: Vec<Client>,
    /// The index in `servers` of each offered tool's server, by tool name.
    tools: HashMap<String, usize>,
    /// Tools that run without asking.
    allow: Vec<String>,
}

/// What starting a session's MCP servers gave.
#[derive(Default)]
pub(crate) struct Started {
    pub(crate) mcp: Mcp,
    /// The schemas of the tools to offer.
    pub(crate) schemas: Vec<ChatCompletionTools>,
    /// What went wrong, a message each; the session goes on without it.
    pub(crate) errors: Vec<String>,
}

/// Start `servers` in `cwd` and offer their tools, except those named
/// like a tool in `taken` (the tools already offered). `allow` names the
/// tools that run without asking.
pub(crate) async fn start(
    servers: Vec<McpServer>,
    cwd: PathBuf,
    taken: Vec<String>,
    allow: Vec<String>,
) -> Started {
    let listed = join_all(servers.into_iter().map(|server| {
        let cwd = &cwd;
        async move {
            let name = server.name.clone();
            let listed = match tokio::time::timeout(START_TIMEOUT, connect(server, cwd)).await {
                Ok(Ok(client)) => list(client).await,
                Ok(Err(e)) => Err(e),
                Err(_) => Err(timed_out(START_TIMEOUT)),
            };
            (name, listed)
        }
    }))
    .await;
    offer(listed, taken, allow)
}

/// Start `server` as a child process and initialise it.
async fn connect(server: McpServer, cwd: &Path) -> Result<Client, String> {
    let mut command = tokio::process::Command::new(&server.command);
    command
        .args(&server.args)
        .envs(server.env)
        .current_dir(cwd)
        // Also when Axon exits without stopping the session first.
        .kill_on_drop(true);
    let transport = TokioChildProcess::new(command)
        .map_err(|e| format!("{}: {e}", server.command.display()))?;
    ().serve(transport).await.map_err(|e| e.to_string())
}

/// `client` with the tools its server lists.
async fn list(client: Client) -> Listed {
    match tokio::time::timeout(START_TIMEOUT, client.list_all_tools()).await {
        Ok(Ok(tools)) => Ok((client, tools)),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(timed_out(START_TIMEOUT)),
    }
}

/// Offer the tools of each server `(name, listed)`, in order; see `start`.
/// With no servers, `allow` isn't checked: a profile's allow list is
/// meant for the sessions that get servers.
fn offer(servers: Vec<(String, Listed)>, taken: Vec<String>, allow: Vec<String>) -> Started {
    let mut started = Started::default();
    let check_allow = !servers.is_empty();
    for (name, listed) in servers {
        let (client, tools) = match listed {
            Ok(listed) => listed,
            Err(e) => {
                started.errors.push(format!("MCP server {name}: {e}"));
                continue;
            }
        };
        let index = started.mcp.servers.len();
        for tool in tools {
            let tool_name = tool.name.to_string();
            if taken.contains(&tool_name) || started.mcp.tools.contains_key(&tool_name) {
                started.errors.push(format!(
                    "MCP server {name}: skipped its tool {tool_name}: another tool has that name"
                ));
                continue;
            }
            started.schemas.push(tools::schema(
                &tool_name,
                tool.description.as_deref().unwrap_or_default(),
                Value::Object((*tool.input_schema).clone()),
            ));
            started.mcp.tools.insert(tool_name, index);
        }
        started.mcp.servers.push(client);
    }
    for name in allow.iter().filter(|_| check_allow) {
        if !started.mcp.tools.contains_key(name) {
            started
                .errors
                .push(format!("allow names {name}, but no MCP tool has that name"));
        }
    }
    started.mcp.allow = allow;
    started
}

impl Mcp {
    /// Whether an MCP server runs the tool `name`.
    pub(crate) fn runs(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Whether the tool `name` runs without asking.
    pub(crate) fn allows(&self, name: &str) -> bool {
        self.allow.iter().any(|allowed| allowed == name)
    }

    /// Call the tool `name` with `args`, the JSON text the model sent: its
    /// text, or why it failed.
    pub(crate) async fn call(&self, name: &str, args: &str) -> Result<String, String> {
        let server = &self.servers[self.tools[name]];
        let mut params = CallToolRequestParams::new(name.to_string());
        if let Some(arguments) = arguments(args)? {
            params = params.with_arguments(arguments);
        }
        match tokio::time::timeout(CALL_TIMEOUT, server.call_tool(params)).await {
            Err(_) => Err(timed_out(CALL_TIMEOUT)),
            Ok(Err(e)) => Err(e.to_string()),
            Ok(Ok(result)) => output(result),
        }
    }
}

/// The model's arguments as a JSON object; none when it sent nothing.
fn arguments(args: &str) -> Result<Option<serde_json::Map<String, Value>>, String> {
    if args.trim().is_empty() {
        return Ok(None);
    }
    match serde_json::from_str(args) {
        Ok(Value::Object(arguments)) => Ok(Some(arguments)),
        _ => Err("the arguments must be a JSON object".into()),
    }
}

/// A result's text content joined, other content as a placeholder; an
/// error result is the error.
fn output(result: CallToolResult) -> Result<String, String> {
    let parts: Vec<String> = result
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text(text) => text.text.clone(),
            ContentBlock::Image(_) => "[image]".into(),
            ContentBlock::Audio(_) => "[audio]".into(),
            _ => "[resource]".into(),
        })
        .collect();
    let text = parts.join("\n");
    if result.is_error == Some(true) {
        Err(text)
    } else {
        Ok(text)
    }
}

fn timed_out(after: Duration) -> String {
    format!("timed out after {} s", after.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        agent, end, prompt, prompt_answering, reply, respond, sent_messages, spawn_mcp, tool_calls,
    };
    use axon_protocol::{Change, Command, Event, SessionHandle, StopReason, ToolResult};
    use rmcp::ServerHandler;
    use rmcp::model::{
        CallToolResponse, ErrorData, ListToolsResult, PaginatedRequestParams, ServerCapabilities,
        ServerConfig,
    };
    use rmcp::service::{RequestContext, RoleServer};
    use serde_json::json;
    use std::sync::Arc;
    use wiremock::MockServer;

    /// An MCP server with a tool for each case; `read_file` clashes with
    /// the built-in tool.
    #[derive(Clone)]
    struct Tools;

    fn tool(name: &'static str, schema: Value) -> Tool {
        let Value::Object(schema) = schema else {
            panic!("a schema is an object")
        };
        Tool::new(name, format!("The {name} tool"), Arc::new(schema))
    }

    impl ServerHandler for Tools {
        fn get_info(&self) -> ServerConfig {
            ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
        }

        async fn list_tools(
            &self,
            _: Option<PaginatedRequestParams>,
            _: RequestContext<RoleServer>,
        ) -> Result<ListToolsResult, ErrorData> {
            let none = || json!({"type": "object", "properties": {}});
            Ok(ListToolsResult::with_all_items(vec![
                tool("get_time", none()),
                tool(
                    "open_app",
                    json!({"type": "object", "properties": {"name": {"type": "string"}}}),
                ),
                tool("broken", none()),
                tool("hang", none()),
                tool("read_file", none()),
            ]))
        }

        async fn call_tool(
            &self,
            request: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            let text = |text: String| vec![ContentBlock::text(text)];
            let result = match request.name.as_ref() {
                "get_time" => CallToolResult::success(text("12:00".into())),
                "open_app" => {
                    let name = &request.arguments.unwrap_or_default()["name"];
                    CallToolResult::success(text(format!("opened {name}")))
                }
                "broken" => CallToolResult::error(text("no such app".into())),
                _ => {
                    context.ct.cancelled().await;
                    return Err(ErrorData::internal_error("cancelled", None));
                }
            };
            Ok(result.into())
        }
    }

    /// A client connected to a `Tools` server running in this process.
    async fn connected() -> Client {
        let (client, server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let running = Tools.serve(server).await.unwrap();
            let _ = running.waiting().await;
        });
        ().serve(client).await.unwrap()
    }

    /// A session with the tools of `servers`, of which `allow` run without asking.
    async fn session(
        server: &MockServer,
        servers: Vec<(String, Listed)>,
        allow: &[&str],
    ) -> SessionHandle {
        let agent = agent(server, Path::new("."));
        let taken = agent
            .tools
            .iter()
            .map(|tool| tools::name(tool).to_string())
            .collect();
        let allow = allow.iter().map(|name| name.to_string()).collect();
        spawn_mcp(agent, async move { offer(servers, taken, allow) }).await
    }

    async fn with_tools(server: &MockServer, allow: &[&str]) -> SessionHandle {
        let listed = list(connected().await).await;
        session(server, vec![("test".into(), listed)], allow).await
    }

    fn error(message: &str) -> Event {
        Event::Error {
            message: message.into(),
        }
    }

    fn finished(events: &[Event]) -> Vec<&ToolResult> {
        events
            .iter()
            .filter_map(|event| match event {
                Event::ToolCallFinished { result, .. } => Some(result),
                _ => None,
            })
            .collect()
    }

    /// What the model was told about its last tool call.
    async fn tool_message(server: &MockServer) -> Value {
        let sent = sent_messages(server).await;
        let messages = sent.last().unwrap().as_array().unwrap();
        messages.last().unwrap()["content"].clone()
    }

    #[tokio::test]
    async fn the_servers_tools_are_offered_after_the_built_ins_skipping_a_clash() {
        let server = MockServer::start().await;
        respond(&server, reply(&["Hi"])).await;
        let mut handle = with_tools(&server, &[]).await;

        let events = prompt(&mut handle, "hi").await;

        assert_eq!(
            events[0],
            error("MCP server test: skipped its tool read_file: another tool has that name")
        );
        let requests = server.received_requests().await.unwrap();
        let body: Value = requests[0].body_json().unwrap();
        let names: Vec<&str> = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap())
            .collect();
        let built_in = crate::tools::schemas().len();
        assert_eq!(
            names[built_in..],
            ["get_time", "open_app", "broken", "hang"]
        );
        assert_eq!(names.iter().filter(|name| **name == "read_file").count(), 1);
        assert_eq!(
            body["tools"][built_in]["function"]["description"],
            "The get_time tool"
        );
    }

    #[tokio::test]
    async fn an_allowed_tool_runs_without_asking_and_the_model_gets_its_text() {
        let server = MockServer::start().await;
        respond(&server, tool_calls(&[("c1", "get_time", "{}")])).await;
        respond(&server, reply(&["Noon."])).await;
        let mut handle = with_tools(&server, &["get_time"]).await;
        handle.events.recv().await; // the clash

        let events = prompt(&mut handle, "time?").await;

        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::ApprovalRequested { .. })),
            "{events:?}"
        );
        assert_eq!(finished(&events), [&ToolResult::Output("12:00".into())]);
        assert_eq!(events.last(), Some(&end(StopReason::EndTurn)));
        assert_eq!(tool_message(&server).await, "12:00");
    }

    #[tokio::test]
    async fn a_tool_not_allowed_asks_first() {
        let server = MockServer::start().await;
        let args = r#"{"name": "Safari"}"#;
        respond(&server, tool_calls(&[("c1", "open_app", args)])).await;
        respond(&server, reply(&["Opened."])).await;
        respond(&server, tool_calls(&[("c2", "open_app", args)])).await;
        let mut handle = with_tools(&server, &["get_time"]).await;
        handle.events.recv().await;

        let approved = prompt_answering(&mut handle, "open safari", Some(true)).await;
        let rejected = prompt_answering(&mut handle, "again", Some(false)).await;

        let asked = Change::CallTool {
            tool: "open_app".into(),
            args: args.into(),
        };
        assert!(
            approved.contains(&Event::ApprovalRequested {
                id: 1,
                change: asked
            }),
            "{approved:?}"
        );
        assert_eq!(
            finished(&approved),
            [&ToolResult::Output(r#"opened "Safari""#.into())]
        );
        assert_eq!(finished(&rejected), [&ToolResult::Rejected]);
        assert_eq!(rejected.last(), Some(&end(StopReason::EndTurn)));
    }

    #[tokio::test]
    async fn an_error_result_fails_the_call() {
        let server = MockServer::start().await;
        respond(&server, tool_calls(&[("c1", "broken", "")])).await;
        respond(&server, reply(&["Sorry."])).await;
        let mut handle = with_tools(&server, &["broken"]).await;
        handle.events.recv().await;

        let events = prompt(&mut handle, "go").await;

        assert_eq!(
            finished(&events),
            [&ToolResult::Failed("no such app".into())]
        );
        assert_eq!(tool_message(&server).await, "error: no such app");
    }

    #[tokio::test]
    async fn arguments_that_are_not_an_object_fail_without_calling() {
        let server = MockServer::start().await;
        respond(&server, tool_calls(&[("c1", "get_time", "[1]")])).await;
        respond(&server, reply(&["Hm."])).await;
        let mut handle = with_tools(&server, &["get_time"]).await;
        handle.events.recv().await;

        let events = prompt(&mut handle, "go").await;

        assert_eq!(
            finished(&events),
            [&ToolResult::Failed(
                "the arguments must be a JSON object".into()
            )]
        );
    }

    #[tokio::test]
    async fn cancel_stops_a_running_call() {
        let server = MockServer::start().await;
        respond(&server, tool_calls(&[("c1", "hang", "{}")])).await;
        let mut handle = with_tools(&server, &["hang"]).await;
        handle.events.recv().await;
        handle
            .commands
            .send(Command::Prompt { text: "go".into() })
            .await
            .unwrap();
        let started = handle.events.recv().await;
        assert!(
            matches!(started, Some(Event::ToolCallStarted { .. })),
            "{started:?}"
        );

        handle.commands.send(Command::Cancel).await.unwrap();

        let events = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            [handle.events.recv().await, handle.events.recv().await]
        })
        .await
        .expect("the call should stop at once");
        assert_eq!(
            events,
            [
                Some(Event::ToolCallFinished {
                    id: 1,
                    result: ToolResult::Failed("cancelled by user".into())
                }),
                Some(end(StopReason::Cancelled))
            ]
        );
    }

    #[tokio::test]
    async fn a_server_that_fails_to_start_is_reported_and_so_is_an_allow_entry_matching_nothing() {
        let server = MockServer::start().await;
        respond(&server, reply(&["Hi"])).await;
        let agent = agent(&server, Path::new("."));
        let missing = McpServer {
            name: "gone".into(),
            command: "/no/such/server".into(),
            args: vec![],
            env: vec![],
        };
        let cwd = agent.cwd.clone();
        let started = start(vec![missing], cwd, vec![], vec!["get_time".into()]);
        let mut handle = spawn_mcp(agent, started).await;

        let events = prompt(&mut handle, "hi").await;

        let Event::Error { message } = &events[0] else {
            panic!("{events:?}")
        };
        assert!(
            message.starts_with("MCP server gone: /no/such/server: "),
            "{message}"
        );
        assert_eq!(
            events[1],
            error("allow names get_time, but no MCP tool has that name")
        );
        assert_eq!(events.last(), Some(&end(StopReason::EndTurn)));
    }

    #[tokio::test]
    async fn without_servers_the_allow_list_is_not_checked() {
        let server = MockServer::start().await;
        respond(&server, reply(&["Hi"])).await;
        let mut handle = session(&server, vec![], &["get_time"]).await;

        assert_eq!(
            prompt(&mut handle, "hi").await,
            [
                Event::TextDelta { text: "Hi".into() },
                end(StopReason::EndTurn)
            ]
        );
    }

    #[test]
    fn content_other_than_text_becomes_a_placeholder() {
        let result = CallToolResult::success(vec![
            ContentBlock::text("a"),
            ContentBlock::image("AAAA", "image/png"),
        ]);

        assert_eq!(output(result), Ok("a\n[image]".into()));
    }
}
