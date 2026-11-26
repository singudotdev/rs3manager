# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- A download cancelled or cut by the network keeps what it wrote, and the next one asks the gateway only for the rest.
- A bucket's Settings: its tags, lifecycle rules (expiring objects, noncurrent versions and unfinished uploads) and CORS rules, read, edited and removed in one dialog. Tabs follow the profile's policies; lifecycle rules set elsewhere with more than that are kept as they are.

### Changed

- The AWS SDK's legacy HTTPS client (hyper 0.14, rustls 0.21) is gone: requests go through the SDK's current client.
- The project is called rs3manager, as it is for Ceph radosgw S3.
