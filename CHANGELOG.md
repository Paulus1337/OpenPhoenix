# Changelog

## Unreleased

**Fixes**

- The agent loop no longer stops a productive run after a fixed number
  of turns. `agent.max_turns` (default 24, documented in GOAL.md as
  "soft target, not a hard limit") was in fact wired as a hard
  `for _ in 0..max_turns` cap, so a long but genuinely productive
  tool-call chain was cut off mid-task with
  `(stopped: tool-loop budget reached)` regardless of whether it was
  making progress. Per standing product policy, the gateway imposes no
  iteration ceiling of its own: the only limit on a run is the model's
  own provider-side limits, plus the existing degenerate-loop detector
  (`loop_detect.rs`), which still stops a call that is truly stuck
  (identical call and result repeated, no-progress polling, alternating
  ping-pong). `max_turns` and the `[agent] max_turns` config key are
  removed entirely, along with the now-dead validation, schema entry,
  and CLI-reported "max turns per reply" line. Regression test
  `agent::tests::the_gateway_never_caps_a_productive_run` drives a mock
  backend through 200+ productive tool calls and asserts the run
  completes on the model's own "done", not on a turn count.
- Ghost-mode child agents no longer inherit a halved turn budget from
  their parent (dead code once the cap above was removed); ghost
  children still get their own wall-clock deadline, which was already
  the real backstop for a runaway child.

**Eye candy**

- `phoenix doctor` and `phoenix status` render inside a single Unicode
  box-drawn panel (`text::draw_box`) instead of a flat list of lines,
  with color-tagged `ok`/`warn`/`FAIL` markers and a one-line summary
  at the bottom of `doctor`. Falls back to plain, uncolored text when
  stdout is not a terminal (piped output, CI, `--json` unaffected).
- The web UI's unauthenticated landing page is a styled "this nest is
  locked" screen (dark theme, Pip mascot, brand orange) instead of a
  bare `{"error":"unauthorized"}` JSON blob, for a browser `GET /` or
  `GET /index.html` specifically. Every other endpoint, including the
  `/run` HTTP API used by scripts and integrations, is completely
  unchanged: still a 401 JSON body, same `WWW-Authenticate` header,
  same status code. Regression coverage in `http::web_tests` is
  unchanged and still green.

**More eye candy**

- Interactive `chat` shows a live braille spinner ("thinking") on stderr
  while waiting on the model or a tool result, instead of going silent
  for the whole round trip. It clears itself the instant a tool call
  fires or the reply starts printing; off entirely when stdout is not
  a terminal (`spinner.rs`, off-by-default-on-pipe covered by
  `start_with_no_tty_never_spawns_or_marks_active`).
- Tool-call event lines in interactive chat use a toolbox glyph and
  brand orange for the tool name instead of a bare `-> name {...}`.
- `phoenix --help` / `-h` color the command names and the "first time
  here?" section in brand orange when stdout is a terminal, plain text
  otherwise (piped output, CI, and the existing
  `the_help_text_lists_every_command_it_can_actually_run` test are
  unaffected since the plain command word is still contiguous in the
  string either way).

**New: colab, two models on one task**

- `/colab PROVIDER/MODEL TASK` puts your current model and a second named
  model on the same task together, sharing the workspace and tools, taking
  turns until one of them writes the exact marker line that means the task
  is genuinely done, or 6 rounds pass (`colab.rs`). Each model sees the
  other's replies labeled by name and is told explicitly to build on or
  correct what the other one said, not just restate it. Verified live
  against two different real NVIDIA-hosted models: the second model
  independently ran a tool, and the first caught and corrected a mistake
  in its answer before converging.
- Refuses to run with the same model on both sides, refuses an unknown
  provider kind or a spec with no model name, and never silently
  double-prefixes a same-provider model spec (regression coverage:
  `same_model_twice_is_refused`, `unresolvable_partner_spec_is_a_clean_error_not_a_panic`,
  plus a scripted-provider test driving a full two-model exchange to
  convergence and one proving the round cap holds without it).

## 0.0.1

