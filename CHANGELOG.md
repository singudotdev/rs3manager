# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- A download cancelled or cut by the network keeps what it wrote, and the next one asks the gateway only for the rest.

### Changed

- The AWS SDK's legacy HTTPS client (hyper 0.14, rustls 0.21) is gone: requests go through the SDK's current client.
- The project is called rs3manager, as it is for Ceph radosgw S3.
