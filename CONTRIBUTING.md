# Contributing to OpenPhoenix

Thanks for helping Pip fly farther. OpenPhoenix is a compact personal AI agent runtime, so every change should stay understandable, secure by default, and easy to verify.

## Before you begin

- Use GitHub Discussions for questions and early ideas.
- Search existing issues and pull requests before opening a new one.
- Report vulnerabilities privately through the repository's [security advisory form](https://github.com/Paulus1337/OpenPhoenix/security/advisories/new). Do not open a public issue for a suspected vulnerability.
- Keep a change focused. Discuss broad architectural changes before investing in an implementation.

By contributing, you agree that your contribution is licensed under the repository's [MIT License](LICENSE).

## Development setup

Install Git and the current stable Rust toolchain. The repository's `rust-toolchain.toml` selects the minimal stable toolchain with Clippy and rustfmt.

```sh
git clone https://github.com/Paulus1337/OpenPhoenix.git
cd OpenPhoenix
cargo build --locked --bins
```

OpenPhoenix is one Rust crate with two binaries:

- `phoenix` is the runtime.
- `phoenix-e2e` is the end-to-end test driver.

The dependency graph is committed in `Cargo.lock`. Use `--locked` in validation and release builds. Do not update dependencies incidentally; dependency changes should be explicit and reviewed on their own merits.

## Make a focused change

1. Create a branch from the latest `main`.
2. Add or update tests with behavior changes.
3. Preserve fail-closed defaults and provider-neutral, client-neutral behavior.
4. Keep user-facing guidance in the README or wiki rather than adding source comments.
5. Do not commit credentials, generated release output, local state, or `target/`.
6. Update the wiki when a change affects installation, configuration, security boundaries, or operations.

The source intentionally avoids Rust comments and em dashes. Existing tests enforce both conventions. Prefer clear names and small functions for implementation clarity.

## Run every local gate

Run the same gates as CI before submitting a pull request:

```sh
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked -- --test-threads=1
cargo build --release --locked --bins
```

Tests use one harness thread because some tests isolate changes to process-wide environment variables. A complete test run can take several minutes.

If you changed container behavior, also build the source target:

```sh
docker build --target source -t openphoenix:local .
docker run --rm openphoenix:local --version
```

Do not weaken a check to make a contribution pass. Fix the cause or explain the constraint in the pull request.

## Security checklist

Changes touching tools, networking, authentication, updates, channels, storage, or command execution need additional scrutiny:

- Treat model output, channel messages, fetched content, tool arguments, and configuration as untrusted input.
- Preserve workspace confinement, including protection against symlink escapes.
- Keep private, loopback, link-local, metadata, and special-use network destinations blocked unless the user explicitly opts into private networking.
- Keep shell approval, command denial, egress policy, audit, and sandbox controls independent. No single control is a complete security boundary.
- Never log or echo credentials. Add redaction tests for newly supported credential formats.
- Keep secret storage encrypted and written with restrictive permissions.
- Do not introduce a new third-party action, binary download, install script, or large dependency without explaining its trust and maintenance cost.
- Add regression tests for security fixes without publishing exploitable secrets or unnecessary weaponized detail.

Read [SECURITY.md](SECURITY.md) before working on a security boundary.

## Pull requests

A useful pull request is small enough to review and includes:

- the problem and intended behavior;
- the design choice and important alternatives;
- security and compatibility impact;
- tests run and their results;
- documentation changes;
- screenshots or transcripts when user-visible output changes, with secrets removed.

Link the relevant issue when one exists. Use a clear imperative title. Keep review discussions technical and respectful, and resolve feedback with new commits rather than hiding the review trail.

Maintainers may ask for a change to be split, simplified, or moved to the wiki. Passing CI is necessary but does not guarantee acceptance.

## Releases

Maintainers publish releases from semantic version tags after all required checks pass. The project uses the current stable Rust channel; each release records the concrete compiler version used while `Cargo.lock` fixes dependencies. Contributors should not edit version numbers, release workflows, or generated release notes unless the pull request is specifically about a release. The release process is documented in the [wiki](https://github.com/Paulus1337/OpenPhoenix/wiki/Redeploying).
