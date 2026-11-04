# r3manager

A desktop app for developers to work with the buckets and objects of their Ceph RADOS Gateway (radosgw) account, as
they did in the MinIO console: browse folders, upload (files, folders, or dropped on the window), download, rename or
move, delete, preview text and images, edit an object's headers and metadata, and share a download link that expires
(one hour to seven days). Written in Rust with
[GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui).

Cluster administration (accounts, users, quotas) stays with the team running Ceph; r3manager only does what an
account's keys allow.

## Profiles

r3manager uses the AWS CLI's own profiles, in `~/.aws/config` and `~/.aws/credentials` (or wherever
`AWS_CONFIG_FILE` and `AWS_SHARED_CREDENTIALS_FILE` point). Each profile points at one radosgw account:

```ini
# ~/.aws/config
[profile team-a]
endpoint_url = https://s3.example.com
region = default

# ~/.aws/credentials
[team-a]
aws_access_key_id = ...
aws_secret_access_key = ...
```

The app starts on `AWS_PROFILE` (else `default`, else the first profile) and switches between them from the top bar.
Profiles added or edited in the app are written to those files, leaving every other line as it was, and work with
`aws --profile` too. Requests use path-style addressing.

A share link is a presigned URL signed with the profile's keys: anyone holding it can download the object until it
expires, and it can't be revoked before then short of changing those keys.

On the Ceph side, `dev/rgw.sh account NAME` creates an account and its root user and prints that profile; on a cluster
node run it as `RGW_ADMIN=radosgw-admin RGW_ENDPOINT=https://s3.example.com dev/rgw.sh account NAME`.

## Keys

| Key | Does |
| --- | --- |
| Click a folder / Backspace or Alt+Up | Open it / go up |
| Click, Ctrl+click, Shift+click, Ctrl+A | Select one, toggle, a range, all |
| Double-click an object | Download it |
| F2 | Rename or move the selected row |
| Delete | Delete the selected rows (asks first) |
| F5 or Ctrl+R | Refresh |
| Ctrl+F | Filter the folder |
| Enter / Escape | Confirm / close a dialog |

## Building

```sh
cargo run -p r3manager-app --release
```

On Linux GPUI needs the Wayland/X11, Vulkan and font development packages Zed lists for
[building on Linux](https://github.com/zed-industries/zed/blob/main/docs/src/development/linux.md).

### Installing on Linux

Into `~/.local`, with a launcher entry and icon (the window's app id is `r3manager`, so docks group it under them):

```sh
cargo install --locked --path gui --root ~/.local
install -Dm644 gui/assets/r3manager.desktop ~/.local/share/applications/r3manager.desktop
install -Dm644 gui/assets/r3manager.svg ~/.local/share/icons/hicolor/scalable/apps/r3manager.svg
```

`~/.local/bin` must be on the `PATH` the desktop session starts launchers with.

## Development

- `cargo test` runs the core's unit tests; `cargo test --workspace` adds the app's.
- `dev/rgw.sh up` starts a one-node Ceph with radosgw in podman, on 127.0.0.1:7480 (why:
  [decision 3](docs/decisions/0003-develop-against-a-one-node-ceph-in-a-container.md)).
- `dev/test-rgw.sh` runs the integration tests against it: the core's operations, and the window driven by clicks and
  keys.
- `cargo clippy --workspace --all-targets` must be warning-free; `cargo fmt --all` formats.

Decisions are recorded in [docs/decisions](docs/decisions).

## License

AGPL-3.0-only; see [LICENSE](LICENSE) and [NOTICE](NOTICE) for third-party material.
