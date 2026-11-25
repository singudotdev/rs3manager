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

## Tests against Ceph

The integration tests (`core/tests/rgw.rs`, the window tests) need radosgw: `dev/rgw.sh up` starts a one-node Ceph in
podman, and `dev/test-rgw.sh` runs them against it (why this image: decision 0003). CI doesn't run them.

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
