//! axon-a76: Spike — async-openai streaming tool_calls evaluation
//!
//! Run against neurons-service or any OpenAI-compat backend:
//!   OPENAI_BASE_URL=http://localhost:8080/v1 \
//!   OPENAI_API_KEY=x \
//!   MODEL=<model-name> \
//!   cargo run --example spike_async_openai
//!
//! What this checks:
//!   1. Do token deltas arrive as individual chunks (needed for streaming UI)?
//!   2. Do tool_calls arrive as partial-argument delta chunks or fully assembled?
//!   3. Is the async-openai streaming API ergonomic for the agentic loop?
//!
//! Record the verdict in axon-a76 before closing the ticket.

use async_openai::{
    Client,
    config::OpenAIConfig,
    types::chat::{
        ChatCompletionRequestUserMessageArgs, ChatCompletionTool, ChatCompletionTools,
        CreateChatCompletionRequestArgs, FunctionObject,
    },
};
use futures::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base_url =
        std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "http://localhost:8080/v1".to_string());
    let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| "x".to_string());
    let model = std::env::var("MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());

    println!("Backend : {base_url}");
    println!("Model   : {model}");
    println!("{}", "─".repeat(60));

    let client = Client::with_config(
        OpenAIConfig::new()
            .with_api_base(&base_url)
            .with_api_key(&api_key),
    );

    // ── Part 1: plain token streaming ─────────────────────────────
    println!("\n[1] Plain streaming — counting token delta chunks\n");

    let plain_req = CreateChatCompletionRequestArgs::default()
        .model(&model)
        .messages([ChatCompletionRequestUserMessageArgs::default()
            .content("Count from 1 to 5, one number per word, nothing else.")
            .build()?
            .into()])
        .stream(true)
        .build()?;

    let mut stream = client.chat().create_stream(plain_req).await?;
    let mut token_chunks = 0usize;
    let mut assembled_text = String::new();

    while let Some(Ok(resp)) = stream.next().await {
        for choice in &resp.choices {
            if let Some(content) = &choice.delta.content {
                token_chunks += 1;
                print!("{content}");
                assembled_text.push_str(content);
            }
        }
    }
    println!();
    println!("\n  → {token_chunks} token delta chunks  |  response: {assembled_text:?}");

    // ── Part 2: tool_calls streaming ──────────────────────────────
    println!("\n{}", "─".repeat(60));
    println!("\n[2] Tool call streaming — inspecting delta granularity\n");

    let tool = ChatCompletionTools::Function(ChatCompletionTool {
        function: FunctionObject {
            name: "read_file".to_string(),
            description: Some("Read a file from the local filesystem".to_string()),
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute path of the file to read"
                    }
                },
                "required": ["path"]
            })),
            strict: None,
        },
    });

    let tool_req = CreateChatCompletionRequestArgs::default()
        .model(&model)
        .messages([ChatCompletionRequestUserMessageArgs::default()
            .content("Read the file at /etc/hostname and tell me what it contains.")
            .build()?
            .into()])
        .tools(vec![tool])
        .stream(true)
        .build()?;

    let mut stream = client.chat().create_stream(tool_req).await?;
    let mut total_chunks = 0usize;
    let mut tool_chunks = 0usize;
    let mut assembled_args = String::new();

    while let Some(result) = stream.next().await {
        match result {
            Ok(resp) => {
                total_chunks += 1;
                for choice in &resp.choices {
                    let delta = &choice.delta;
                    if let Some(tool_calls) = &delta.tool_calls {
                        for tc in tool_calls {
                            tool_chunks += 1;
                            let name = tc.function.as_ref().and_then(|f| f.name.as_deref());
                            let args = tc
                                .function
                                .as_ref()
                                .and_then(|f| f.arguments.as_deref())
                                .unwrap_or("");
                            assembled_args.push_str(args);
                            println!(
                                "  chunk {total_chunks:>3} | tool_call idx={} id={:?} name={name:?} args_fragment={args:?}",
                                tc.index,
                                tc.id.as_deref().unwrap_or("-"),
                            );
                        }
                    } else if let Some(content) = &delta.content {
                        println!("  chunk {total_chunks:>3} | text: {content:?}");
                    }
                }
            }
            Err(e) => {
                eprintln!("  stream error: {e}");
                break;
            }
        }
    }

    // ── Verdict ───────────────────────────────────────────────────
    println!("\n{}", "═".repeat(60));
    println!("VERDICT\n");
    println!("  Token delta chunks : {token_chunks}");
    println!("  Total SSE chunks   : {total_chunks}");
    println!("  Tool call chunks   : {tool_chunks}");
    println!("  Assembled args     : {assembled_args}");
    println!();

    if token_chunks > 1 {
        println!("  ✓ Token streaming works — deltas arrive individually");
    } else {
        println!("  ✗ Token streaming may be assembled — only {token_chunks} chunk(s) seen");
    }

    match tool_chunks {
        0 => {
            println!("  ? No tool_calls chunks seen — model may have responded with text.");
            println!("    Try a model that reliably uses tools, or rephrase the prompt.");
        }
        1 => {
            println!("  ~ tool_calls arrived in a single chunk — async-openai may assemble");
            println!("    arguments before yielding. For Axon this is acceptable: we only");
            println!("    need the final args to dispatch the tool, not streaming progress.");
            println!("    → USE async-openai (ergonomic, handles SSE parsing for us).");
        }
        _ => {
            println!("  ✓ tool_calls deltas stream as fragments — raw chunks exposed.");
            println!("    → USE async-openai (gives us both streaming text AND tool deltas).");
        }
    }

    println!("{}", "═".repeat(60));
    Ok(())
}