First flight. Everything before this was prototyping; history starts
clean here.

One static binary: terminal chat, nine chat channels, an HTTP API and
web UI, browser automation, a canvas, MCP tools, skills, memory, cron,
a container sandbox, the Agent Client Protocol, and a background
service. Built with `clippy -D warnings` clean and the full unit and
container end-to-end suites green (counts live in CI, on every run).

**The basics**

- One static binary holding the whole bird: chat, background service,
  web UI, CLI. No runtime to install.
- `phoenix` chats, `phoenix serve` goes live on your chat apps,
  `phoenix run "task"` does one job and leaves nothing behind.
- One config file at `~/.openphoenix/config.toml`, written mode 600.
  Secrets can live in env vars or the encrypted secret store instead
  and never touch the config.

**First run sets itself up**

- Finds API keys already in your environment, probes them against live
  models, and keeps the first one that actually answers.
- No key at all? Every step that asks for one names the exact page you
  get it from, and the closing hint repeats it, so nobody has to go
  hunting. Ollama is offered as the free local route that needs none.
- Nothing detected, or you want control? Five short steps, every one
  with a default: provider (including any OpenAI-compatible endpoint),
  model picked from your provider's live catalogue plus an optional
  fallback, any of nine chat apps, an optional web UI and HTTP API with
  a choice of local-only or network bind, and ten optional extras.
  Everything off unless you ask for it.
- Spots an existing gateway setup and asks whether to migrate or
  start fresh. Answer no and it is left completely untouched.
- Say yes and it carries your keys, Telegram token and allowlist,
  model choice, fallback chain, persona files and daily notes.
  `phoenix migrate` does the same from the command line, dry-run by
  default.
- Refuses to install itself as a service while your old gateway is
  still running, so the two never fight over the same bot.

**Talking to it**

- Terminal chat with slash commands: `/model`, `/privacy`, `/lean`,
  `/thinking`, `/compact`, `/usage`, `/status`, and more.
- Channels: Telegram, WhatsApp, Discord, Slack, Signal, iMessage, IRC,
  Matrix, Mattermost. Every one refuses everybody until you add
  yourself to its allowlist.
- Photos, PDFs and voice notes come in; voice notes can go back out.
- Browser chat, HTTP API, webhooks and a websocket, all off unless you
  turn them on and set credentials.

**Models**

- 21 provider kinds work without touching `base_url`: anthropic, openai,
  openrouter, ollama, google, nvidia, groq, mistral, deepseek, xai,
  moonshot, cohere, together, novita, opencode, byteplus, volcengine,
  xiaomi, meta, huggingface, custom. Each one knows its own endpoint and
  its own environment variable.
- Three API dialects, not one. `anthropic-messages`, `openai-completions`
  and `openai-responses` differ in endpoint and in wire shape. Set
  `provider.api` to pick one; it is validated against the three known
  dialects and guessed from the provider when left empty.
- `phoenix models` lists the live catalogue from your provider, with
  aliases resolved, so you can see what you may actually ask for.
- Claude Code OAuth tokens work as Anthropic keys, detected by shape.
- Fallbacks switch model, provider, key and endpoint mid-conversation;
  `phoenix models --test-fallback` drives the whole chain live with
  per-link latency and verdicts.
- Several keys per provider rotate automatically on rate limits. Key
  cooldowns cap at 120 seconds, expire on their own, are dropped when
  stale at load, and `phoenix doctor` lists any active cooldown with
  its reason and time left.
- A billing 402 fails fast: no retry, no key rotation, no fallback
  churn (regression test
  `a_billing_402_fails_fast_no_retry_no_rotation`).

**Reliability**

- OAuth token refresh retries transient failures with backoff and fails
  fast on auth errors (oauth.rs).
- Context overflow sheds the oldest history mechanically (tool pairs
  intact), retries the same model, then falls back (agent.rs).
- An empty model reply (no text, no tool calls) is an error, not a
  silent blank turn: every dialect reports it, the turn retries up to
  three times, and only then does it surface, pointing at `/model`.
