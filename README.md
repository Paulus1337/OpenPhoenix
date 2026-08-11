<p align="center">
  <a href="https://github.com/Paulus1337/OpenPhoenix/wiki"><img src="assets/phoenix.svg" alt="Pip the Phoenix" width="150"></a>
</p>

<h1 align="center">OpenPhoenix</h1>

<p align="center"><b>Hi, I am Pip.</b><br>Your own AI agent, in one small Rust binary. No runtime, no telemetry, no nonsense.</p>

<p align="center">
  <a href="https://github.com/Paulus1337/OpenPhoenix/actions/workflows/phoenix-ci.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/Paulus1337/OpenPhoenix/phoenix-ci.yml?branch=main&style=for-the-badge&label=CI&labelColor=14100d&color=ff8c2e"></a>
  <a href="https://github.com/Paulus1337/OpenPhoenix/releases"><img alt="Release" src="https://img.shields.io/github/v/release/Paulus1337/OpenPhoenix?include_prereleases&sort=semver&style=for-the-badge&labelColor=14100d&color=f2600f"></a>
  <a href="LICENSE"><img alt="MIT" src="https://img.shields.io/badge/license-MIT-ffd968?style=for-the-badge&labelColor=14100d"></a>
  <a href="https://www.rust-lang.org/"><img alt="Rust stable" src="https://img.shields.io/badge/rust-stable-000000?style=for-the-badge&logo=rust&labelColor=14100d"></a>
  <a href="https://github.com/Paulus1337/OpenPhoenix/wiki"><img alt="Wiki" src="https://img.shields.io/badge/docs-the%20wiki-blue?style=for-the-badge&labelColor=14100d"></a>
</p>

<p align="center"><a href="https://openphoenix.app">openphoenix.app</a></p>

## Rise and shine

```text
phoenix
```

That is the whole setup. On first flight I look around your machine for a model I can talk to, write myself a config, and say hello. If you have a key in your environment, I will find it. If you do not, a local Ollama works fine and costs nothing.

```text
phoenix                  chat with me right here
phoenix serve            go live on your chat apps
phoenix run "task"       one job, then I forget it ever happened
```

## What I can do for you

- **Bring a friend.** Turn on Colab and two models take your task as equals. They argue about the plan, split the work, run their halves at the same time, then check each other before you see a word of it.

- **Live where you already chat.** Telegram, WhatsApp, Discord, Slack, Signal, IRC, Matrix, Mattermost, iMessage. Also a browser UI, an HTTP API, and a WebSocket if you would rather build your own thing.

- **Keep secrets like a professional.** Your keys come from the environment or a sealed store, never from a file in this repo. Known credential patterns are redacted from logs, errors, tool output, and replies.

- **Stay in my lane.** File tools stop at the workspace fence. Risky commands wait for your yes. Web requests refuse to wander into your private network. Everything optional starts switched off.

- **Remember only what you allow.** Ghost forgets instantly, session remembers this conversation, recall keeps plain text notes you can open, edit, or delete yourself.

- **Do the boring parts.** Cron jobs, background tasks, a heartbeat that checks in, skills you can teach me, and MCP tools when you want more hands.

## Where the real documentation lives

The [**wiki**](https://github.com/Paulus1337/OpenPhoenix/wiki) is the proper documentation. This page is just me waving.

| If you want to | Read this |
|---|---|
| Get flying in five minutes | [Quickstart](https://github.com/Paulus1337/OpenPhoenix/wiki/Quickstart) |
| Install or build me | [Install](https://github.com/Paulus1337/OpenPhoenix/wiki/Install) |
| Turn knobs | [Configuration](https://github.com/Paulus1337/OpenPhoenix/wiki/Configuration) |
| Pick a model | [Providers](https://github.com/Paulus1337/OpenPhoenix/wiki/Providers) |
| Wire up a chat app | [Channels](https://github.com/Paulus1337/OpenPhoenix/wiki/Channels) |
| Watch two models argue | [Colab](https://github.com/Paulus1337/OpenPhoenix/wiki/Colab) |
| Know exactly how I am locked down | [Security](https://github.com/Paulus1337/OpenPhoenix/wiki/Security) |
| See how I am put together | [Architecture](https://github.com/Paulus1337/OpenPhoenix/wiki/Architecture) |
| Build and ship me yourself | [Building](https://github.com/Paulus1337/OpenPhoenix/wiki/Building) |

## Joining in

Changes are welcome. [CONTRIBUTING.md](CONTRIBUTING.md) says how, [SECURITY.md](SECURITY.md) says what to do if you find something scary, and [LICENSE](LICENSE) says MIT.

<p align="center"><sub>Built in Rust. Runs anywhere. Answers to you.</sub></p>
