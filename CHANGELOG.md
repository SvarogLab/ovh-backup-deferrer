# Changelog

All notable changes to this project will be documented in this file.

This project adheres to [Keep a Changelog](https://keepachangelog.com/en/1.0.0/)
and follows [Semantic Versioning](https://semver.org/).

## [0.1.3] - 2026-11-11

### Build system 🛠️

- Track the lockfile again (#10)

### Fixed 🐛

- Retry transient api failures instead of exiting on them (#9)
- Update rust crate jiff to 0.2.43 (#11)
- Update rust crate serde_json to 1.0.154 (#12)
- Update rust crate tokio to 1.54.1 (#13)

### Miscellaneous 🧹

- Update svaroglab/.github action to v2 (#14)

## [0.1.2] - 2026-10-28

### Build system 🛠️

- Stop tracking the lockfile

### CI/CD ⚙️

- Check through the shared workflow and publish from the new location (#7)

### Miscellaneous 🧹

- Let renovate update dependencies through the shared preset

## [0.1.1] - 2026-09-28

### CI/CD ⚙️

- Publish the helm chart to ghcr for release tags

## [0.1.0] - 2026-09-28

### Added ✨

- Defer the vps automated backup to an hour ago, every hour

### Miscellaneous 🧹

- Initialise the repository
