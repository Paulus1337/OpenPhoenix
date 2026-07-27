# Contributing

Small codebase, high bar. The rules that keep it that way:

- **Rust only.** No new languages, no comments in code: docs go to the
  [wiki](https://github.com/Paulus1337/OpenPhoenix/wiki).
- **No new dependencies** without a very good reason. Six crates is the
  budget.
- **Fail closed.** Every new surface refuses by default.
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo test` must pass. CI enforces all three.

Details: [wiki/Building](https://github.com/Paulus1337/OpenPhoenix/wiki/Building).
Bugs and ideas: [issues](https://github.com/Paulus1337/OpenPhoenix/issues) ·
questions: [discussions](https://github.com/Paulus1337/OpenPhoenix/discussions).
