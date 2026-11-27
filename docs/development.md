# Development

## Build and run

A Rust workspace: `core/` (rs3manager-core: profiles and S3, without GPUI) and `gui/` (rs3manager-app, the GPUI app,
binary `rs3manager`). A plain `cargo build` or `cargo test` covers the core only; the app needs `-p rs3manager-app` or
`--workspace` (on Linux, `libfontconfig-dev libwayland-dev libx11-xcb-dev libxkbcommon-x11-dev libvulkan1`).

```sh
cargo build --release -p rs3manager-app   # the app: target/release/rs3manager
```

## Checks

```sh
cargo fmt --all && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked
cargo deny check                          # licenses, advisories and git sources (deny.toml)
```

Lints must be warning-free (`max_width = 120`, pedantic clippy, no `unsafe`; see `Cargo.toml` and `rustfmt.toml`).

## Tests

Six layers, from the fastest:

- **Unit tests**, inline in each module (`#[cfg(test)]`): names, policies, errors, the `~/.aws` editor, transfers,
  display detection. `cargo test --workspace`; CI runs them on every push.
- **Window tests** (`gui/src/ui/tests.rs`): the app in a GPUI test window, driven by clicks and keys, checked by what
  it shows. The ones that need a gateway are `#[ignore]`d in a plain `cargo test`.
- **Integration tests against Ceph** (`core/tests/rgw.rs` and the ignored window tests): `dev/rgw.sh up` starts a
  one-node Ceph in podman (why this image: decision 0003), and `dev/test-rgw.sh` runs them against it, then
  `dev/test-guardrails.sh` checks an account's guardrails with the AWS CLI. CI can't run Ceph, so these run on the
  developer's machine before each pull request that touches S3.
- **HTTPS** (`dev/test-https.sh`): the same integration tests through a TLS proxy (`dev/tls-proxy.py`) with its own CA,
  and a check that a gateway whose certificate isn't trusted is refused.
- **Smoke test of the release app** (`e2e/smoke-app.sh`, CI job `app smoke` under `xvfb`): the app says so and exits
  when there is no display, starts and stays up against an unreachable gateway, idles under 3% CPU, and draws a window
  that isn't black (screenshot in the job's artifact).
- **Mutation testing** of the pure modules (`cargo mutants`, CI job `mutants` on each pull request's diff): a mutant
  no test catches is a missing test. See below.

Scripts are checked with `shellcheck -x` in CI.

### Mutation testing

[cargo-mutants](https://mutants.rs/) alters the code (`==` for `!=`, a body for `Ok(())`…) and runs the tests again:
a change no test notices is behavior nothing guards. `.cargo/mutants.toml` says what it alters (the modules unit tests
judge without a gateway: the core's names, policies, errors, profiles and checks, and the app's state) and lists the
mutants no test can tell apart, each with why; anything else missed is a test to write. A mutant whose tests hang (an
endless loop) is cut at the [timeout](https://mutants.rs/timeouts.html) and counts as caught.

CI runs it on the lines a pull request changes (`.github/workflows/mutants.yml`). Locally, from a clean worktree on
disk, with its own target and temporary folder: each mutant's copy builds there, and `/tmp` is often a RAM-backed
tmpfs that a full build of the app fills.

```sh
S=~/.cache/rs3manager-mutants; mkdir -p $S/tmp && git worktree add --detach $S/tree HEAD && cd $S/tree
env -u CARGO_TARGET_DIR TMPDIR=$S/tmp CARGO_BUILD_JOBS=6 cargo mutants --workspace -j 2 -o $S   # or --in-diff FILE
```

## Branches

`main` is what's released and nothing else. Work happens on a branch per change (`feat/…`, `fix/…`, `docs/…`,
`test/…`, `ci/…`) from `develop`, and comes back to it through a pull request once CI passes, squashed into one
commit whose message is the pull request's title (one line, imperative, conventional: `feat:`, `fix:`, `docs:`,
`build:`, `chore:`, `ci:`). A release is cut from `develop` on `release/vX.Y.Z`, which adds only its notes and the
version, and is squashed into `main` through a pull request, then tagged. `main` is merged back into `develop`. A fix
for a released version starts from `main` on `hotfix/…` and goes back to both. Commits are signed (`-S`); pushed
branches are never rewritten.

## Dependencies

Dependabot opens weekly pull requests against `develop` (compatible bumps grouped). GPUI is pinned to one Zed tag with
the patches in `Cargo.toml` and moves by hand. Advisories that no release fixes are ignored in `deny.toml`, each with
its reason, and checked again when the crate that pulls them in moves.

## Releases

Pushing a tag `vX.Y.Z` that matches `version` in `Cargo.toml` runs `.github/workflows/release.yml`: it builds the app in
release mode on Linux, packages it with `.github/package.sh` (binary, launcher entry, icon, licenses, SHA-256) and
publishes a GitHub release with generated notes.

Versions are `0.Y.Z` for now: a release bumps `Z`, and only one that breaks something (profiles, files the app
writes, behaviour users rely on) bumps `Y`. Each release moves the `Unreleased` section of `CHANGELOG.md` under its
version and date. A pull request changing the workflow or the script builds and
packages the same way without publishing.
