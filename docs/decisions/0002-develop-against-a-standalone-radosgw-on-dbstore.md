# 2. Develop against a standalone radosgw on dbstore

Date: 2026-11-03. Status: superseded by [3](0003-develop-against-a-one-node-ceph-in-a-container.md).

## Context

The production gateway, radosgw on Ceph under Proxmox, isn't reachable from the development machine, which has
rootless podman and no docker. Emulators (MinIO, Garage) answer S3 their own way, not RGW's.

Tried:

- The radosgw in Ceph's own image (`quay.io/ceph/ceph:v19`, 19.2.7) runs alone with the SQLite `dbstore` backend, with
  no monitors or OSDs. Signed path-style requests in region `default` work, and so does the official AWS CLI with a
  profile pointed at it.
- A full one-node Ceph in one container: the monitor, manager and radosgw started, but the OSD never came up (the
  monitor left it `down`, with no addresses, both on `127.0.0.1` and on the container's address).
- dbstore doesn't support [accounts](https://docs.ceph.com/en/squid/radosgw/account/): `radosgw-admin account create`
  fails with "Operation not supported".

## Decision

- `dev/rgw.sh` runs the standalone radosgw on dbstore in podman, with its data in a volume, and creates plain RGW users
  locally.
- On a real cluster, `dev/rgw.sh account` creates an account and its root user with radosgw-admin. Either way, it
  prints the AWS CLI profile to give the developer.

## Consequences

- The S3 code that answers is RGW's own; the storage under it is not RADOS. What depends on pools, placement or
  accounts (account-owned buckets, IAM) can only be checked on the real cluster.
- For the buckets and objects r3manager works on, a plain user's keys behave as an account root user's do.
