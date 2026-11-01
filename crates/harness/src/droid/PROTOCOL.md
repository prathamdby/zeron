# Factory Droid native protocol

Tested on macOS with Droid **1.1.0**, 2026-11-01. No SDK runtime dependency.
Launch `droid exec --input-format stream-jsonrpc --output-format stream-jsonrpc`.
Both auto-update environment switches are disabled at spawn. Credentials are
owned by Droid; Zeron never invokes authentication RPCs or `droid update`.

## Contract

Frames are newline-delimited JSON. Every outgoing frame includes
`jsonrpc:"2.0"`, `factoryApiVersion:"1.0.0"` and `type` (`request`,
`notification`, or `response`). IDs are strings, including `"1"`. Incoming
frames in these captures also carried `jsonrpc:"2.0"`; the shared reader accepts
its omission. Numeric IDs or missing envelope fields receive `-32700`, with
`id:null`. Plain Codex/ACP framing is unchanged.

`list_models {}` is sessionless. IDs are preserved exactly, including `auto`
and fast variants. Disabled rows are excluded; only minimal/low/medium/high/
xhigh/max become Zeron reasoning levels. No live ID ended in `[1m]` or `-1m`,
or equalled `default`, so client normalization needs no exception.
The normal catalog cache hashes the executable, Factory/Droid environment,
and only `.factory/{settings.json,settings.local.json,auth-deployment.json}`
under `FACTORY_HOME_OVERRIDE` or the home directory. Auth and organization
cache rewrites must not invalidate it.

Initialization and load must finish before a prompt is submitted. Always set
`autonomyLevel` (low/medium/high for Supervised/Auto-accept edits/Full access),
`interactionMode` (spec/auto), and `toolExecutionMode:"direct_only"`.
`auto_approve` means Full access for tool permissions, not plan approval.
Explicit models and efforts apply equally to the spec model.
Omitted/default selections use the effective normal model and
effort, then mirror those to spec settings before verification.

Settings can be coerced or organization-capped despite a successful RPC.
Check effective model, effort, spec model/effort, autonomy, mode and tool mode,
including `availableAutonomyLevels`, before prompting. Updates produce an
effective `settings_updated` with **`requestId` equal to the update RPC ID**,
observed before the empty acknowledgement. This is the verification barrier, not an
uncorrelated earlier notification. The response's `availableModels` supplies
supported efforts for errors. Configured steers wait for turn completion.
Each later idle prompt reapplies the requested interaction mode.

Ordinary tool permissions use `proceed_once` and `cancel`. Every batched action
must fit the run's access mode; otherwise one shared approval question decides
the whole batch. A batch containing `exit_spec_mode` always asks for plan
approval, including with Full access or legacy `auto_approve`. Only after an
explicit Yes, reply with the offered `proceed_auto_run_low`, `_medium`, or
`_high` matching the run's autonomy. Generic `proceed_once` exits spec mode
with autonomy Off; level-specific outcomes preserve it during implementation.
If the matching outcome is not offered, cancel visibly rather than silently
downgrade or escalate. No, missing answers, and stop still send `cancel`.
Unknown client methods, including `pick_options`, receive `-32601`.
Questions are cancellable;
stopping closes each pending input with empty answers.

`tool_call` is an argument upsert, not a new invocation. Emit one tool chip at
the first queued phase, permission request, or result. Results are capped to
16 KiB; inline edit diffs to 64 KiB. Native `ExitSpecMode` calls wait for
permission details before emission, so `details.plan` wins over preliminary
arguments. Only the proposal's title/body survive input sanitization, at most
64 KiB serialized; malformed/oversized proposals cancel visibly, not truncate.
The desktop renders this content as Markdown outside compact work folds;
the small approval question remains separate from the tool-access policy.
MCP names use `server___tool`.
Final `create_message` text is never replayed. Steering is receipted only by
the matching user `create_message.requestId`; discarded input is not receipted.

Only `agent_turn_completed.reason` determines terminal status: completed,
cancelled, authentication failure, or an error with the last error message.
EOF during a turn is an error, not success. A stop interrupts the turn but
keeps the process reusable; teardown closes stdin and reaps the owned process
group, escalating against stubborn children. The prompt end is authoritative.
Autonomous coverage remains conservative (`deterministic_turn_end = false`).

Load reattaches MCP and ignores `session.messages`. `-32004` starts a fresh
session with a visible “without the previous context” notice. No other load
error silently discards context.

## Phase 0 answers and captured sequences

Disposable probe captures are local diagnostics, not test dependencies.
The deterministic peer in `tests/fixtures/droid_rpc.rs` enforces the envelope
and replays the captured shapes below. No credential file was read for content
or copied.

