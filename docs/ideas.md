# Axon — Ideas

**Version:** 0.1.0
**Last updated:** 2026-10-27

A catalogue of ideas for the Axon agent harness. Nothing here is decided. Each entry is a candidate to pick up, design properly, and then build — or to reject with a reason.

This document stands alone. It consolidates earlier brainstorm notes, a review of those notes, a design conversation about ACP and the core loop, and a persistence spike specification. Those sources are retired; anything of value from them should be here.

---

## How to use this document

- **Pick an idea by its ID** (`C5`, `G3`, …) when you are ready to implement it. Design it then, against real code and a real model, not before.
- **Cards are deliberately not designs.** They record what the idea is, why it matters, the known footguns, facts and prior art gathered so far, and the questions a design must answer.
- **Parked ideas (§Z)** were set aside on purpose. Each says why and what evidence would bring it back, so it does not get re-argued from scratch.
- **When an idea is built or rejected,** update its card rather than deleting it: mark the status and note where the decision lives.

Card fields, used when they have content:

| Field | Meaning |
|---|---|
| **What** | The idea in a few sentences |
| **Why** | What it buys |
| **Footguns** | Ways it goes wrong, known in advance |
| **Facts & prior art** | Research already done: how other harnesses do it, server and API behaviour |
| **Open questions** | What a design has to answer |
| **At the target tier** | How the idea's value changes for 27B–35B models (see §0.2) |
| **Related** | Other cards |

---

## 0. Ground rules and environment

### 0.1 Guiding principles

- **Learning first.** The primary goal is to learn how agent harnesses work. Daily use is second; a product is a distant third. Breadth has value that pure YAGNI would cut — but say plainly when something is built to learn from rather than because it is needed.
- **The loop is trivial; the hard parts are what feeds it and what guards it.** Context assembly, retrieval, and editing decide quality. Permissions, budgets, and loop detection decide safety. Design effort belongs there.
- **Spend design effort where risk is, and where it can be validated.** An earlier round of notes put thousands of words into persistence and frontend protocols — things that cannot be tested until much later — and a few dozen into the context assembler and system prompt, which decide whether the harness works at all. Avoid repeating that.
- **Measure, then decide.** Several questions below (todo authorship, edit format, tool count, embeddings) are arguments until an evaluation set (J1) turns them into measurements.
- **Solo project: enforce with the compiler or CI, not review.** There is no reviewer. Any convention that relies on someone noticing will be broken.
- **Local models only.** No frontier-model token budget is assumed. Anything that needs a cheap strong model on tap is out of scope.
- **The harness compensates for model weakness** rather than delegating to model competence — scaled to the actual target tier, not a worst case.

### 0.2 Target models and hardware

- **Machine:** MacBook M2 Max, 64 GB unified memory.
- **Target models:** at least Qwen3.6-35B-A3B (mixture-of-experts, ~3B active parameters per token) or Qwen3.8-27B (dense). Chosen because they are reasonably good at tool calling; a 4B model would get in the way of daily use in Zed and the TUI.
- **Consequences for the design:**
  - Tool use is competent enough that the model can drive its own search and maintain a todo list. Several ideas originally justified by a 4B floor are now optional enhancements rather than load-bearing; their cards say so.
  - Context window is nominally large (64K was the working figure), but **effective** context is smaller: local models degrade well before the window is full. Keep context short, not just under the limit.
  - **Prompt processing (prefill) is the slow step on Apple Silicon.** Generation speed is acceptable, especially for the 3B-active MoE; re-processing a long history is what makes the agent feel slow. Prefix-cache reuse (B3) is therefore a first-order design constraint.
  - A dense 27B generates more slowly than a 3B-active MoE. If both are used, that difference is a reason to have a `ModelProfile` (A6).

### 0.3 Local serving facts

Recorded from research; verify against the build actually in use.

