# Contributing

Small codebase, high bar. The rules that keep it that way:

- **Every change lands by pull request and needs approval from
  [@Paulus1337](https://github.com/Paulus1337).** No exceptions. The
  branch is protected and CODEOWNERS routes every PR to the maintainer.

- **Rust only.** No new languages, no comments in code: docs go to the
  [wiki](https://github.com/Paulus1337/OpenPhoenix/wiki).
- **No `unsafe` outside the documented shim.** The one exception is the
  small signal handler in `daemon.rs` (SIGTERM/SIGHUP/SIGINT for
  graceful shutdown); it is fenced by a unit test and a CI grep that
  fail the build if `unsafe` appears anywhere else. If something seems
  to require `unsafe`, find a safe alternative or redesign.
- **No em dashes.** Not in code, docs, UI strings, commit messages, PR
  titles, or release notes. Use a comma, colon, semicolon, or full stop
  instead. CI greps for the character and fails the build.
- **No new dependencies.** 0 crates is the goal. Every crate that
  remains is temporary and must have a justification in Cargo.toml.
  CI fails the build if a dependency has no justification comment.
- **No binary size limit.** Stripped, static, as small as possible.
  The release profile (strip, lto, codegen-units=1) handles this.
  Do not add a size gate to CI.
- **Fail closed.** Every new surface refuses by default.
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo test` must pass. CI enforces all three.

Details: [wiki/Building](https://github.com/Paulus1337/OpenPhoenix/wiki/Building).
Bugs and ideas: [issues](https://github.com/Paulus1337/OpenPhoenix/issues) ·
questions: [discussions](https://github.com/Paulus1337/OpenPhoenix/discussions).
