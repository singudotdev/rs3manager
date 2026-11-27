# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-11-27

The first release: a desktop app for developers to work with the buckets and objects of their Ceph radosgw account, in
place of the MinIO console.

### Added

- Profiles from the AWS CLI's `~/.aws/config` and `~/.aws/credentials`, one radosgw account each: switched from the top bar, and added, edited and removed in the app, every other line of those files kept as it was.
- What the window shows follows the profile's own IAM policies, read at connect and on Refresh: buckets it can't list are hidden, actions it can't do aren't offered, and new buckets are checked against the names it may create.
- Buckets and folders: browse, filter, create, rename or move, and delete (a bucket emptied first when asked), with keyboard shortcuts.
- Uploads of files and folders, also dropped on the window, in parts for large files and asking before replacing what's there; downloads that continue where a cancelled or cut one stopped. Progress and cancelling in the transfers list.
- An object's details: preview of text and images, its headers and user metadata edited in place, and a share link that expires after an hour to seven days.
- A bucket's Settings: its tags, lifecycle rules (expiring objects, noncurrent versions and unfinished uploads) and CORS rules, read, edited and removed in one dialog, and its versioning turned on or suspended. Tabs follow the profile's policies; lifecycle rules set elsewhere with more than that are kept as they are.
- An object's versions, in a bucket that keeps them, listed in its details: restore one (it becomes the current version) or delete one for good.
- A Linux x86_64 package with a launcher entry and icon, and its SHA-256, on each release.
- `dev/rgw.sh` for the Ceph side: accounts with guardrails on their root users (no bucket policies or ACLs), admin users, and project accounts with read-write and read-only groups.

[Unreleased]: https://github.com/singudotdev/rs3manager/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/singudotdev/rs3manager/releases/tag/v0.1.0
