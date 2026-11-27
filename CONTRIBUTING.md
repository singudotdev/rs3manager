# Contributing

Everything to build, test and understand the code is in [docs/development.md](docs/development.md). In short:

```sh
cargo fmt --all && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked
```

- Branch from `develop` and open pull requests against it: `main` only takes tested releases (see
  [Branches](docs/development.md#branches)).
- Keep pull requests focused, and describe the behavior change in the description.
- Add tests: one that fails without the change, and for every behavior it could affect.
- Contributions are licensed under the project's license, [AGPL-3.0-only](LICENSE) with the additional terms in
  [NOTICE](NOTICE).
