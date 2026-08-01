<p align="center"><img src="assets/phoenix.svg" alt="Pip the Phoenix" width="140"></p>

<h1 align="center">OpenPhoenix</h1>

<p align="center"><b>Rise clean. Leave no ashes.</b> 🔥</p>

<p align="center"><a href="https://openphoenix.app">openphoenix.app</a> · <a href="https://github.com/Paulus1337/OpenPhoenix/wiki">wiki</a> · <a href="https://github.com/Paulus1337/OpenPhoenix/releases">releases</a></p>

<p align="center">
<a href="https://github.com/Paulus1337/OpenPhoenix/actions/workflows/ci.yml"><img src="https://github.com/Paulus1337/OpenPhoenix/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
<a href="https://github.com/Paulus1337/OpenPhoenix/releases"><img src="https://img.shields.io/github/v/release/Paulus1337/OpenPhoenix" alt="Release"></a>
<a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-orange" alt="MIT"></a>
<img src="https://img.shields.io/badge/deps-0_goal-orange" alt="0 crates goal">
<img src="https://img.shields.io/badge/binary-static,_stripped-orange" alt="static binary">
</p>

Your own AI assistant, in a single file you can carry on a USB stick.
No Node, no Python, no runtime to install, nothing phoning home. It
talks to you in the terminal, on Telegram and eight other chat apps,
runs real tools on your machine, and remembers only what you tell it
to keep.

```
phoenix               # chat, right here, right now
phoenix serve         # go live on your chat apps
phoenix run "task"    # one shot, zero residue
```

## Why you might like it

- **It sets itself up.** First run hunts for API keys already in your
  environment, tests them against live models, keeps the one that
  answers.
- **You choose what survives.** Ghost forgets everything, session
  remembers until restart, recall keeps plain text notes you can read
  and delete yourself. No hidden database.
- **Every door starts locked.** Empty allowlist means nobody gets in.
  No web password means no web UI. You open doors on purpose.
- **Small enough to trust.** Six dependencies, no async runtime, no
  build scripts. You can read it before handing it a shell.
- **One config file.** Plain TOML, mode 600. Secrets stay in env vars
  or the encrypted secret store and never need to touch the config.

## Get it

Grab your file from
[Releases](https://github.com/Paulus1337/OpenPhoenix/releases): static
binaries for Linux (x86_64 and arm64), macOS (Apple silicon and
Intel), Windows, plus `.deb` packages, detached signatures, and
`SHA256SUMS`.

```
sudo dpkg -i openphoenix_0.0.1_amd64.deb                    # debian/ubuntu/kali
docker run --rm -v phoenix-data:/data ghcr.io/paulus1337/openphoenix init
cargo build --release                                       # from source
```

Coming from another AI gateway? Phoenix spots your old setup on first
run and asks whether to bring it along or start fresh. Your answer
decides; saying no leaves the old setup completely untouched. See
[Migration](https://github.com/Paulus1337/OpenPhoenix/wiki/Migration).

## Learn more

Everything lives in the **[wiki](https://github.com/Paulus1337/OpenPhoenix/wiki)**:

| | |
|---|---|
| [Install](https://github.com/Paulus1337/OpenPhoenix/wiki/Install) · [Quickstart](https://github.com/Paulus1337/OpenPhoenix/wiki/Quickstart) | binaries, docker, first flight |
| [Chat-Commands](https://github.com/Paulus1337/OpenPhoenix/wiki/Chat-Commands) | the slash commands you will actually use |
| [Configuration](https://github.com/Paulus1337/OpenPhoenix/wiki/Configuration) | the one TOML file, every knob |
| [Providers](https://github.com/Paulus1337/OpenPhoenix/wiki/Providers) | claude, gpt, gemini, openrouter, ollama, failover, key rings |
| [Channels](https://github.com/Paulus1337/OpenPhoenix/wiki/Channels) | telegram, whatsapp, discord, slack, signal, imessage, irc, matrix, mattermost |
| [Web-UI](https://github.com/Paulus1337/OpenPhoenix/wiki/Web-UI) | browser chat, HTTP API, webhooks, websocket, canvas |
| [Tools](https://github.com/Paulus1337/OpenPhoenix/wiki/Tools) | shell, files, browser, media, child agents, task board |
| [Memory](https://github.com/Paulus1337/OpenPhoenix/wiki/Memory) | privacy modes, recall, compaction, dreaming |
| [Skills](https://github.com/Paulus1337/OpenPhoenix/wiki/Skills) | markdown instruction packs, ClawHub |
| [Security](https://github.com/Paulus1337/OpenPhoenix/wiki/Security) | the model, approvals, doctor |
| [Migration](https://github.com/Paulus1337/OpenPhoenix/wiki/Migration) | bring your old gateway across, or do not |
| [Service-and-Updates](https://github.com/Paulus1337/OpenPhoenix/wiki/Service-and-Updates) | run it forever, update it safely |
| [Building](https://github.com/Paulus1337/OpenPhoenix/wiki/Building) | cross builds, tests, release flow |
| [Roadmap](https://github.com/Paulus1337/OpenPhoenix/wiki/Roadmap) | the way to 1.0 |

Every major version is a molt: burn what aged badly, keep the flame.

MIT. See [LICENSE](LICENSE).