- Provider switches rebuild the backend and restore the previous config
  when the new one cannot be built; switching clears a stale dialect
  override. `/model`, `/models`, and `/fast` all use the same safe path.
- The fallback notice is set only after a successful switch, so the
  reply never claims a model that did not serve it.
- Scheduler catch-up sweeps run each job at most once, report a broken
  expression once, and never cascade duplicates after a sleep.
- Heartbeat skips beats while interactive traffic is active (busy window
  capped at ten minutes), and skips the model call entirely when the
  built-in prompt finds no usable HEARTBEAT.md.
- Provider calls accept `provider.timeout_secs` to override the built-in
  180 s / 300 s ceilings; every dialect honors it.
- Children get a wrap-up note when under a minute of wall-clock budget
  remains, then the hard stop still lands.
- Telegram polling cannot wedge: startup retries `getMe` with backoff
  and fails fast only on a rejected token, clears any stale webhook
  before polling and again when a poll answers 409, respects
  `retry_after` on 429, and keeps confirming processed updates by
  advancing the offset (five regression tests in telegram.rs).
- Telegram chunk sends are error-checked per chunk; failures are counted
  and named instead of silently dropped.
- WhatsApp webhook events are handled in message-timestamp order.
- `phoenix run` exits 1 when the reply is a provider error, so scripts
  and cron jobs can tell success from failure.

**Scheduling**

- `[job_defaults]` fills delivery for jobs that name none; per-job wins.
- Per-job `expect` marker flags runs whose result lacks it.
- Per-job `can_act = false` runs observe-only (no shell, writes, sends,
  or browser); `[heartbeat] can_act` does the same for beats.
- Per-job `model` override retargets that run only.
- Per-job `precheck` shell gate skips the run cheaply; `script` jobs
  deliver command output with no model call at all.
- `jobs.d/` loads one TOML file per job, sorted, with loud collisions.
- `phoenix jobs` shows each job's next fire time (and `next_at` in JSON).
- Job and heartbeat `chat_ids` accept `chat#tNN` to land in a forum topic.
- Cron webhooks never carry auth headers, and the delivery URL scheme
  is validated before any connection.
- `[update] check_hours` keeps the status update cache fresh, check-only.

**Sessions**

- `phoenix sessions snapshot|restore|diff|snapshots`: atomic 600-perm
  checkpoints, repair on restore, deterministic drift report.
- User images round-trip through session files.
- `session_history` pages backwards with `offset`.
- GET /sessions and DELETE /sessions/ID on the HTTP API, behind the
  bearer token; /run replies carry `X-Actual-Model`.
- A state file with a newer schema version or unreadable bytes is
  copied to a `.bak.json` beside itself before Phoenix starts fresh,
  and the startup log names the kept file.
- Damaged session files are quarantined instead of overwritten.

**Channels**

- Trusted sender envelopes on every channel with sanitized names and
  elapsed-time stamps.
- Telegram forum topics are first-class end to end; albums go out as
  media groups of up to ten photos with one-by-one fallback.
- Signal message edits arrive as "(edited) ..." texts. Discord edits
  reprocess the same way; bot and embed-only edits stay silent.
- Slack replies land in the thread that asked.
- Telegram parse_mode config; "plain" skips HTML rendering.

**Memory you can see**

- Three modes: ghost keeps nothing, session forgets on restart, recall
  writes plain text notes you can read, edit or delete.
- Long conversations summarize themselves so they never hit the wall,
  and `/compact` does it on demand.
- Memory lines carry a source tag (operator vs agent).
- Operator commands use the recall store directly; privacy governs what
  a conversation records on its own, not what you type deliberately.
- Optional dreaming: journals its day while idle.

**Policy and safety**

- `security.deny_tools` removes tools from the model schema and refuses
  them at dispatch, MCP names included.
- `security.confirm_tools` gates tools behind a yes: chat asks inline,
  serve queues for /approve with a deny re-check at dispatch.
- Egress domain lists (`allow_domains` / `deny_domains`, deny wins,
  subdomain match) enforced at http_get, web_search, and browser.