| Question | Answer and frame evidence |
| --- | --- |
| Q1, autonomous turns | **Yes.** `background.jsonl`: initial `agent_turn_completed(completed)` → `child_session_settled(completed)` → assistant deltas → `agent_turn_completed(completed, turnId:"task-completion:…")`, with no intervening prompt. Loop/cron were not exercised, so deterministic completion for every autonomous shape remains unproven. Background task counts keep runtimes alive. |
| Q2, mid-turn settings | **Immediate, not safely deferred.** `primary.jsonl`: pending Execute permission → `update_session_settings("5", autonomyLevel:"low")` → uncorrelated effective settings → `settings_updated(requestId:"5", autonomyLevel:"low")` → response `"5":{}` → turn completion. Do not update settings during a running turn. |
| Q3, MCP init/load | **Both work; no settings-file fallback needed.** `primary.jsonl` and `resume-retry.jsonl`: initialize/load with `mcpServers:[{name,command,args,env:{}}]` → ToolSearch → `tool_call(name:"zeron___whoami")` → permission → tool result → completed. `mcp-methods.jsonl` confirms the real packaged `zeron mcp` received `initialize`, `notifications/initialized`, `tools/list`, and `tools/call(whoami)` in both processes. The first resume attempt stalled in model thinking for 100 s; a fresh load, explicit settings, and explicit ToolSearch completed. |
| Q4, images | **`imagePaths` works.** `image-valid.jsonl`: a valid 32×32 red PNG → acknowledgement → deltas identifying red → completed. `image-unsupported-valid.jsonl`, model `glm-5.3` (`noImageSupport:true`), completed with text explaining the image was unavailable. Malformed one-pixel input produced `agent_turn_completed(error)` with no preceding error message, covered by the generic terminal-reason fallback. |
| Q5, multi-select | **One answer string, comma-separated labels.** `questions.jsonl`: `ask_user(questions:[{index:1,multiSelect:true,…}])` → `answers:[{index:1,question:…,answer:"Red, Blue"}]` → deltas `Red`, `, Blue` → completed. |
| Q6, retraction | **Not observed** in the captured plain, tool, question, image, resume, and background sequences. There is no shared transcript retraction event; the driver leaves already streamed text and emits a visible warning. It does not claim to remove text. A normalization unit test covers the warning. |
| Q7, Task children | `task.jsonl` and `background.jsonl`: parent Task chip/result plus `child_session_available` and `child_session_settled`; **no child transcript deltas on parent stdout**. Only the parent Task is shown; no fabricated nested transcript. A later parent wake is a new parent turn. |
| Q8, interrupting questions | **Interrupt unblocks both without an answer first.** `questions.jsonl`: pending permission/AskUser → `interrupt_session` → `agent_turn_completed(cancelled)`. A subsequent prompt completed in the same process. The driver additionally sends cancellation replies and `InputResolved(answers:[])`; fixture tests verify these host-side events. |
| Q9, process death | `death.jsonl`: initialized → prompt → permission → SIGKILL → stdout EOF (no terminal notification). Fixture crash tests require exactly one `Done(Errored)` with stderr detail; the stubborn-child test requires reaping after stream drop. |
| Q10, Windows | **Not runnable on this macOS host.** Existing process ownership uses creation-time Job Object membership. Windows spawn/live validation remains skipped. |

Additional captures: `signed-out.jsonl` uses an isolated
`FACTORY_HOME_OVERRIDE`, with `FACTORY_API_KEY` removed: initialize succeeds →
authentication error → `agent_turn_completed(model_authentication_failed)`.
The single sign-in mapping reports `Done(Errored)` with terminal `/login`
guidance, not the engine Cloud-path `HarnessError::NotSignedIn`.

## Validation

`cargo test --locked -p zeron-harness --features native-fixture --test droid --lib`
covers streaming, exact catalog IDs/cache, settings verification, approvals,
spec approval/cancellation and autonomy preservation, AskUser, autonomous parent
wakes, steering receipts/discards, deferred configuration, reusable stop,
missing-session fallback, auth/crash, and reaping.
`crates/engine/tests/droid_resume.rs` covers dispatch after runtime death.

`live_stream_edit_mcp_stop_resume_spec_and_steer` is ignored by default. Set
`DROID_LIVE_EXECUTABLE` and `DROID_LIVE_ZERON_EXECUTABLE` and run with
`--ignored --nocapture`. It spends real Factory credits in a disposable cwd.
Real signed-out user-account tests, manual GUI checks, Loop/cron, and Windows
are not part of deterministic fixture coverage.

Implementation-host results: locked `cargo check` passed for `zeron-harness`,
`zeron-engine`, `zeron-ui`, `zeron-client`, and `zeron-mobile`. Tests passed:
504 harness unit tests, 17 Droid fixtures, 98 protocol tests, 24 engine registry
tests, 51 updater tests, three engine resume tests, 86 ACP tests, and 51 Codex
tests. Four existing Codex live tests remained ignored. The explicit Droid live
acceptance passed with the real CLI and packaged Zeron MCP. New driver/fixture
format checks and `git diff --check` passed. The iOS app was not built.

Desktop E2E with Auto Model verified Low/Medium/High autonomy immediately after
explicit plan approval and through implementation, before any follow-up prompt.
Selecting Yes alone did not start work; Submit was required. No + Submit
cancelled without creating the proposed file.
