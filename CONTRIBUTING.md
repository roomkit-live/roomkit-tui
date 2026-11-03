# Contributing

Thank you for helping improve RoomKit TUI. Bug reports, documentation, fixes
and features are all welcome. For a large change, open an issue first so we can
agree on the approach before you invest time in it.

## Getting started

You need Rust through [rustup](https://rustup.rs); the repository pins its
toolchain, and rustup installs it on first use. The development checks also
need Python 3.11 or later and `cargo-deny`
(`cargo install cargo-deny --locked`).

```sh
git clone https://github.com/roomkit-live/roomkit-tui
cd roomkit-tui
make check
```

## Design rules

Keep the terminal independent of any particular application domain. An engine,
provider or tool dependency belongs to an explicit adapter, not to the protocol
or terminal library. Host applications own policies and business behavior.

The adapter depends on the shared transport and protocol, never the TUI. Keep
disconnects separate from cancellation; do not infer per-request context
occupancy from an engine's cumulative usage or rebuild structured results from
model text.

All engine crates must use the same pinned Git URL and a commit on
`roomkit-rs/main`. Do not make these crates depend on a long-lived compatibility
branch.

## Checks

Run `make check` before submitting a change. Run `make msrv` when adding a
dependency or using a new standard-library API, and `make audit` when changing
dependencies. When working offline, set `CARGO_NET_OFFLINE=true`; the audit
additionally accepts `DENY_FLAGS=--offline` and checks only the cached advisory
database.

Use tests that exercise observable behavior: permission settlement, cancellation,
scoped event correlation, overload handling, rendering and cleanup. Scripted
hosts must be deterministic. Live-provider checks supplement offline tests and
must never require credentials in the ordinary test suite.

The POSIX pseudo-terminal smoke test runs on Linux and macOS. It checks input,
permissions, completion and terminal restoration, and rejects hangs with a
deadline. It does not replace visual review of an interactive terminal.

## Public interfaces

Document every public item. Return failures rather than panicking on caller
input. Keep model state, request acknowledgment and execution completion
separate. Preserve tool data independently from text presented to a model.

The application owns the runtime and resource lifecycle. Avoid hidden global
state or terminal access in constructors. Keep the pure session projection and
renderer testable without a terminal. Add production dependencies to the
workspace and explain their purpose.

## Documentation and distribution

Write code, comments, public documentation and fixtures in English. Use generic,
fictional records and environment-provided credentials. Do not commit local
planning, private integration data, machine paths, credentials or session logs.
Library manifests use explicit package include lists; `make package-check`
verifies actual Cargo package listings.

When the interface changes visibly, regenerate `assets/screenshot.png` with
`scripts/screenshot.py`; its header lists the commands.

## Releasing

`roomkit-tui-protocol`, `roomkit-tui-transport` and `roomkit-tui` are crates.io
packages. `roomkit-tui-engine` and the examples are not: each sets
`publish = false`. A host takes the engine through Git, with `roomkit-tui` from
the same commit, so that both share the protocol's types.

1. Set the version in `workspace.package` and in the internal requirements of
   `workspace.dependencies`: they change together. Run
   `cargo update --workspace --offline` and check that the `Cargo.lock` diff
   changes only the roomkit-tui packages.
2. Turn the changelog's `Unreleased` section into the release's, and update its
   comparison links and the README's version.
3. Commit, then run `make check`, `make msrv` and `make audit` on that commit,
   and push it.
4. Package every published crate, build each against the others in a local
   registry and stop before the upload:

   ```sh
   cargo publish --workspace --dry-run --locked
   ```

5. Publish from that clean checkout, with `cargo publish --workspace --locked`.
   crates.io never takes a version back: a yank only hides it from new
   dependents, so a fix is a new version.
6. Once the upload has succeeded, tag that commit, annotated, naming the checks
   run on it, and publish its GitHub release from the changelog's section. A tag
   marks what shipped.

## Pull requests

Use conventional commit subjects, for example `feat(tui): display scoped tool
results`. Describe the resulting behavior and how you validated it in the pull
request. Record user-visible changes in `CHANGELOG.md`.

By contributing, you agree that your contributions are licensed under the
[MIT License](LICENSE), and to follow the [code of conduct](CODE_OF_CONDUCT.md).

## Security

Do not report vulnerabilities in public issues; follow [SECURITY.md](SECURITY.md).