- Requests to private and loopback addresses are blocked, and a
  dual-stack DNS answer with even one private address is refused, so a
  web page cannot talk phoenix into poking your internal network.
  `security.allow_private_network` is the explicit opt-out; scheme
  checks still apply.
- Per-spawn `deny_tools` and jailed relative `workspace` for children.
- Configured credentials are masked in replies and tool results,
  including passwords hidden inside URLs. `config show` masks
  secret-named keys of any shape, commented-out secrets included.
- Giant tool results are capped on every path, MCP included.
- Skill installs verify the registry sha256 digest when present.
- Every reply printed to the terminal passes `sanitize_terminal`, which
  strips CSI and OSC escape sequences (including OSC-8 hyperlink
  wrapping and title injection) while keeping newlines and tabs.
- Generated media filenames include process id and an atomic sequence
  number, so two sessions writing in the same millisecond can never
  hand each other's file over.
- Background tasks start in their own session via `setsid` when
  available, and cancel/timeout kills signal the whole process group
  before the direct pid, so grandchildren cannot outlive the task.
- The audit log records memory writes, tool calls with args, auth
  denials, and per-turn token usage, rotating at 8 MB.
- `GET /health` is the only unauthenticated endpoint and answers only
  `{"ok":true}`; it carries the strong headers and never trips the
  auth rate limiter.
- The `unsafe` keyword is gone from the tree except a four-site signal
  shim in daemon.rs, enforced by a unit test that fails the build if
  any other file uses it or the shim grows.
- Comments are banned from Rust sources and enforced by a unit test;
  the wiki and markdown files carry the documentation.
- `phoenix doctor` audits the setup and says plainly what looks risky.

**Agent quality**

- The system prompt carries a context-use line every turn.
- `agent.tool_list = false` drops the tool inventory line.
- Persona PROMPT.md replaces the built-in base template; every other
  persona .md file loads after the known set, sorted.
- web_search accepts max_results (1-20).
- `[provider.headers]` adds headers to every model call with ${ENV_VAR}
  expansion; overrides replace built-ins case-insensitively.
- Heartbeat prompts carry the current time.
- `config check` reports unknown keys with misplaced-key hints and
  exits 1; doctor carries the same hints.
- Status shows the live serve probe with measured latency, session
  counts, and update state from the on-disk cache.

**Tools from other programs**

- A real MCP client: JSON-RPC over stdio, the `initialize` handshake,
  `tools/list` and `tools/call`. Servers you configure appear as
  ordinary tools named `mcp_<server>_<tool>`; `phoenix mcp call TOOL
  [JSON]` invokes one from the CLI without a model round-trip.
- Replies are matched to requests by id, so a server that chats while
  answering cannot have a stray notification mistaken for the answer.
- Environment variables that look like secrets are scrubbed before a
  server phoenix did not write is launched.
- Everything a server returns is fenced as untrusted data, with the
  framing note kept outside the fence so quoting the result gives you
  the payload rather than phoenix's own scaffolding.

**Keeping it running**

- `phoenix service install` for a tightened systemd unit that starts at
  boot, with secrets kept out of the unit file.
- `phoenix update` fetches the current release, verifies the checksum,
  demands a detached Ed25519 signature over the binary, verifies it
  against the pinned release key, and health-probes the swapped binary
  for 10 seconds, rolling back automatically on failure. Unsigned
  releases are refused by name.
- Cron jobs, a heartbeat check-in, and background tasks you can list
  and cancel.
- Local time comes from a hand-written TZif parser reading
  `/etc/localtime` (or `$TZ` under `/usr/share/zoneinfo`) with pure
  calendar math behind it, covered by fixture tests. Windows builds
  fall back to UTC for now.

**Guided setup**

- First flight uses arrow-key menus when it owns a terminal: single
  choice, space-to-toggle multi choice, and yes/no. Piped input and CI
  fall back to the numbered prompts, so scripted setup still works.
- `phoenix commands` prints the whole command registry, and the test
  suite asserts that everything advertised actually parses.

