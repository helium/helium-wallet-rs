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
The hook will fail if any potential secrets are detected (including both 
verified and unverified patterns).

**If you encounter a false positive:**
- Ensure the flagged content is not a real secret
- Consider using environment variables or a secure secret manager instead
- For legitimate test fixtures or example keys that need to be committed, see exclusions below

**Excluding files from secret scanning:**

If you have legitimate test data or examples that trigger false positives, you can exclude them:

1. Create a `.trufflehog-exclude` file in the repo root with glob patterns:
   ```
   # Exclude test fixtures
   **/tests/fixtures/**
   **/examples/**
   
   # Exclude specific files
   path/to/test-data.json
   ```

2. Update `.pre-commit-config.yaml` to use the exclusion file by adding `--exclude-paths=.trufflehog-exclude` to the TruffleHog entry

3. Get approval from a maintainer before excluding paths from security scanning

**Security note**: pre-commit can fetch and run hooks from third-party
repositories. This repo uses TruffleHog from the official trufflesecurity
repository, pinned to a specific version for security.

This project is intended to be a safe, welcoming space for
collaboration, and contributors are expected to adhere to the
[Contributor Covenant Code of
Conduct](http://contributor-covenant.org/).

Above all, thank you for taking the time to be a part of the Helium community.
