# How to Contribute to this repository #

We value contributions from the community and will do everything we
can to get them reviewed in a timely fashion. If you have code to send
our way or a bug to report:

* **Contributing Code**: If you have new code or a bug fix, fork this
  repo, create a logically-named branch, and [submit a PR against this
  repo](https://github.com/helium/helium-wallet-rs). Include a
  write up of the PR with details on what it does.

* **Reporting Bugs**: Open an issue [against this
  repo](https://github.com/helium/helium-wallet-rs/issues) with as much
  detail as you can. At the very least you'll include steps to
  reproduce the problem.

## Pre-commit checks

We use pre-commit to run Rust hygiene checks and security scans. 

### Prerequisites

1. Install `pre-commit`:
   ```sh
   brew install pre-commit
   ```

2. Install `trufflehog` for secret scanning:
   ```sh
   brew install trufflesecurity/trufflehog/trufflehog
   ```

### Setup

After installing the prerequisites, run:

```sh
pre-commit install
pre-commit run --all-files
```

To also run checks on `git push`, install the pre-push hook:

```sh
pre-commit install --hook-type pre-push
```

### Security checks

This repo uses TruffleHog to scan for hardcoded secrets before each commit.
The hook will fail if verified secrets are detected. If you need to exclude
specific files from scanning, contact a maintainer.

**Security note**: pre-commit can fetch and run hooks from third-party
repositories. This repo uses TruffleHog from the official trufflesecurity
repository, pinned to a specific version for security.

This project is intended to be a safe, welcoming space for
collaboration, and contributors are expected to adhere to the
[Contributor Covenant Code of
Conduct](http://contributor-covenant.org/).

Above all, thank you for taking the time to be a part of the Helium community.