**Full command parity**

- All 66 documented OpenClaw commands are accounted for: 78 built here,
  5 refused by name with a reason (`clawbot`, `crestodian`, `openclaw`,
  `voicecall`, `worker`). `phoenix commands --json` prints both lists and
  a unit test fails the build if an advertised command does not parse.

**Talks to editors**

- `phoenix acp` speaks the Agent Client Protocol over stdio: JSON-RPC
  framing, `initialize` handshake, `session/new`, `session/prompt`,
  `session/cancel`, streamed `agent_message_chunk` updates and a real
  stop reason. Nothing answers before `initialize`, unknown session ids
  are refused, and a provider error ends the turn as a refusal.

**Shell commands can run in a container**

- `[sandbox] runtime = "docker"` (or `podman`) runs every shell command
  inside a container: capabilities dropped, no-new-privileges, pids
  capped, memory and cpu limited, network off by default, workspace
  mounted at /work. `read_only = true` adds a noexec tmpfs and nothing
  else writable.
- `phoenix sandbox status | check | args` shows the policy, warns about
  host networking, and prints the exact argv that would be used. The
  command string is never word-split by phoenix.

**Pairing, devices and nodes**

- `phoenix pairing` queues DM pairing requests from strangers behind an
  unambiguous six-character code and names the exact config key that
  approves one. Off unless `[pairing] enabled = true`; a stranger still
  gets nothing until the operator acts.
- `phoenix devices` pairs devices with scoped tokens. Only the SHA-256
  hash reaches disk, the token is shown once, rotation invalidates the
  old token immediately, and revoked devices can never authenticate.
- `phoenix nodes` enrolls nodes with declared capabilities. Enrolling
  grants nothing: approval is explicit, approval never grants a
  capability that was not asked for, and `shell` needs `--admin`. Node
  addresses pass the same SSRF gate as everything else.

**QR codes, offline**

- A complete QR encoder in the tree: byte mode, error correction level
  M, versions 1 to 10, Reed-Solomon over GF(256), block interleaving,
  all eight masks scored by penalty, BCH format and version bits. Output
  is verified against a real scanner.
- `phoenix qr pair | url | text` prints a scannable code in the
  terminal, half-block by default and `--large` when a camera is fussy.

**Policy, fleet, wiki, path**

- `phoenix policy check` measures the running setup against written
  rules (denied channels, required sandbox, required approvals, required
  audit log, denied tools, no secrets in the config, http token) and
  prints a stable SHA-256 attestation of the verdicts. An unknown rule
  is refused rather than quietly passing.
- `phoenix fleet` creates isolated per-tenant cells, each with its own
  state directory, 0700 directory, 0600 config with every door closed,
  and a port that never collides. `fleet env` prints the two exports
  that switch to one.
- `phoenix wiki` keeps a linked markdown vault: slugs that cannot escape
  the vault, `[[wiki links]]`, search, broken-link and orphan linting.
- `phoenix path resolve px://FILE/...` reads one addressable leaf out of
  a TOML, JSON, JSONL or markdown file, including `[field=value]` filters
  over JSONL, without a bespoke parser per caller.
- `phoenix claws` reads a `CLAW.md` agent package, plans every side
  effect before touching disk, and refuses to overwrite an agent.

**Browser, plugins, promos, dns, board**

- `phoenix browser open | snapshot | click | type | screenshot` drives
  the managed browser from the command line, not just from the model.
- `phoenix plugins` lists the real extension surface (mcp servers,
  hooks, skills) and names the broken ones.
- `phoenix promos` reads the ClawHub promo catalogue, drops entries with
  unknown providers or private base URLs, and prints a claim plan that
  never writes a key into your config.
- `phoenix dns plan | check` prints discovery records and shows how a
  host resolves, flagging private answers.
- `phoenix board show | dispatch | --json`, aliased as `workboard`.

**Shipped for**

- Linux x86_64 and arm64, macOS Apple silicon and Intel, Windows
  x86_64, `.deb` packages, and a multi-arch container image built from
  scratch with no shell inside.