- **The context belongs to the request, not the connection.** The OpenAI-compatible API is stateless: every call sends the full message list. What the server keeps is the KV cache in a fixed number of **slots**.
- **llama-server:** `-c` sets total context, `-np` sets the number of slots. Classically each slot gets `c / np` tokens (`-c 131072 -np 2` → two 64K slots); newer builds can share a unified KV pool. A request goes to a free slot, preferably the one whose cached prefix matches best; `id_slot` pins a request to a slot. Within a slot, only the changed suffix of the prompt is processed.
- **Slots isolate caches, not speed.** With one slot, a subagent's request evicts the main loop's cache and the main loop pays a full re-prefill afterwards — tens of seconds on a long context. Two slots (main pinned to 0, subagents to 1) keep both caches warm. Weights are loaded once and shared; each slot costs only KV memory. Concurrent generation shares the GPU, so each runs slower; sequential use is fine.
- **Memory:** roughly 37 GB for Qwen3.6-35B-A3B at Q8, leaving room for two slots' KV cache on 64 GB.
- **LM Studio** runs llama.cpp underneath and supports concurrent requests per model, but gives less control over slots and pinning. Good for trying models; llama-server for a multi-agent setup.
- **Hybrid attention caveat.** The Qwen3.5/3.6 family mixes recurrent layers with standard attention. llama.cpp cannot always reuse an arbitrary prefix for such models and relies on saved checkpoints. **Watch "prompt tokens processed" in the server log across turns.** If the whole history is re-processed every turn, the cache strategy needs adjusting.
- **Tool-call parsing happens server-side.** llama-server's OpenAI endpoint parses the model's native tool-call tokens (Qwen's XML-style blocks) into standard `tool_calls`, but only when started with `--jinja` so it uses the model's chat template. vLLM does the same via `--tool-call-parser`. Raw native formats only appear on raw completion endpoints or when the parser does not recognise a brand-new model's format — which shows up as tool calls leaking into the text.

### 0.4 Building blocks

Crates identified so far:

| Need | Crate |
|---|---|
| Async runtime, HTTP | `tokio`, `reqwest` |
| SSE streaming | `eventsource-stream`, or `async-openai` to skip writing the client |
| Serialization, tool schemas | `serde`, `serde_json`, `schemars` (JSON Schema from Rust structs) |
| Search | `ignore`, `grep-searcher` (ripgrep's own libraries; `.gitignore`-aware) |
| Syntax / symbol maps | `tree-sitter` |
| Zed integration | `agent-client-protocol` (official Rust ACP SDK) |
| MCP | `rmcp` |
| Permission matching, config | `globset`, `toml` |
| TUI | `ratatui` |

---

## Index

| ID | Idea | Area |
|---|---|---|
| A1 | The minimal core loop | Loop & model I/O |
| A2 | Streaming and the stop reason | Loop & model I/O |
| A3 | One internal message type, adapters per provider | Loop & model I/O |
| A4 | Provider robustness | Loop & model I/O |
| A5 | Model routing | Loop & model I/O |
| A6 | `ModelProfile` and a probe suite | Loop & model I/O |
| A7 | Grammar-constrained tool calls | Loop & model I/O |
| A8 | Reasoning handling and thinking budgets | Loop & model I/O |
| A9 | Explicit completion and escalation | Loop & model I/O |
| A10 | Deterministic prompt preprocessing | Loop & model I/O |
| B1 | Log versus context | Context |
| B2 | The context assembler | Context |
| B3 | Prefix-cache-friendly ordering | Context |
| B4 | Limit what enters the context | Context |
| B5 | Tiered compaction | Context |
| B6 | The system prompt as a designed artifact | Context |
| B7 | Tool schemas as prompt surface; tool-count budget | Context |
| B8 | Provenance and prompt injection | Context |
| B9 | Secrets redaction at capture time | Context |
| B10 | Project instruction files, config layering, trust | Context |
| C1 | Baseline tool set | Tools & editing |
| C2 | Tool registry metadata | Tools & editing |
| C3 | The executor | Tools & editing |
| C4 | Error normalization | Tools & editing |
| C5 | The edit path | Tools & editing |
| C6 | Read-before-edit with change detection | Tools & editing |
| D1 | Harness-owned language-server client | Code intelligence |
| D2 | A stable symbolic tool surface that degrades | Code intelligence |
| D3 | Language-server readiness and freshness | Code intelligence |
| D4 | Language support as declarative manifests | Code intelligence |
| D5 | Repo map for orientation | Code intelligence |
| D6 | Proactive symbol injection | Code intelligence |
| E1 | Separate plan from act | Task control |
| E2 | The todo list | Task control |
| E3 | The verification loop | Task control |
| E4 | Budgets | Task control |
| E5 | Loop, stall, and spiral detection | Task control |
| E6 | Structured self-report | Task control |
| E7 | State machine as nested enums and a pure transition function | Task control |
| E8 | Cancellation and mid-turn steering | Task control |
| E9 | Subagents | Task control |
| F1 | Effect classes | Safety |
| F2 | Permission modes and profiles | Safety |
| F3 | Permission rule engine | Safety |
| F4 | Path jail | Safety |
| F5 | Shell safety and sandboxing | Safety |
| F6 | Approval memory | Safety |
| F7 | Hooks as the general mechanism | Safety / extensibility |
| G1 | The event log as the single source of truth | Persistence |
| G2 | Split storage by artifact | Persistence |
| G3 | Disposable snapshots; never truncate the log | Persistence |
| G4 | Deterministic replay | Persistence |
| G5 | Checkpoints and rewind | Persistence |
| G6 | Crash consistency | Persistence |
| G7 | Event schema evolution | Persistence |
| G8 | Session identity and concurrency | Persistence |
| G9 | Retention | Persistence |
| H1 | The core/frontend boundary: options | Frontends |
| H2 | Library core with a command/event API | Frontends |
| H3 | Every user action is a serializable command | Frontends |
| H4 | ACP: running inside Zed | Frontends |
| H5 | The TUI | Frontends |
| H6 | Exec (headless) mode | Frontends |
| H7 | Editor bridge as an MCP client | Frontends |
| H8 | Public events are a pure projection | Frontends |
| I1 | Harness or MCP? Two litmus tests | Extensibility |
| I2 | MCP client support | Extensibility |
| I3 | Skills with progressive disclosure | Extensibility |
| I4 | Plugins as a packaging format | Extensibility |
| I5 | Project memory | Extensibility |
| I6 | Backlog adapter | Extensibility |
| I7 | Web search | Extensibility |
| J1 | Evaluation set | Quality |
| J2 | Record and replay | Quality |
| J3 | A written scope fence per milestone | Quality |
| J4 | A first-milestone path | Quality |
| Z1–Z10 | Parked ideas | Parked |

---

## A. Core loop and model I/O

### A1 · The minimal core loop

**What.** Send messages and tool definitions to the model. If it returns tool calls, execute them, append the results, and call again. If it returns plain text with no tool calls, the turn ends (or escalates to the user).

```
assemble context → call model → parse output
  → if tool calls: execute, append results, repeat
  → else: done (or escalate)
```

**Why.** Everything hangs off this. Against an OpenAI-compatible endpoint the first working version is a few hundred lines — the fastest route to watching a real model use real tools, which is where most of the learning is.

**Footguns.**
- Treating "no tool calls" as the only completion signal (see A9).
- Unbounded iteration (see E4, E5).

**Related.** A2, A9, B2, C3, E7.

### A2 · Streaming and the stop reason

**What.** The client does not know in advance whether a response will contain tool calls. It streams events and decides at the end:

- Text deltas arrive first and are rendered immediately.
- If the model calls a tool, a new block starts partway through. Its JSON arguments arrive as fragments; collect them **by index** and parse only when the block ends.
- The message ends with a stop reason. Branch on it: `end_turn` / `stop` → turn over; `tool_use` / `tool_calls` → run tools and call again.

A single response can contain text followed by tool calls ("Let me look at the config" then `read_file`).

**Facts & prior art.**
- Anthropic: `content_block_start` of type `tool_use`, then `input_json_delta` events; stop reason `end_turn` or `tool_use`.
- OpenAI Chat Completions: `tool_calls` deltas; `finish_reason` `stop` or `tool_calls`.
- Local servers stream tool calls in the OpenAI shape; detecting where a tool call starts is the server parser's job (§0.3).

**Footguns.**
- Mid-stream cancellation leaves a partial message and possibly half-collected tool-call fragments. Decide what is logged (E8).

**Related.** A1, A3, E8.

### A3 · One internal message type, adapters per provider

**What.** Define one internal representation of messages, tool calls, and tool results. Write an adapter per provider that converts in both directions.

**Why.** Formats differ even though all hosted APIs return structured tool calls:
- Anthropic: content blocks; a tool call is a `tool_use` block with id, name, and input as JSON. Results go back as `tool_result` blocks in the next user message.
- OpenAI: `tool_calls` array on the assistant message, arguments as a JSON **string**. Results go back as `role: "tool"` messages.

**Footguns.** This adapter layer is where most provider-specific bugs show up. Test it with recorded responses (J2).

**Open questions.** Is supporting a hosted API worthwhile at all, given the local-only goal? It may be useful purely as a reference point for comparing behaviour.

**Related.** A2, A4, J2.

### A4 · Provider robustness

**What.**
- Retries with backoff; distinguish retryable (429, 5xx, timeout) from fatal (400, auth).
- Token accounting per request and per session.
- **Malformed tool-call JSON is expected**, not exceptional. Retry, or feed the parse error back to the model as a tool result, rather than crashing.
- Mid-stream cancellation (A2, E8).

**Related.** A2, C4, E4.

### A5 · Model routing

**What.** A configurable policy for which task hits which model: a cheap model for classification, summarization, and compaction; a strong model for planning and editing.

**At the target tier.** On one 64 GB machine, a second model costs memory that would otherwise be KV cache, and swapping models costs load time. Routing may mean "same model, different slot and settings" more often than "different model." Worth measuring before building.

**Open questions.** Is one model with per-step settings (A8) enough?

**Related.** A6, A8, E9, §0.3.

### A6 · `ModelProfile` and a probe suite

**What.** A struct the harness consults instead of hardcoding assumptions about the model. Candidate fields:

- `max_parallel_tool_calls`
- `effective_context_tokens` (not nominal)
- `max_tools_in_schema`
- `supports_grammar_constrained_output`
- `tool_call_reliability`
- `supports_reasoning_blocks`
- `prefers_search_replace_vs_diff`

Start as hand-written TOML for the models actually run. Later, a **probe suite** runs a fixed battery against an endpoint and writes the profile.

**Why.** The failure mode to avoid is `max_parallel_tool_calls = 1` hardcoded at twenty call sites, which is what happens by default when you develop against one model. The upgrade from hand-written to probed is purely additive — only the source of the profile changes.

**At the target tier.** Still worth it: Qwen3.6-35B-A3B and Qwen3.8-27B already differ (MoE vs dense, speed), and new models will keep arriving.

**Related.** A5, A7, B7, C5, E2.

### A7 · Grammar-constrained tool calls

**What.** Use constrained decoding so tool-call output is always valid JSON matching the schema.

**Facts.** Available on llama.cpp (GBNF grammars) and vLLM (guided decoding) as **extensions** to the OpenAI-compatible API, not part of it. A capability to detect, not assume — hence a `ModelProfile` field.

**At the target tier.** Less necessary than at 4B, but still a cheap way to eliminate A4's malformed-JSON path entirely.

**Related.** A4, A6.

### A8 · Reasoning handling and thinking budgets

**What.**
- **Thinking budget as a per-step knob:** deep on planning and debugging, shallow on mechanical edits. Reasoning tokens spent on a rename are pure cost.
- **Do not send previous turns' reasoning back** to Qwen in thinking mode.
- Dropping old reasoning traces is also a compaction tier (B5).

**Footguns.** Removing reasoning from earlier messages edits history and can invalidate the prefix cache from that point (B3). Decide whether reasoning is ever stored in the context's history in the first place.

**Related.** A5, B3, B5.

### A9 · Explicit completion and escalation

**What.** Give the model `task_done` and `ask_user` tools, so finishing and escalating are observable events the harness can act on, rather than inferred from the absence of tool calls.

**Why.** Makes completion something the state machine can verify — for example, refusing `task_done` while the verification item is still pending (E2, E3).

**Footguns.** Models often just answer in plain text and stop, with `finish_reason: stop`, without calling `task_done`. Treat that as a valid end of turn too, or the loop will spin. Decide whether a plain-text stop mid-task means "done", "asking", or "nudge once and continue".

**Related.** A1, E2, E3, H6.

### A10 · Deterministic prompt preprocessing

**What.** Do not spend an extra model call to "analyze the prompt first" — the main loop's model does that itself, and an extra call costs a full prefill. Do deterministic preprocessing instead:

- Expand slash commands into prompt templates.
- Resolve `@`-mentions into file contents or references.
- Attach editor context (open file, selection).
- Later, run prompt hooks (F7).

**Related.** B2, F7, H4, H7.

---

## B. Context

### B1 · Log versus context

**What.** Keep two separate representations of a conversation:

- **The log:** an append-only record of everything — every message, tool call, full tool output, timing. Stored, never edited.
- **The context:** what is actually sent to the model on this request. Computed from the log for each request.

Compaction, memory, skills, and injection then all become **rules for building the context from the log.**

**Why.** Information is never lost, and context rules can change without breaking stored sessions. This is probably the most important single idea in this document: it simplifies compaction, persistence (G1), replay (G4), and rewind (G5) at once.

**Related.** B2, B5, G1.

### B2 · The context assembler

**What.** Decides what goes into each request. Inputs, in layers:

1. System prompt: identity, how to use tools, environment facts (OS, cwd, git branch, project layout)
2. Memory / instruction files: user-wide, then project (B10)
3. Skill index (I3)
4. Tool definitions (B7)
5. Task specification and current plan / todo state (E1, E2)
6. Symbol context and diagnostics from the language server (D6)
7. Conversation history
8. The new user message

Must be **budget-aware**: given a token ceiling, decide what to drop, compress, or summarize.

**Why.** The highest-leverage module in the system. Earlier notes said so and then gave it nine bullet points; it deserves more design than anything in persistence or frontends.

**Open questions.**
- What is the budget split between layers?
- What happens when the fixed layers alone exceed the budget?

**Related.** B3–B10, D6, E2.

### B3 · Prefix-cache-friendly ordering

**What.** llama.cpp and MLX reuse the KV cache for any unchanged prompt prefix. On an M2 Max, prefill is the slow part, so cache hits are what make the agent feel fast. Three rules:

1. Stable content first; append history at the end.
2. Never put anything that changes every turn (a timestamp, a counter) in the system prompt.
3. Avoid editing earlier messages — any change invalidates the cache from that point on. This is why compaction should happen rarely and all at once.

**Footguns.**
- **Per-turn injection fights caching.** Re-injecting the todo list every turn (E2) or proactive symbol context (D6) adds content that changes each turn. Put it at the tail. If it stays in the history, the history grows; if it is removed later, the cache breaks from that point.
- Hybrid-attention models may not reuse arbitrary prefixes at all (§0.3). Measure.

**Related.** A8, B2, B5, D6, E2, §0.3.

### B4 · Limit what enters the context

**What.** The first and cheapest lever — matters more than any compaction scheme.

- Truncate every tool output **and say so**: "showing 200 of 1,400 lines, use `offset`".
- `read_file` with line ranges; search tools return matches, not whole files.
- For huge outputs (a 500 KB build log): head-plus-tail, head-only, or spill to a file and return the path.

**Related.** B5, C1, C4.

### B5 · Tiered compaction

**What.** When context fills, compact in tiers, cheapest first:

1. **Prune old tool results.** Replace with a stub like `[read src/agent.rs:1-200 — elided]`. The model can re-read if needed. Cheap, reliable, and frees the most space, since tool output is most of the context.
2. **Drop old reasoning traces.**
3. **Summarize older turns** into a structured state note — goal, decisions made, files touched, open questions — keeping the last few turns verbatim. Keep it factual and structured, not narrative.
4. **Hand off** to a new session seeded with that note when the conversation has drifted too far.

Always keep verbatim: the task specification, the current plan / todo list, and recent tool results.

Trigger at roughly 70–80% of the window, leaving room for the response — but local models degrade before the window is full, so aim to stay short, not just under the limit.

**Footguns.**
- **Summarization introduces hallucinations.** At least one local-agent project found that summarizing a coding session produced made-up details. Dropping old tool results usually works better than summarizing them. Treat tier 3 as the risky step.
- Compaction rewrites history and invalidates the prefix cache (B3). Do it rarely and in one go.

**Make it visible and reversible:** the user should see that compaction happened, and the full log (B1) means it can be undone.

**Related.** A8, B1, B3, B4, G9.

### B6 · The system prompt as a designed artifact

**What.** Treat the system prompt as the single largest determinant of harness quality: designed, versioned, and tested against the evaluation set (J1) — not a bullet inside context assembly.

**Open questions.** How is it versioned alongside the binary? Is it per-model (A6)?

**Related.** B2, B7, J1.

### B7 · Tool schemas as prompt surface; tool-count budget

**What.** Tool names, descriptions, and parameter schemas are what the model actually reads. Write descriptions for a reader choosing between tools under uncertainty. Keep parameters flat.

**Tool count is a budget.** More tools with rich descriptions degrade selection. The baseline set (C1) plus the symbolic tools (D2) is already around fourteen. Options: fewer tools, terser schemas, or tools hidden until relevant.

**At the target tier.** Much less acute than at 4B, but not gone. A dynamic tool set also changes the prefix (B3) and can confuse a model mid-session (D2), so "hide until relevant" has its own cost.

**Related.** A6, B6, C1, D2, J1.

### B8 · Provenance and prompt injection

**What.** Permissions (F) guard actions. Nothing yet guards **instructions arriving as data**: file contents, tool output, MCP responses. Clone a hostile repository and its README is in the context window.

Track provenance on everything entering the prompt, and hold a hard rule: content from data sources can never escalate permissions.

**Related.** B10, F1, I2.

### B9 · Secrets redaction at capture time

**What.** `.env` contents and credentials flow through tool results into an append-only log, in plaintext, permanently. Redact when the event is captured, not when it is displayed.

**Why.** Painful to retrofit precisely because the log is append-only (G1, G3).

**Related.** F3, G1, G3.

### B10 · Project instruction files, config layering, and trust

**What.**
- **Instruction files** (`AGENTS.md`-style) are always loaded; their cost is paid on every request, so keep them short. Load hierarchically: user-wide, then the repo root, then a subdirectory's file only once the agent touches files there.
- **Config resolution order:** CLI flag > project config > user config > default.
- **Trust on first use.** A file in the repository is untrusted input — a cloned repo carrying instructions or config is the `direnv` problem. Ask before honouring a new or changed project file.

**Related.** B2, B8, F3, I5.

---

## C. Tools and editing

### C1 · Baseline tool set

**What.**

| Tool | Notes |
|---|---|
| `read_file` | Line ranges, so large files don't blow the budget |
| `list_dir` | Cheap orientation |
| `glob` | Path patterns |
| `grep` | Literal/regex; still needed alongside symbolic tools and as their fallback |
| `find_definition`, `find_references`, `find_implementations`, `document_symbols`, `workspace_symbols` | Symbolic navigation (D2) |
| `apply_patch` | Preferred over whole-file writes (C5) |
| `write_file` | Ideally new files only |
| `run_command` | The dangerous one (F5) |
| `ask_user`, `task_done` | Explicit escalation and completion (A9) |
| `todo_write` | The todo list (E2) |

**Design notes.**
- Keep the set small with flat parameters; local-model tool calls get far more reliable.
- A reasonable start: `list_files`, `grep`, `read_file`; then edit; then shell.
- **Prefer patch over write:** cheaper in tokens, less destructive, reviewable as a diff.

**Related.** B7, C5, D2, F5.

### C2 · Tool registry metadata

**What.** Each tool declares: name, JSON schema (`schemars`), handler, **permission/effect class** (F1), **cost hint**, and whether it is **parallel-safe**.

**Related.** C3, F1, B7.

### C3 · The executor

**What.**
- Parallel dispatch for parallel-safe tools (reads, searches); serialized for writes.
- Timeouts and cancellation on every call.
- Each result is appended with its `tool_call_id`.
- Everything passes through the permission check (F3) first.

**Footguns.** Parallel results return out of order; the log must record them in a defined order so replay is deterministic (G4).

**Related.** C2, C4, E8, F3, G4.

### C4 · Error normalization

**What.** Failures come back to the model as tool results it can act on — "file not found; nearest matches are `src/lib.rs`, `src/main.rs`" — never as a raw error string, a panic, or a Rust error that ends the turn.

**Why.** Good error text is the cheapest capability improvement available. The same principle drives honest degradation notices (D2) and truncation notices (B4).

**Related.** A4, B4, D2.

### C5 · The edit path

**What.** The highest-failure-rate operation in every coding agent. Currently the least designed. A design needs:

- **Edit format:** unified diff, search/replace blocks, or line-range replacement. They fail very differently per model — hence `prefers_search_replace_vs_diff` in the profile (A6).
- **Repair strategy** for patches that do not apply: fuzzy matching, whitespace tolerance.
- **Does a failed patch cost a model turn,** or is it repaired silently?
- **Ambiguous match sites** (the search text appears twice) and **stale line numbers**.
- Patch over write (C1).

**Related.** A6, C1, C6, E3, H4, J1.

### C6 · Read-before-edit with change detection

**What.** Refuse to edit a file the agent has not read, and track the file's modification time or content hash at read time. If the file changed since (the user edited it in their editor), refuse and make the model re-read.

**Why.** Without it the agent silently clobbers the user's edits. Small mechanism, large correctness payoff.

**Facts.** In ACP mode, `fs/read_text_file` and `fs/write_text_file` go through Zed's open buffers, including unsaved changes, rather than hitting disk behind the editor's back (H4). That removes part of the race but not the need to detect staleness.

**Related.** C5, H4.

---

## D. Code intelligence

### D1 · Harness-owned language-server client

**What.** The harness runs an LSP client itself, uses it to answer symbolic queries, and exposes the results as tools (D2) and, optionally, as proactive injection (D6).

**Why language servers.** Retrieval breaks into query types with different best answers:

| Query | language server | grep | embeddings |
|---|---|---|---|
| Where is `handle_request` defined? | exact | noisy | worse |
| Who calls it? | exact (call hierarchy) | unreliable | can't |
| What does this type contain? | exact | no | no |
| What implements `Storage`? | exact | no | no |
| Where is rate limiting handled? | only if named that | good | plausible |
| How does this codebase do X? | no | no | genuinely this |
| What files matter for this task? (turn 1) | no | no | maybe |

The first four are what an agent hits constantly while editing, and a language server answers them with compiler-grade accuracy — resolving generics, trait implementations, and macro expansion, which no syntactic index can.

**Three shapes considered:**

| | Shape | Read |
|---|---|---|
| A | LSP exposed through an MCP server | Every lookup costs a model turn; retrieval becomes model-driven |
| B | **Harness owns the LSP client**; tools plus optional injection | Preferred |
| C | Custom index (tree-sitter or embeddings) | Reimplements, less accurately, what a language server provides |

**Value by tier.** Symbolic navigation is tier-independent: one `find_references` returning twelve exact locations replaces several greps and file reads at any model size. Proactive injection is tier-dependent (D6).

**Facts & prior art.** Serena (LSP-backed symbolic retrieval); Aider's repo map (D5). Neither uses GPU memory.

**Open questions.**
- Per-session or shared long-lived language server? Startup cost argues shared; isolation argues per-session.
- Concurrent sessions on one repo contend for one server or pay startup twice.

**Related.** D2–D6, I1, Z1.

### D2 · A stable symbolic tool surface that degrades

**What.** `find_definition`, `find_references`, `find_implementations`, `document_symbols`, `workspace_symbols` **always exist.** With a language server loaded they are exact. Without one they fall back to ripgrep heuristics and **say so** in the result: *"no language server for `.py`; fell back to text search, results may be incomplete."*

**Why.**
- Availability is per-file, not per-session: mixed-language repos are normal.
- Tools appearing and vanishing mid-session destabilize models and break the prefix cache (B3).

**Sequencing.** The tool contract plus ripgrep fallback is cheap to build early and expensive to change later, because it is model-facing. The real LSP backing can come later without the model noticing.

**Related.** B7, C4, D1, D3.

### D3 · Language-server readiness and freshness

**What.**
- **Readiness.** rust-analyzer on a large project takes tens of seconds to minutes before it can answer, and during that window it returns **partial results, not errors**. An agent told "0 references" for a symbol with thirty will delete something. Model server state explicitly — `starting` / `indexing` / `ready` / `degraded` — and never trust or inject from a non-ready server; fall back with the degradation notice.
- **Freshness.** The agent edits files, so the harness must send `didChange` notifications or answers go stale mid-task. Stale-but-authoritative-looking is worse than no answer.

**Facts.** `$/progress` is standard, but which token means "ready" is per-server. This is the strongest argument for a per-language adapter layer existing at all (D4).

**Related.** D1, D2, D4.

### D4 · Language support as declarative manifests

**What.** LSP is already the language-agnostic layer — `textDocument/definition`, `textDocument/references`, `callHierarchy/incomingCalls`, `workspace/symbol` are the same across rust-analyzer, gopls, pyright. What varies per language is configuration, lifecycle, and quirks:

| Concern | Why per-language |
|---|---|
| Binary + args | `rust-analyzer` vs `gopls` vs `pyright-langserver --stdio` vs `clangd` |
| Project root detection | `Cargo.toml` vs `go.mod` vs `pyproject.toml` vs `compile_commands.json` |
| `initializationOptions` | Very different per server |
| Readiness | No standard "done indexing" signal |
| Capability gaps | Not every server implements call hierarchy |
| Diagnostics semantics | rust-analyzer surfaces cargo errors; pyright gives type errors only |

So: a TOML manifest per language (binary, args, root markers, init options, readiness heuristics) consumed by one generic client, with **compiled-in adapters** as the escape hatch for pathological servers. Adding a language is a text file.

**Sequencing.** Start with rust-analyzer only (dogfooded daily, since Axon is Rust). Design the manifest schema after two or three servers are integrated, not before.

**Related.** D1, D3, Z2.

### D5 · Repo map for orientation

**What.** Turn 1 — "add rate limiting to the API" — has no symbol to look up. A **repo map** answers orientation: file tree plus a symbol outline per file, budget-capped, ranked by git recency and import centrality. Can be generated from `workspace/symbol` and document symbols on an already-running server, or from `tree-sitter`.

**Facts & prior art.** Aider's repo map: tree-sitter symbols ranked over the reference graph.

**Open questions.** Built eagerly at session start (startup cost the user feels) or lazily on first orientation query (costs a turn)? Cache keyed by commit SHA and adapter versions (G2).

**Related.** D1, G2, Z1.

### D6 · Proactive symbol injection

**What.** The harness looks up symbols relevant to the current step and injects definitions and diagnostics into the context before calling the model, rather than waiting for the model to ask.

**At the target tier.** Originally load-bearing for a 4B floor ("the model does not know what to search for and gives up after one attempt"). A 27B–35B model can drive its own symbolic search reasonably well, so injection becomes an **optional enhancement to measure**, not a foundation. Tools (D2) come first.

**Footguns.**
- Per-turn injection breaks the prefix cache unless placed at the tail (B3).
- Near-miss context still degrades a model with a limited effective window; precision matters more than recall.
- Never inject from a non-ready server (D3).

**Open questions.** What is the query on turn N — the current todo item, the last test failure, the file just edited?

**Related.** B2, B3, D1, D3, J1.

---

## E. Task control

### E1 · Separate plan from act

**What.** Before execution, the model emits a plan the user can see and edit. Correction moves from after the damage to before it — what keeps long tasks steerable.

**Open questions.** Is the plan a file in the repo, or session-only state? Optional per task, or a mode (F2's read-only mode is close)?

**Related.** E2, F2, H3.

### E2 · The todo list

**What.** The model's externalized decomposition of the task into **independently verifiable outcomes** — not a plan of tool calls. For "add rate limiting to the API":

1. Locate request-handling wiring — `done`
2. Check whether a limiter crate is already a dependency — `done`
3. Implement middleware — `in_progress`
4. Wire into router — `pending`
5. Expose limits in config — `pending`
6. Add tests — `pending`
7. Run test suite, fix failures — `pending`

**Granularity.** Something that could plausibly fail on its own and be verified independently. "Read main.rs" is too fine; "implement the feature" too coarse.

**Mechanics.** Written through a `todo_write` tool so it is structured data, not prose buried in the transcript. That lets it do four jobs:
- **Anchor:** shown every turn, it resists drift.
- **Compaction survivor:** kept verbatim when everything else is summarized.
- **Drift signal:** tool calls that map to no item suggest something has gone wrong.
- **Steering surface:** the user can edit it mid-run — the cheapest course correction there is.

**Properties.** Revised, not fixed (discovery appends items). Statuses: `pending`, `in_progress`, `done`, `blocked`, `abandoned`; roughly one `in_progress` at a time. **Always contains an explicit verification item** — largely how the model is made to run the tests rather than declare victory.

**Hierarchy.** One shallow level at most. If an item needs five subtasks, that is a signal for a subagent with its own flat list (E9), not a deep tree.

**Variant — harness-authored list.** The harness derives the list from the plan (E1) and the model only marks items complete. Proposed for 4B models that drift and emit malformed lists.

**At the target tier.** Model-authored is the likely default for 27B–35B. Keep the harness-authored variant as a fallback to test against J1 if drift shows up.

**Footguns.**
- The list is **data**, not a projection of live state machines (see Z9).
- Showing it every turn costs prefix cache unless it sits at the tail (B3).

**Related.** A9, B3, B5, E1, E3, E9, J1.

### E3 · The verification loop

**What.** After edits, run build / tests / lint and feed failures back to the model.

**Why.** The single biggest quality lever — the difference between a harness that works and one that produces plausible garbage.

**Open questions.** How does the harness know the project's verify commands — config, instruction file (B10), detection from `Cargo.toml` and friends? Are verify commands pre-approved by permission rules (F3)?

**Related.** A9, C5, E2, E5, F3.

### E4 · Budgets

**What.** Enforced on three axes, with the model told what remains:
- Tokens (per step and per session)
- Wall clock
- Iteration count (per turn and per task)

**Why.** With no human present (exec, H6), budgets are the only thing stopping a spiral. Exceeding one must produce a **distinct non-zero exit code** so CI fails loudly.

**Related.** E5, H6.

### E5 · Loop, stall, and spiral detection

**What.**
- **Identical tool call repeated.** On the second repeat, answer with "you already ran this; here is the earlier result" instead of executing again.
- **No file changes** across N steps in a task that is supposed to be editing.
- **Oscillation:** the same file edited back and forth.
- **Bounded reflection:** if the same tool fails the same way three times, stop and ask the user. Unbounded self-correction produces expensive spirals, not solutions.

On detection: interrupt, summarize state, hand control to the user.

**Related.** E3, E4.

### E6 · Structured self-report

**What.** At the end of a task, the model states what it believes it changed and why, giving the user something to review that is not a diff.

**At the target tier.** Doubtful at 4B; plausible at 27B–35B. Worth comparing the self-report against the actual diff as a cheap honesty check.

**Related.** A9, J1.

### E7 · State machine as nested enums and a pure transition function

**What.** Two different things often get conflated:

- **Nested states** — a task is `Executing`, and within that `AwaitingTools`. Fixed shape, known at compile time. **Enums handle this.**
- **A tree of running machines** — sub-tasks spawned at runtime. Only needed with subagents (E9, Z4).

For a single machine: data lives in the variants, nesting is nested enums, and the transition function is **pure** and returns **effects** instead of performing them.

```rust
enum TaskState {
    Planning { draft: Option<Plan> },
    Executing { plan: Plan, cursor: usize, step: StepState },
    Verifying(VerifyState),
    Blocked { reason: Blocker },
    Done(Outcome),
}

enum StepState {
    Thinking,
    AwaitingTools { pending: Vec<CallId> },
    Repairing { attempts: u8 },
}

fn step(state: TaskState, ev: Event) -> (TaskState, Vec<Effect>)
```

The runtime executes effects (`Effect::CallModel`, `Effect::RunTool`, `Effect::AskUser`) and feeds results back as events.

**Why.**
1. Transitions are unit-testable with no mocking or I/O.
2. The state layer stays synchronous — no async in traits.
3. State is a plain serde enum, so snapshot, resume, and rewind are serialization.
4. It is the clearest way to *see* how an agent loop works, which suits the learning goal.

**Footguns.**
- The sketch above will not survive contact with real tasks. Expect to rewrite it.
- Streaming, cancellation, and parallel tool results do not fit "one event in, one state out" neatly. Effects need ids so results can be correlated.

**Related.** A1, E8, G1, G4, Z4, Z6.

### E8 · Cancellation and mid-turn steering

**What.**
- **Ctrl-C cancels the in-flight step, not the session.**
- Cancellation can hit mid-stream, mid-tool, or mid-patch. Decide what lands in the log, and record the cancellation so the next turn knows what happened. This is where event-sourced systems corrupt.
- **Steering mid-turn:** the user types while the model generates. Queue it, or interrupt and re-plan? Expressed as commands (H3), so cheap to decide now and expensive once a UI exists.

**Related.** A2, C3, E7, G1, H3.

### E9 · Subagents

**What.** A subagent runs a separate loop with a fresh context. It can read thirty files and return a two-paragraph summary; the main context only sees the summary. The most effective way to keep a limited window focused, and a good fit for "gather context, then answer."

**Facts.** On llama-server, pin the main loop and subagents to different slots, or every subagent run evicts the main loop's KV cache and forces a long re-prefill (§0.3).

**Footguns.** Running main and subagent generation concurrently shares the GPU; each runs slower.

**Open questions.** Parallel subagents, or strictly sequential? When the tree of machines becomes necessary, see Z4.

**Related.** A5, E2, E7, Z4, §0.3.

---

## F. Safety and permissions

### F1 · Effect classes

**What.** The model is the untrusted party. Tag every tool with an effect class:
- `read_only` — reads, searches, listings
- `workspace_write` — edits inside the repo root
- `destructive` — deletes, force operations, history rewrites
- `network` — anything that egresses
- `exec` — arbitrary command execution

**Facts.** ACP tool calls carry a `kind` (read, edit, delete, move, search, execute, …) that Zed uses for icons and views — a natural mapping (H4).

**Related.** B8, C2, F2, F3.

### F2 · Permission modes and profiles

**What.**
- **Modes:** read-only (plan and explain, never mutate); auto-edit (repo edits automatic, exec and network prompt); full-auto (sandboxed or throwaway environments).
- **Profiles:** named rule sets ("read-only", "edit `src/` only"). ACP session modes (`session/set_mode`) let Zed show a picker for switching profiles.

**Related.** F3, H4.

### F3 · Permission rule engine

**What.** Rules map a tool plus a matcher to `allow`, `ask`, or `deny`. Matchers are path globs for file tools and command prefixes for shell.

```toml
[profiles.dev]
rules = [
  { tool = "read_file", path = "**",          action = "allow" },
  { tool = "edit",      path = "src/**",      action = "ask" },
  { tool = "edit",      path = ".env*",       action = "deny" },
  { tool = "bash",      prefix = "cargo test", action = "allow" },
]
```

**Layers:** user config (`~/.config/axon/config.toml`), project config (`.axon/config.toml`), session rules created by "always allow". **Deny wins at every layer**, so a project file cannot override personal denies.

**Start simple.** First version: every tool call goes through one `fn check(&ToolCall) -> Decision` that always returns `Allow`. Adding real policy later changes one function, not every tool.

**Facts.** `globset` for matching; `toml` + `serde` for config. In ACP, the policy is ours and Zed supplies the prompt UI (H4).

**Related.** B10, F1, F2, F4–F7.

### F4 · Path jail

**What.** Writes confined to the repo root by default, with an explicit escape list. **Canonicalize paths and resolve symlinks before checking**, or `../` and symlinks escape and the jail is decorative.

**Footguns.** Time-of-check/time-of-use: a symlink swapped between check and write. Open-then-verify or operate on file descriptors if this matters.

**Related.** F3, F5.

### F5 · Shell safety and sandboxing

**What.** Do not try to parse shell to decide whether it is safe; that is not winnable. Layer instead:

1. Allowlist / denylist on the leading command — ergonomics only.
2. Any command containing `&&`, `|`, `;`, or `$(...)` is `ask`; prefix matching cannot judge compound commands.
3. Prompt on anything not allowlisted.
4. **Real sandboxing as the actual boundary:** namespaces + seccomp on Linux, `sandbox-exec` on macOS.

**Footguns.**
- `sandbox-exec` is deprecated on macOS, though Codex and Claude Code still use it.
- CI usually already runs inside a container, so sandboxing needs a trust-the-environment bypass or nested sandboxing fails (H6).

**Related.** F3, F4, H6.

### F6 · Approval memory

**What.** Remember approvals per session and per pattern so the user is not prompted forty times for the same thing. Offer: allow once / allow for session / always allow / deny. "Always allow" writes a rule (F3).

**Related.** F3, H4.

### F7 · Hooks as the general mechanism

**What.** Hooks run user code at lifecycle points: before a prompt is sent, before and after a tool runs, when a turn ends. **The permission engine is really a pre-tool hook**, so build hooks as the general mechanism and implement permissions on top.

**Why.** One mechanism serves permissions, formatting-after-edit, auto-verification (E3), prompt preprocessing (A10), and logging.

**Footguns.** Hooks require the core to call back into the consumer mid-turn — the boundary must be bidirectional from day one (H2). Project-supplied hooks are untrusted code (B10).

**Related.** A10, B10, E3, F3, H2, I4.

---

## G. Persistence and sessions

### G1 · The event log as the single source of truth

**What.** Exactly one artifact is authoritative: an append-only event log per session. Everything else — snapshots, indexes, UI state — is derived and can be deleted and rebuilt by replay.

**Why.** It shrinks the persistence problem from "evolve every state type forever" to "keep the event enum readable forever". Events are far more stable than states. This is the persistence counterpart of B1.

**Facts & prior art.**
- **Claude Code** stores each session as JSONL under `~/.claude/projects/<project>/<session-id>.jsonl`, one object per message, tool use, or metadata entry. No migration story by design: internal format, subject to change, transcripts auto-purged after 30 days by default. Bounding the data's lifetime makes evolution nearly a non-problem.
- **Codex CLI** splits by artifact: rollout JSONL under `~/.codex/sessions/YYYY/MM/DD/`, metadata and memories in `~/.codex/state_4.sqlite`, a `session_index.jsonl` cache. The schema version lives in the filename (`_4`); migration is "bump and start fresh."
- Nobody uses protobuf, Avro, or Cap'n Proto for this. The working answer is JSON lines plus aggressive scoping of what must survive.

**Footguns.** The burden moves rather than vanishing: old events must stay interpretable forever (G7).

**Related.** B1, B9, G2–G7.

### G2 · Split storage by artifact

**What.** Categorize data as **precious** or **regenerable** and give each the cheapest strategy that works.

```
.axon/
  sessions/
    <id>/
      events.jsonl          ← source of truth, append-only
      state.v7.msgpack      ← internal-state snapshot: derived, deletable
      public.v3.msgpack     ← projected-public snapshot: derived, deletable
      meta.json             ← id, title, schema versions, binary version, last-good offset
  state_2.sqlite            ← session index, todo projections, queryable UI state
  repomap/
    manifest.json           ← commit SHA, adapter versions
    outline.json            ← regenerable in seconds
```

- **Event log:** append-only; evolution per record (G7).
- **Snapshots:** versioned filenames; on mismatch, delete and replay (G3).
- **Session index:** SQLite with `user_version` and ordered migrations, or a version suffix and a rebuild.
- **Repo map / LSP caches:** keyed by commit SHA and adapter versions; mismatch means regenerate.

**Footguns.** More artifacts means more tooling — a dump/inspect path per format. Start with only the log until a derived artifact is actually needed.

**Related.** D5, G1, G3, G6, G8.

### G3 · Disposable snapshots; never truncate the log

**What.**
- **Internal-state snapshot** — machine state at a given log offset; shortens replay on resume.
- **Projected-public snapshot** — the public view (H8) at an offset; lets a late-joining frontend receive current state plus live tail instead of replaying everything.
- **Never truncate the log behind a snapshot.** The moment the log is pruned, the snapshot becomes precious and the whole migration burden returns. It also destroys the ability to re-derive after a projection bugfix, which will be needed.
- **Prune content, not structure.** Size problems come from raw tool output and repeated compaction history, not event count. Replace large payloads with a truncation marker or a content-addressed reference, keeping the event skeleton intact.

**Footguns.** Replay cost grows with session length — that is the only reason snapshots exist. Do not build them until resume is measurably slow.

**Related.** B9, G1, G2, G9, H8.

### G4 · Deterministic replay

**What.**
- **Record every effect's result as an event** — each model response and each tool result. Replay reads them back; it never calls the model or re-runs tools. Without this, "state is a fold over events" is false, because model output is not reproducible.
- **Replay after a bugfix changes history's meaning.** Fixing a bug in the transition function changes what every old log folds into. Good for debugging; surprising for "resume my session". Record the binary version in `meta.json` and warn on cross-version replay.
- Parallel tool results must be logged in a defined order (C3).

**Related.** C3, E7, G1, J2.

### G5 · Checkpoints and rewind

**What.** Snapshot the working tree per step so the user can rewind to before the model went sideways.

**Mechanism: a shadow index.** Set `GIT_INDEX_FILE` to a path under `.axon/`, then `git add -A`, `git write-tree`, `git commit-tree`. This produces real, addressable commits without touching HEAD, the user's index, or the working tree. `git diff <snapshot-sha>` works for inspection. (`git stash` and scratch branches were rejected; see Z5.)

**Rewind in an append-only log.** Append a `RewoundTo(n)` event and have the fold honour it. The naive implementation truncates the log, which breaks G1.

**State the honest scope.** Rewind covers the conversation and tracked files. It does **not** cover untracked or ignored files the agent created, submodules, non-git repositories, or any side effect of a shell command — a dropped table, a pushed branch, a sent request. Name the feature so it does not imply the last category.

**Open questions.** Does rewinding mint a new session id or keep the old one? Scripts hold ids (H6).

**Related.** G1, G8, H6, Z5.

### G6 · Crash consistency

**What.**
- **Atomic writes** for anything not append-only (write to temp, fsync, rename), so a crash never leaves a truncated file that fails on next load.
- **Across artifacts:** events flushed to 500 while the index committed at 480. Keep a last-good offset in `meta.json` and truncate-on-recovery, or derived state is silently wrong.
- A torn final line in `events.jsonl` must be detected and dropped, not fail the whole session.

**Related.** G1, G2.

### G7 · Event schema evolution

**What.** Since the log is authoritative and permanent, the `Event` type carries the evolution discipline. Requirements gathered so far:

1. **Sum-type evolution:** add a variant, change a payload, rename a variant, without corrupting old data. Most formats handle records well and unions badly.
2. **Field evolution:** add with defaults, drop, rename.
3. **Explicit version tagging**, readable without deserializing the body.
4. **Backward compatibility** (new binary, old data) — mandatory.
5. **Forward compatibility** (old binary, new data) — only needs to detect and refuse cleanly.
6. **A failure policy per artifact.** Machine state fails loudly rather than loading lossily; transcripts may degrade gracefully.
7. **Chainable migrations** for semantic changes; eager or lazy.
8. **Unknown-field/variant preservation** — only if mixed-version access is supported. If not, drop the requirement.
9. **Schema derived from the Rust types**, not a separate IDL.
10. **Human inspectability.**
11. **Testable compatibility:** a golden corpus of old files replayed in CI.
12. **Size and write cost.**
13. **Atomicity** (G6).

**Encoding notes.**

| Option | Read |
|---|---|
| serde + JSON | No IDL, maximally debuggable, `default`/`alias` cover most cases. Large; mitigate with zstd. |
| serde + MessagePack / CBOR | Same evolution properties as JSON, smaller and faster, less readable. |
| serde + bincode / postcard | Field order is the schema. Fine only for regenerable caches. |
| Protobuf | `oneof` is not a real sum type; proto3 makes everything optional, losing exhaustiveness. |
| Cap'n Proto / FlatBuffers | Better unions; heavy machinery unless state gets huge. |
| Avro | Writer's schema travels with data; unions first-class. Thin Rust ecosystem. |
| SQLite | A migration idiom (`user_version`), not an encoding. Composes with the above. |
| `savefile` | Schema embedded in data, verified on load. Separate from serde. See Z3. |
| `versionize` | Firecracker's per-field version ranges. bincode-only, experimental outside Firecracker. |

- `#[serde(other)]` only works on **unit** variants; an unknown variant carrying a payload needs a hand-written `Deserialize`.
- `rmp_serde::to_vec` encodes structs as **positional arrays**. Use `to_vec_named` / `with_struct_map()` or field order becomes the schema.
- MessagePack barely beats JSON on string-heavy transcripts; zstd-compressed JSON lands close.
- **The encoding is largely orthogonal to the migration problem** — the weaknesses are serde's. Don't spend design effort choosing it. JSONL is the default.

**A contradiction to avoid.** A catch-all `Unknown` variant on every persisted enum conflicts with requirement 6 (machine state fails loudly). Since snapshots are disposable (G3), the catch-all discipline is only needed on the **event** enum. See Z7 for the heavier enforcement scheme that was proposed.

**Enforcement without review:** a property test that round-trips every persisted type, plus the golden corpus replayed on every build.

**Open questions.** Is mixed-version access (two binaries, one directory) supported at all? "No" deletes requirement 8.

**Related.** G1, G3, G6, Z3, Z7.

### G8 · Session identity and concurrency

**What.** Exec prints its session id (H6), which makes the id a public contract.

- **Format:** UUIDv7 or ULID for free chronological sort. Git-style prefix matching (`--resume a3f2`) via the session index. Not guessable — it becomes a bearer token if anything remote ever exists.
- **Two processes, one session:** exec resumes a session the TUI has open. Advisory lock per session, with defined behaviour: refuse, or attach read-only.
- **Staleness:** a session captured at commit X and resumed three days later replays perfectly against a repo that no longer matches. Record commit SHA and dirty state; warn on divergence.
- **Retention with tombstones,** so an expired session reports "expired" rather than "not found" (G9).

**Related.** G2, G5, G9, H6.

### G9 · Retention

**What.** A retention policy is part of the design, not an afterthought.

**Facts.** A reported case from the ecosystem: a 732 MB session file, 184 files over 100 MB, and a ~91 GB sessions directory, dominated by repeated compaction history and raw tool output. Claude Code purges after 30 days by default.

**Open questions.** Default retention period? Content pruning (G3) only, or whole-session expiry too?

**Related.** B5, G3, G8.

---

## H. Frontends and integration

### H1 · The core/frontend boundary: options

**What.** Where the seam sits between the core and whatever drives it. Largely irreversible in practice, because internal types leak into whatever sits above.

| | Shape | Pros | Cons |
|---|---|---|---|
| A | Monolith: TUI links the core and calls functions | Simplest; zero serialization; one binary | Rust-only; retrofitting an API is painful because types leak |
| B | Library core + in-process command/event API | Monolith speed, but the boundary exists while cheap to move; core testable without UI | Rust-only until a transport is added (then mechanical) |
| C | Subprocess + JSON-RPC over stdio | Polyglot clients nearly free; no ports, auth, or daemon lifecycle | A public protocol is a permanent versioning commitment; one client per process |
| D | Long-lived daemon + socket | Many frontends attach/detach; survives frontend crash | Lifecycle, stale sockets, local auth, version skew |
| E | HTTP/gRPC server | Web UI trivial | A local HTTP server that can run shell is a serious attack surface |
| F | Relay: local process connects outbound to a hosted relay | Works through NAT and locked-down networks | Requires hosted infrastructure — a second product |

**Facts & prior art.**
- **Codex** started at A and added an app-server: a child process speaking JSON-RPC 2.0 over stdio, with clients in Go, Python, TypeScript, Swift, Kotlin. A message processor turns core's internal events into stable UI-ready notifications. Bindings generated from Rust protocol types, version-pinned to the binary.
- **Claude Code's SDK** spawns the CLI as a subprocess over stdio, bundling a native binary pinned to the package version. One session per subprocess.
- **The divergence:** Codex publishes the wire protocol; Claude Code publishes the SDK surface and keeps the wire internal. Protocol surface area versus freedom to refactor.
- **Claude Code Remote Control** is the F case: outbound HTTPS only, polls for work, server routes to web/mobile clients. Going remote forced commands to be reshaped so arguments are passed inline rather than through interactive pickers.

**Related.** H2–H8, Z8.

### H2 · Library core with a command/event API

**What.** Option B. The core exposes `submit(Command)` and an event stream (`tokio::sync::mpsc`); each frontend reads the stream and renders it its own way. Design command and event types as though they were already a wire protocol.

```
core (lib crate)
 ├─ agent loop, model client, tools, permission policy
 ├─ emits events: TextChunk, ToolCall, Diff, ToolResult, Done
 └─ depends on traits:
      Approver   → decides when policy says "ask"
      FileSystem → read/write files

frontends (bin crates)
 ├─ acp:  events → session/update; Approver → session/request_permission; FileSystem → fs/* in Zed
 ├─ tui:  events → ratatui; Approver → y/n prompt with diff; FileSystem → disk
 └─ cli:  headless, for scripts and tests
```

**Must include** (expensive to retrofit):
- All events serializable: no `dyn`, no borrowed data, no handles to core objects.
- All user actions as serializable commands (H3).
- **Bidirectional from day one.** Approvals and hooks need the core to call back into the frontend mid-turn.
- Frontends depend only on a command/event crate, never on the core crate — enforced by the dependency graph, not discipline.

**Must not include yet:** a separate internal-vs-public event vocabulary, codegen, JSON Schema annotations, version negotiation, handshakes.

**Related.** F7, H1, H3, H8.

### H3 · Every user action is a serializable command

**What.** No user action may require interactive UI state to express. Every action is a command with its arguments inline — `SetModel { name }`, never "open the model picker." The UI may render a picker, but then it emits the command; it never holds state the core does not have.

**Why.** Costs nothing now and keeps every frontend option (ACP, TUI, exec, remote) reachable. The remote case is a lie detector for logic that leaked into a frontend.

**Related.** E8, H1, H2.

### H4 · ACP: running inside Zed

**What.** Implement the agent side of the Agent Client Protocol so Axon runs in Zed's Agent Panel. Zed provides chat, file mentions, and diff review for free, so the harness is usable daily long before a TUI exists.

**Facts.**
- Zed launches the agent as a subprocess; JSON-RPC over stdin/stdout. The roles are the reverse of MCP: **the editor is the client and the agent is the server it drives** — more like LSP than MCP. The harness can serve ACP to Zed and be an MCP client to tool servers at the same time.
- Use the `agent-client-protocol` crate (official Rust SDK, alongside Python, TypeScript, and Kotlin). Implement the agent side — initialize, new session, prompt, cancel — and call the client side for updates and permission requests.
- **Diffs:** a tool call's content can be a diff block (`path`, `oldText`, `newText`) that Zed renders inline. Tool calls carry a `kind` used for icons and views.
- **Accept/reject:** `session/request_permission` with options ("Allow once", "Always allow", "Reject") blocks until the user picks. An edit: compute new contents → send the tool call with the diff → request permission → write only if approved.
- **File I/O:** `fs/read_text_file` and `fs/write_text_file` route through Zed's open buffers, including unsaved changes (C6).
- **Modes:** `session/set_mode` maps onto permission profiles (F2).

**Footguns.**
- **stdout is the protocol channel.** All logs go to stderr or a file, or the JSON-RPC stream is corrupted. Nearly everyone hits this.
- **ACP is an Option C shape.** Zed is a second frontend that genuinely drives the session, which was the stated trigger for moving to C. The usual cost of C — owning and versioning a public protocol — mostly does not apply, because ACP is someone else's maintained protocol. The core still has to be behind a clean boundary (H2) for ACP and the TUI to coexist.

**Related.** C6, F1, F2, F6, H2, H5, H7.

### H5 · The TUI

**What.** A standalone terminal frontend on the same core: `ratatui` rendering of the event stream, approval prompts with a diff view, direct disk access, streaming output, interrupt handling (E8), and rendering of the todo list (E2).

**Open questions.** ACP first and TUI later, or the reverse? ACP gives a UI for free; building the TUI teaches the frontend boundary first-hand.

**Related.** E2, E8, H2, H4.

### H6 · Exec (headless) mode

**What.** Non-interactive: a task in at invocation, an event stream out, an exit code at the end. **Single inbound message per invocation** — not single turn; a CI task like "fix the failing test" runs many model turns, but nothing new comes in after start.

- **Approvals** bind to a **policy function** instead of a prompt — deny-all, allowlist, full-auto-in-container. No second permission model.
- `ask_user` has nobody to ask: exit with a distinct code plus the session id.
- **Multi-turn exec comes from resume,** not a protocol: `exec --resume <id> "answer"`. So exec **prints its session id on exit**.
- **Budgets become load-bearing** (E4), with distinct exit codes.
- Sandboxing needs a trust-the-environment bypass inside CI containers (F5).
- Also serves shell piping (`cat errors.log | axon -p "explain this"`), git hooks, and build-system invocation.

**Facts.** A headless CLI is the easiest frontend to debug and doubles as the test harness for replaying recorded model responses (J2).

**Related.** A9, E4, F5, G8, J2.

### H7 · Editor bridge as an MCP client

**What.** An alternative editor integration to H4. When Claude Code's editor extension is active, the extension runs a local MCP server and the terminal CLI connects to it as a client. The editor sends focused file, cursor, and selection with each message; the agent sends diffs to the editor's native diff viewer. The editor is a **tool provider and display sink**; the terminal session remains the sole owner.

**Contract needed from the editor:** open a diff for review, return the current selection, return diagnostics, return the focused file. There is no standard "IDE MCP server", so this contract would be ours, with support enumerated per editor.

**Degrades gracefully:** with no editor connected, diffs render in the TUI and enrichment is absent.

**Relationship to H4.** ACP puts the agent inside the editor's chat panel; this keeps the agent in a terminal and uses the editor as a viewer. For Zed specifically, ACP is the native route. This bridge matters for a TUI-first workflow or for editors without ACP.

**Related.** H4, H5, I1, I2.

### H8 · Public events are a pure projection

**What.** If internal and public event vocabularies ever split (only when a frontend outside the codebase consumes events), **persist internal, derive public**, and make the projection a pure function of the internal stream. Otherwise UI state cannot be reconstructed on resume or reattach.

**Related.** G3, H2, Z10.

---

## I. Extensibility and memory

### I1 · Harness or MCP? Two litmus tests

**What.** For any proposed feature, decide whether it is built into the harness or provided by an MCP server.

- **Test 1 — Is it on the context-assembly critical path?** Anything that must run *before* the model is called cannot be MCP without the harness hard-coding a specific server. *Qualifier:* optional enrichment from a local source that degrades gracefully when absent is exempt (editor selection via H7). The test is about whether the feature is load-bearing.
- **Test 2 — Does it participate in checkpointing?** An MCP server cannot roll back. If rewind (G5) has to undo what it wrote, it belongs inside.
- **Cost rule underneath both:** every MCP call costs a model turn. Anything needed every turn cannot be one.

Fails either test → built in. Passes both, and the user plausibly already owns a system for it → MCP.

**Worked examples.** File read/edit, shell, git: fail test 2 → built in. Code intelligence: built in (D1). Issue trackers, calendars, cloud services: MCP. Web search: a coin flip (I7).

**Related.** D1, H7, I2, I5, I6, I7.

### I2 · MCP client support

**What.** Connect to user-configured MCP servers via `rmcp`; expose their tools through the registry (C2) with an effect class and permission rules (F3).

**Footguns.** MCP responses are untrusted data (B8). Each server's tools add to the tool-count budget (B7).

**Related.** B7, B8, C2, F3, I1.

### I3 · Skills with progressive disclosure

**What.** Only skill names and one-line descriptions sit in the system prompt. The model loads a skill's full instructions with a tool call when relevant; scripts and reference files load later still. Hundreds of skills at almost no context cost.

**Footguns.** The skill index is part of the stable prefix (B3); changing the installed set invalidates the cache.

**Related.** B2, B3, I4.

### I4 · Plugins as a packaging format

**What.** A plugin is a bundle of skills, slash commands, hooks, and MCP server configs. Once those pieces exist, plugins are mostly a loader. Not a code-plugin system (see Z2).

**Related.** A10, F7, I2, I3, Z2.

### I5 · Project memory

**What.** Durable facts about a project that persist across sessions.

- **Simplest form:** writable memory is a tool that appends to an instruction file (B10). It needs curating or it bloats.
- **Policy in the harness, store pluggable.** Decay needs signals only the harness has: was this memory in context when the task succeeded? Did the user correct it? Was it retrieved and then ignored? An MCP server only sees retrieval calls, so its decay degenerates to TTL. The backing store (files, SQLite, Obsidian, a team database via MCP) sits behind a trait. What to write, when to inject, how to score, decay, and invalidate stays in the harness.
- **Invalidation outranks decay.** A merely old fact is noise; a fact that is now *wrong* ("this project uses Postgres") is actively harmful. Detecting contradiction means comparing stored beliefs with current repo state — harness-side by definition. Design invalidation first.

**Facts & prior art.** Codex CLI's memory pipeline — extraction agents over rollout files, consolidation, usage-aware retrieval, diff-based forgetting — is built into the harness.

**Open questions.**
- What does memory retrieve against? Memories are few, short, and prose, so keyword matching over a small set may be enough. This is the strongest remaining argument for embeddings (Z1).
- Decay signal weights, and where tuning data comes from before there is much usage.

**Related.** B10, I1, Z1.

### I6 · Backlog adapter

**What.** Separate the **working todo list** (E2: ephemeral, model-authored, read every turn, rolls back with checkpoints → built in) from the **project backlog** (durable, human-authored, hierarchical, read occasionally → MCP). The backlog lives in Jira, Linear, or GitHub Issues; a thin adapter pulls an item into a session as the task specification and pushes status back.

**Open questions.** Automatic write-back, or only on explicit user action? Writing automatically to a shared tracker is a surprising side effect.

**Related.** E2, I1, I2.

### I7 · Web search

**What.** Passes both litmus tests, so it could be MCP or a built-in tool — decided by whether it is core enough to guarantee it is present.

**Related.** I1, I2.

---

## J. Quality and evaluation

### J1 · Evaluation set

**What.** Ten to fifteen tasks with scripted pass/fail on a fixture repository, runnable against any endpoint.

**Why.** The instrument everything else depends on. It turns these questions from arguments into measurements:
- Does the verification loop (E3) actually help?
- Model-authored or harness-authored todo list (E2)?
- Which edit format (C5)?
- How much does tool count hurt (B7)?
- Is proactive injection (D6) worth its cache cost?
- Is the residual retrieval gap large enough to bring embeddings back (Z1)?
- Did a system-prompt change (B6) help or hurt?

Without it, every context-assembler change is a vibe.

**Related.** A6, B6, B7, C5, D6, E2, E3, J2.

### J2 · Record and replay

**What.** Capture real model responses once, replay them as fixtures. Nearly free given the event log (G1) and deterministic transitions (E7, G4). The headless CLI (H6) is the natural runner.

**Why.** The difference between an agent that can be regression-tested and one that can only be tried. Also the right way to test provider adapters (A3).

**Related.** A3, E7, G4, H6, J1.

### J3 · A written scope fence per milestone

**What.** For each milestone, a short list of what is in and what is explicitly out, kept in the repo. Otherwise scope lives implicitly in a document like this one, and every idea looks in scope.

**Related.** J4.

### J4 · A first-milestone path

**What.** One suggested ordering that gets a usable tool quickly:

1. Streaming chat REPL against a local endpoint (A1, A2).
2. Read-only tools and the tool loop (C1, C3, C4, B4) — this alone covers "ask questions about code."
3. ACP, to use it daily in Zed (H4).
4. Edits with diff approval (C5, C6, F3).
5. Shell, saved sessions, context management (F5, G1, B5).

**Related.** J3, and everything above.

---

## Z. Parked

Set aside on purpose. Each entry records why and what would bring it back.

### Z1 · Semantic index (embeddings)

**What it was.** Chunking, embedding, ANN search, git hooks to re-index, an on-disk vector store keyed by a config hash, and three modes (full auto-injection / tool only / off) with a dynamically registered `semantic_search` tool.

**Why parked.**
1. **Compute and memory contention.** The generating model is local; an embedding model competes for the same GPU and unified memory, and re-embedding on save steals inference time. Prior art mostly assumes cloud models and omits this cost.
2. **Precision over recall.** Embedding similarity is recall-oriented. A model with a limited effective window is degraded by near-miss chunks. *At the target tier* this argument is weaker than it was at 4B, but still real because effective context is well below nominal.
3. **Code has exact retrieval.** Definitions, callers, and types have precise answers via language servers (D1).
4. **The query was never specified.** On turn N, what is the query — the last message, the current todo item, the last test failure? The modes were designed before the input was.

**What brings it back.** Measured evidence against J1 that "how does this codebase do X" and turn-1 orientation are frequent failures the repo map (D5) does not cover. It would return as one more ranked source, not a redesign. Project memory retrieval (I5) is the strongest standing argument for it.

### Z2 · A plugin system for language support

**What it was.** Installable plugins translating between a harness-defined code-intelligence structure and a specific language server.

**Why parked.** LSP is already the language-agnostic layer, so the translation is close to an identity function; what varies is configuration and quirks (D4). Dynamic-library plugins bring an unstable Rust ABI and crash the host; subprocess plugins over JSON-RPC commit to a public protocol before any consumer exists.

**What brings it back.** A second, genuinely different plugin kind. One instance is not a pattern.

### Z3 · The `savefile` spike

**What it was.** A two-day, hard-stop spike to test whether the `savefile` crate (schema embedded in data, verified on load) handles harness state well enough to avoid building a serde-based evolution layer.

**Why parked.** It tested schema evolution of a state type that did not exist yet, and G1/G3 make state snapshots disposable, removing most of the need.

**What brings it back.** Building snapshots (G3) for real, with real types. If revived, the specification was:

- **Adopt if all hold:** the model is expressible without contorting domain types; adding a variant to a nested enum is backward compatible with one annotation or less; newer-schema data fails with a clear catchable error; size and speed within budget; third-party types supported in under half a day.
- **Reject if any hold:** any scenario silently corrupts; nested sum types must be flattened; it cannot coexist with the enforcement layer.
- **Kill at the timebox.** Inconclusive means default to serde.
- **Representative model:** 8–12 types; a three-level nested enum with unit, tuple, and struct variants; a heterogeneous id-keyed map; `Vec`/`HashMap`/`Option`/`Box`; newtype ids; a timestamp and UUID; an opaque JSON payload; a large string. 50–200 nodes, 10–20 todo items.
- **Scenarios (write vN, read vN+1 unless noted):** add field with default; remove field; rename field; widen `u32`→`u64`; split a field; **add a variant to a nested enum** (new reads old, and old reads new — must fail cleanly); remove a variant present in old data; change a variant payload; **reorder variants** (is position load-bearing?); rename a type; **skip a version** (v1 → v3); truncated file; corrupted bytes.
- **Measurements:** size with and without embedded schema, compressed and not; serialize, cold deserialize, and deserialize-with-migration time; compile time and dependency count. Budget: under ~10 ms per snapshot is fine, over ~100 ms is not.
- **Enforcement checks:** native unknown-variant handling? Coexistence with a project derive? Can save/load be wrapped in a sealed trait? Do version annotations propagate through nesting? **Readability after five accumulated versions** — likely the decisive test.
- **Operational risk:** maintainers, release cadence, open versioning bugs, dependents, transitive `unsafe`, cost of migrating off it if abandoned.
- **Control arm:** the same model and scenarios on serde + MessagePack with named struct encoding, about a third of the timebox.

### Z4 · A supervisor tree of state machines

**What it was.** A supervisor owning `HashMap<MachineId, Node>`, where `Node` is a tagged enum over machine kinds plus parent/child ids. Children report upward only through outcome events (`Running | Awaiting(Need) | Done(V) | Failed(E)`), never by mutating the parent, so every machine is testable and the tree serializes in one shot.

**Why parked.** Only subagents need it, and with the todo list decoupled from machines (Z9) nothing else does. One machine first (E7).

**What brings it back.** Adopting subagents (E9) that need to be resumable and inspectable as a tree.

### Z5 · Checkpoints via `git stash` or scratch branches

**Why parked.** `git stash` mutates the index and working tree and can conflict; a scratch branch needs a checkout. Both disturb a user with uncommitted work — the normal case. Replaced by the shadow index (G5).

### Z6 · Typestate, `Box<dyn Machine>`, and `statig`

**Why parked.**
- **Typestate** (each state a distinct type): heterogeneous machines cannot share a collection or deserialize into an unknown variant, which kills snapshots.
- **`Box<dyn Machine>`:** heterogeneity without serialization. If a uniform interface is wanted, keep state as a serializable enum and dispatch behaviour on a kind tag.
- **`statig`:** a real hierarchical-state-machine crate with proper superstate entry/exit, but aimed at `no_std`, static, allocation-free machines — the opposite of a runtime tree that survives restarts.

**What brings it back.** `statig` only: a single static, deeply nested machine turning up somewhere.

### Z7 · A `Persist` derive enforcing catch-all variants everywhere

**What it was.** Three layers, airtight together: (a) a derive macro that emits `compile_error!` unless one variant is marked `#[persist(unknown)]`, and generates the deserializer including the unknown arm so `serde::Deserialize` is never derived directly; (b) a sealed `Persist` trait that only the macro can implement, with save/load generic over it; (c) recursive bounds so a non-conforming enum nested three levels down fails to compile.

**Why parked.** A proc-macro crate to maintain, and a catch-all on machine state contradicts "fail loudly" (G7). Disposable snapshots mean only the event enum needs the discipline, which a property test and golden corpus cover.

**What brings it back.** Many persisted enums that must survive indefinitely, with evolution bugs showing up that tests miss. The sealed-trait-plus-recursive-bounds pattern is worth knowing regardless.

### Z8 · Daemon, HTTP server, and relay frontends (Options D, E, F)

**Why parked.** D only if attach/detach becomes a real need. E only with a hosted story; a local HTTP server that runs shell is an attack surface. F is hosted infrastructure — a second product. See H1.

### Z9 · The todo list as a projection of the machine tree

**What it was.** "Item 3 at `in_progress` *is* a child machine in `Executing`," so the UI would come for free.

**Why parked.** The list is revised constantly; binding it to live machines turns every revision into an identity problem (renamed item 3, or replaced it?). All four of the list's jobs work on plain data.

### Z10 · A separate public event vocabulary now

**Why parked.** Maintaining a translation layer for a single in-process consumer is a translation for nobody. ACP (H4) speaks its own protocol through an adapter, which does not require a public Axon vocabulary.

**What brings it back.** A frontend outside this codebase consuming Axon's own events, or an SDK in another language (H8).

---

## Changelog

### 0.1.0 — 2026-10-27
- Initial catalogue, consolidating the earlier architecture notes (v0.8.0), their review, the ACP and core-loop design conversation, and the `savefile` spike specification.
- Target re-set from a 4B–8B floor to Qwen3.6-35B-A3B / Qwen3.8-27B on an M2 Max; tier-dependent ideas (D6, E2, E6, B7, Z1) re-evaluated rather than dropped.
- Added from analysis: injection versus prefix caching (B3, D6), record-every-effect for deterministic replay (G4), plain-text stop as a completion fallback (A9), the catch-all-variant contradiction (G7), ACP as an Option C shape with an external protocol (H4), `sandbox-exec` deprecation and path-jail TOCTOU (F4, F5).
