# Changelog

## 0.0.1

First public cut. Everything before this was prototyping; history starts
clean here.

- Single static binary: agent runtime, gateway service, web UI, CLI.
- First flight wizard: guided setup or one-keystroke migration from an
  existing OpenClaw config (secrets carried over with consent, mode 600).
- Providers: anthropic, openai, openrouter, nvidia, gemini, ollama. Key
  resolution: config, then PHOENIX_API_KEY, then the provider's standard
  env var.
- Channels: Telegram (fail-closed chat allowlist) and the built-in web UI.
- `phoenix service install|uninstall|start|stop|restart|status|logs`:
  hardened systemd unit, secrets from $PHOENIX_HOME/env, never in the unit.
- Security core: workspace jail, command gate, secret redaction, output
  clipping, exec approvals via /approve.
- `phoenix doctor` config schema check and service status.
- Static builds: Linux x86_64/arm64 (musl), Windows x86_64.
