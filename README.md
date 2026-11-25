# rs3manager

A desktop app for developers to work with the buckets and objects of their Ceph RADOS Gateway (radosgw) account, as
they did in the MinIO console: browse folders, upload (files, folders, or dropped on the window), download, rename or
move, delete, preview text and images, edit an object's headers and metadata, and share a download link that expires
(one hour to seven days). Written in Rust with
[GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui).

Cluster administration (accounts, users, quotas) stays with the team running Ceph; rs3manager only does what an
account's keys allow.

## Profiles

rs3manager uses the AWS CLI's own profiles, in `~/.aws/config` and `~/.aws/credentials` (or wherever
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

The window shows only what the profile's policies allow. At connect (and on Refresh) it reads the profile's own IAM
identity, groups and policies and evaluates them: buckets it can't list aren't shown, actions it can't do aren't
offered, and the New bucket dialog shows the names it may create (e.g. `rs3manager-*-dev`) and refuses others before
sending. Each user needs a policy letting it read its own policies (`dev/rgw.sh project` writes one; see
`docs/decisions/0004-*`); without it the window asks the gateway instead and hides the buckets it's refused. The
account's root user may do everything in its account.

A share link is a presigned URL signed with the profile's keys: anyone holding it can download the object until it
expires, and it can't be revoked before then short of changing those keys.

On the Ceph side, `dev/rgw.sh account NAME` creates an account and its root user and prints that profile; on a cluster
node run it as `RGW_ADMIN=radosgw-admin RGW_ENDPOINT=https://s3.example.com dev/rgw.sh account NAME`.

Every account root gets guardrails (no bucket policies or ACLs, and it can't remove them; `dev/rgw.sh breakglass`
lifts them), and devsecops administer accounts through their own admin users, `dev/rgw.sh admin ACCOUNT USER`
(`docs/decisions/0005-*`). `dev/test-guardrails.sh` checks all of it against the local Ceph.

`dev/rgw.sh project PROJECT ENV USER rw|ro` sets up an account the way teams split them: account `PROJECT-ENV` (root
user `PROJECT-ENV-root`), groups `read-write-objects`, `read-only-buckets` and `read-write-buckets` whose policies cover
only buckets named `PROJECT-*-ENV`, and IAM user `USER` in `read-write-objects` plus the `rw` or `ro` buckets group;
it prints USER's profile. Its policies are drafts in the script, to swap for the ones production enforces.

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
cargo run -p rs3manager-app --release
```

On Linux GPUI needs the Wayland/X11, Vulkan and font development packages Zed lists for
[building on Linux](https://github.com/zed-industries/zed/blob/main/docs/src/development/linux.md).

### Installing on Linux

Into `~/.local`, with a launcher entry and icon (the window's app id is `rs3manager`, so docks group it under them):

```sh
cargo install --locked --path gui --root ~/.local
install -Dm644 gui/assets/rs3manager.desktop ~/.local/share/applications/rs3manager.desktop
install -Dm644 gui/assets/rs3manager.svg ~/.local/share/icons/hicolor/scalable/apps/rs3manager.svg
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
