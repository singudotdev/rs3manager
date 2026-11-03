# 3. Develop against a one-node Ceph in a container

Date: 2026-11-03. Status: accepted; supersedes [2](0002-develop-against-a-standalone-radosgw-on-dbstore.md).

## Context

The core's integration tests against the standalone radosgw on dbstore (decision 2) failed where the code was right.
The official AWS CLI reproduced each failure against that gateway:

- `copy-object --metadata-directive REPLACE` onto the same key answered 200 and left the old headers;
- `copy-object` to another key answered 200 and created nothing;
- a 40 MB file uploaded in parts by `aws s3 cp` came back with different bytes.

dbstore also has no [accounts](https://docs.ceph.com/en/squid/radosgw/account/), which is how developers will connect.
r3manager renames and edits headers by copying, and uploads large files in parts, so that gateway can't tell whether
the app works.

Ceph's own demo image, `quay.io/ceph/demo:latest-squid` (Ceph 19.2.0), runs a monitor, manager, one OSD and radosgw
in one container, with objects in RADOS. Against it the same three CLI checks pass and accounts can be created.

## Decision

- `dev/rgw.sh` runs that image under podman with the host's network: the monitor and OSD find each other by address,
  which a rootless podman network broke when tried by hand. Every daemon is on 127.0.0.1; `RGW_FRONTEND_IP` keeps
  radosgw there too, since by default it would listen on all interfaces, the LAN included.
- Data is kept in two volumes (`/etc/ceph`, `/var/lib/ceph`); `down` keeps them, `reset` deletes them.
- `dev/rgw.sh account NAME` creates a user account and its root user, locally or on a cluster's node
  (`RGW_ADMIN=radosgw-admin`). `dev/test-rgw.sh` runs the core's integration tests as such a root user.

## Consequences

- The tests exercise RADOS, accounts and radosgw's real copy and multipart paths. Ceph 19.2.0 here, against the
  cluster's own Squid point release.
- The image is from December 2024 and the ceph-container project that builds it is no longer updated. When it stops
  working, the replacement is a VM with cephadm.
- Port 7480 and Ceph's ports (3300, 6800 and up) are taken on 127.0.0.1 while it runs.
