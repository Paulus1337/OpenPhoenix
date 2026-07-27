<p align="center"><img src="assets/phoenix.svg" alt="Pip the Phoenix" width="140"></p>

<h1 align="center">OpenPhoenix</h1>

<p align="center"><b>Rise clean. Leave no ashes.</b> 🔥</p>

<p align="center">
<a href="https://github.com/Paulus1337/OpenPhoenix/actions/workflows/ci.yml"><img src="https://github.com/Paulus1337/OpenPhoenix/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
<a href="https://github.com/Paulus1337/OpenPhoenix/releases"><img src="https://img.shields.io/github/v/release/Paulus1337/OpenPhoenix" alt="Release"></a>
<a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-orange" alt="MIT"></a>
<img src="https://img.shields.io/badge/deps-6_crates-orange" alt="6 crates">
<img src="https://img.shields.io/badge/binary-~5_MB_static-orange" alt="static binary">
</p>

A personal AI agent in one static Rust binary. No Node, no Python, no
cloud in the loop. Small enough to read before you trust it with a
shell. Ghost mode by default: nothing in the ashes unless you choose
to keep it.

```
phoenix init          # write a sample config
phoenix               # chat
phoenix serve         # telegram, whatsapp, discord, slack, signal, web UI, cron
phoenix run "task"    # one-shot, zero residue
```

## Why phoenix

- **6 crates, no async runtime.** The whole tree fits in your head.
- **You choose what persists:** ghost / session / recall.
- **Fail closed everywhere:** empty allowlists refuse everyone, the web
  UI refuses to serve without credentials, path jail and command gate on
  by default.
- **One TOML file.** Secrets live in env vars, never on disk.
- **From scratch container:** ~8 MB image, no shell, no libc, TLS roots
  compiled in.

## Get it

Grab a static binary from [Releases](https://github.com/Paulus1337/OpenPhoenix/releases)
(Linux x86_64/arm64, macOS, Windows, `SHA256SUMS` attached), or:

```
docker run -v phoenix-data:/data ghcr.io/paulus1337/openphoenix init
cargo build --release        # from source
```

## Learn more

Everything lives in the **[wiki](https://github.com/Paulus1337/OpenPhoenix/wiki)**:

| | |
|---|---|
| [Install](https://github.com/Paulus1337/OpenPhoenix/wiki/Install) · [Quickstart](https://github.com/Paulus1337/OpenPhoenix/wiki/Quickstart) | binaries, docker, first flight |
| [Configuration](https://github.com/Paulus1337/OpenPhoenix/wiki/Configuration) | the one TOML file, every knob |
| [Providers](https://github.com/Paulus1337/OpenPhoenix/wiki/Providers) | anthropic, openai, openrouter, ollama, failover, key rotation |
| [Channels](https://github.com/Paulus1337/OpenPhoenix/wiki/Channels) | telegram, whatsapp, discord, slack, signal, imessage |
| [Web-UI](https://github.com/Paulus1337/OpenPhoenix/wiki/Web-UI) | HTTP API, embedded chat, canvas, hardening |
| [Tools](https://github.com/Paulus1337/OpenPhoenix/wiki/Tools) | shell, files, browser automation, media, child agents, task board |
| [Memory](https://github.com/Paulus1337/OpenPhoenix/wiki/Memory) | privacy modes, vector recall, compaction, dreaming |
| [Skills](https://github.com/Paulus1337/OpenPhoenix/wiki/Skills) | markdown instruction packs, ClawHub |
| [Security](https://github.com/Paulus1337/OpenPhoenix/wiki/Security) | the model, approvals, doctor |
| [Migration](https://github.com/Paulus1337/OpenPhoenix/wiki/Migration) | one command from your old gateway |
| [Building](https://github.com/Paulus1337/OpenPhoenix/wiki/Building) | cross builds, container smoke, release flow |
| [Roadmap](https://github.com/Paulus1337/OpenPhoenix/wiki/Roadmap) | the way to 1.0 |

Every major version is a molt: burn what aged badly, keep the flame.

MIT. See [LICENSE](LICENSE).
