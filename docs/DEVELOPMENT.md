# qdu development guide

English | [日本語](DEVELOPMENT_ja.md)

## Local setup

qdu is written in Rust and has no runtime dependencies. Development needs:

- Rust 1.85 or later with Clippy and rustfmt, for example from [rustup](https://rustup.rs/)
- A C compiler, which builds the bundled SQLite
- [pre-commit](https://pre-commit.com/) for the commit hooks, and [uv](https://docs.astral.sh/uv/) or [maturin](https://www.maturin.rs/) to build the PyPI package

```bash
rustup toolchain install stable --component clippy,rustfmt
make hooks
```

`make hooks` installs pre-commit hooks that check formatting with rustfmt and look for secrets with Gitleaks on every commit. Gitleaks rejects a commit whose staged changes contain a secret, such as an access token or a private key, and prints the finding with the value redacted. pre-commit builds Gitleaks on first use and downloads a Go toolchain if none is installed.

## Tests and quality gates

```bash
make test        # cargo test --locked
make lint        # rustfmt check and Clippy with warnings as errors
make format      # cargo fmt
shellcheck tools/benchmark.sh
pre-commit run --all-files
make secrets
```

`make secrets` scans every commit in the history with Gitleaks, which also catches a secret committed without the hooks installed.

Unit tests sit next to the code they test in `src/`. The tests in `tests/` run the built `qdu` executable with a private `HOME`, `XDG_CONFIG_HOME`, and `XDG_STATE_HOME`, and read the resulting `index.json` and SQLite files directly: `snapshots.rs`, `analysis.rs`, `operations.rs`, and `scanner.rs` by command family, `formats.rs` for the TSV headers, JSON keys, and table titles that scripts rely on, and `project.rs` for documentation links and release metadata. Fake `crontab` and `fzf` scripts on `PATH` stand in for the real programs. A few tests skip themselves where they cannot run: permission tests as root, non-UTF-8 file names on filesystems that reject them, and the mount-boundary test outside Linux.

Debug builds fail to read the directories named in `QDU_DEBUG_IO_ERROR`, relative paths separated by `:`, with an I/O error, so that tests can produce an incomplete snapshot; the tests that need it run only in debug builds, as `cargo test` builds by default. Release builds ignore the variable.

## Build

```bash
make build      # target/release/qdu
make install    # copies it to ~/.local/bin; PREFIX=/opt/qdu changes the prefix
make wheel      # the PyPI package, in dist/
make clean
```

The PyPI package is a wheel that contains the release executable and no Python code. maturin builds it with `bindings = "bin"` from `pyproject.toml` and takes the version from `Cargo.toml`. `make wheel` runs maturin through `uvx`; set `MATURIN=maturin` to use an installed copy.

## Project layout

```text
qdu/
├── .github/                 CI, CodeQL, Dependabot, and releases
├── .release-please-manifest.json
├── release-please-config.json
├── docs/                    paired Japanese and English guides
├── src/
│   ├── main.rs              entry point and exit codes
│   ├── cli.rs               command-line grammar
│   ├── commands/            one file per command
│   ├── scan/                parallel snapshot scanner: walk, tree, and write
│   ├── traverse.rs          what the scanner and the live scan share
│   ├── live.rs              parallel scan for 'qdu files --live'
│   ├── query.rs             read-only snapshot queries
│   ├── selector.rs          the ways to name a snapshot
│   ├── report.rs            one layout per report for tables, TSV, and JSON
│   ├── repository.rs        snapshot creation, retention, and compaction
│   ├── storage.rs           index and SQLite persistence
│   ├── signals.rs           cleanup when qdu is interrupted
│   └── ...                  configuration, rendering, units, and patterns
├── tests/                   tests that run the qdu executable
├── tools/benchmark.sh
├── Cargo.toml and Cargo.lock
├── pyproject.toml           PyPI packaging with maturin
└── Makefile
```

## How the scanner works

The scanner runs one task per directory on a thread pool: `--threads`, else `QDU_THREADS`, else the CPU count up to 16. Each directory gets a number when it is found, so every directory has a larger number than its parent; after the walk, one pass in descending order adds each directory's totals to its parent. A hard-linked file is counted once, for the path that sorts first, and rows are written in path order, so a snapshot does not depend on the thread count or on scheduling.

Memory grows with the number of directories, about 200 bytes each and 70 more with `--with-users`, and with the number of hard-linked files rather than the number of their links. Paths are built only while the rows are written, in the order SQLite keeps them, so no list of all paths is held; that order is sorted once and reused for the directory rows and for each owner's rows.

A directory is opened relative to its parent's descriptor while the parent stays open, so the length of the full path does not matter. How many directories stay open is limited by a budget derived from the descriptor limit (`RLIMIT_NOFILE`); past it, directories are opened by path, one component at a time when the path is too long. A directory kept open holds only its descriptor, not the directory stream and its buffer. Entries are examined with `fstatat` relative to their open directory; unlike `statx`, which the standard library uses on Linux, it does not trigger automounts. A directory is opened without following a final symbolic link and checked against the device and inode found in its parent's listing; a different device is accepted only while the directory's `..` is still the parent, as at a mount point. So a directory replaced during the scan cannot lead it outside the root. Every directory is claimed by device and inode, so a bind mount or a mount loop is read once; btrfs and overlayfs report one link per directory, so the link count cannot tell which to claim. Totals saturate instead of wrapping, and a failure is classified as vanished, denied, or an error (see [Snapshots and scanning](SNAPSHOTS_AND_SCANNING.md#complete-and-incomplete-snapshots)).

qdu writes snapshot schema 4, and still reads the schemas of the Python versions: 3 from 0.2.0 and 2 from 0.1.0. On SIGINT, SIGTERM, or SIGHUP, a dedicated thread removes the files qdu was writing, such as a partial snapshot database or a decompressed copy, and exits with status 128 plus the signal number.

## Continuous integration and security

`.github/workflows/test.yml` runs rustfmt, Clippy, and the tests on Linux and macOS, checks that the code builds with Rust 1.85, the minimum in `Cargo.toml`, and runs the static and distribution checks: ShellCheck, all pre-commit hooks, a Gitleaks scan of the full history, and a build of the wheel and the source distribution followed by installing the wheel and running `qdu --version`.

`.github/workflows/codeql.yml` is the CodeQL v4 advanced setup for the Rust code and the GitHub Actions workflows. It runs on default-branch pushes, pull requests, a weekly schedule, and manual dispatch with `security-extended` queries. GitHub disables scheduled workflows in a public repository after 60 days without activity; re-enable the schedule with `gh workflow enable codeql.yml`. Workflow permissions are limited to `contents: read`, adding `security-events: write` only to analysis. Every Action reference is pinned to a full commit SHA.

The checks are named `ubuntu-latest / Rust`, `macos-latest / Rust`, `Minimum supported Rust`, `Static and distribution checks`, `Analyze Rust`, and `Analyze Actions`. A branch ruleset that requires checks must list these names.

`.github/dependabot.yml` checks Cargo dependencies, pre-commit hook revisions, and GitHub Actions every Monday at 09:00 Asia/Tokyo and groups updates per ecosystem. Dependabot pauses itself when its pull requests stay untouched for 90 days; merging, closing, or commenting on one resumes it.

Repository administrators must enable Dependency graph, Dependabot alerts, Dependabot security updates, and secret scanning with push protection. If CodeQL default setup exists, switch it to advanced setup before merging the workflow to avoid duplicate configurations.

## Releases and version tags

Release Please maintains a single Release PR from Conventional Commits merged into `main`. The workflow proposes the next semantic version, updates `CHANGELOG.md`, `.release-please-manifest.json`, `Cargo.toml`, and `Cargo.lock`, and lists changes since the previous release. Merging that Release PR creates a `v<version>` tag and a draft GitHub Release. The workflow then builds, from that tag:

- wheels for Linux (manylinux2014 and musllinux, x86_64 and aarch64) and macOS (x86_64 and arm64), and the source distribution;
- a smoke test of each Linux build and of the Apple silicon build, which takes a snapshot of a small tree before anything is published;
- after the smoke tests, the upload of the wheels and the source distribution to PyPI;
- archives of the Linux (glibc and musl) and macOS executables, each with `LICENSE` and `README.md`, named `qdu-<target>.tar.gz`, which it attaches to the release with `SHA256SUMS` before it publishes the release.

Use squash-merge and give the resulting commit a Conventional Commit title:

- `fix: ...` proposes a patch release.
- `feat: ...` proposes a minor release.
- `feat!: ...` or a `BREAKING CHANGE:` footer proposes a breaking release. Before 1.0, breaking changes remain on the `0.x` line.
- `docs:`, `test:`, `refactor:`, and `ci:` describe non-feature work and do not independently force a feature release.

Release state is configured by `release-please-config.json` and `.release-please-manifest.json`. The version is defined only in `Cargo.toml`; the wheel and `qdu --version` read it from there. Releases are created as drafts with their tag (`draft` and `force-tag-creation`), so a release becomes public only with its binaries.

The workflow falls back to the repository `GITHUB_TOKEN`. Pull requests and tags created with that token do not trigger additional workflows. To run CI on Release Please PR updates and tag-created events, configure a fine-grained `RELEASE_PLEASE_TOKEN` Actions secret with repository contents and pull-request write access. Never commit the token.

### Publishing to PyPI

The workflow publishes with PyPI trusted publishing, so no PyPI token is stored in GitHub. Set it up once before the first release:

1. On PyPI, open "Your account" → "Publishing" and add a pending trusted publisher with project name `qdu`, owner `Onely7`, repository `qdu`, workflow `release-please.yml`, and environment `pypi`.
2. On GitHub, create an environment named `pypi` under Settings → Environments. Adding required reviewers there makes each upload wait for approval.

If an upload fails, for example because the publisher was not set up yet, fix the cause and re-run the failed jobs of that workflow run.

## Change discipline and benchmarking

Perform a large responsibility split as a behavior-preserving commit before adding performance changes. Mixing the two makes regressions hard to attribute. Preserve CLI syntax, exit codes, table/TSV/JSON contracts, storage locations, and index/schema versions unless a change is announced as breaking. A change of a TSV header or JSON key must update `tests/formats.rs`, and one that removes or redefines a key also raises `FORMAT_VERSION` in `src/report.rs`.

On Linux, run:

```bash
make bench
QDU=/path/to/another/qdu tools/benchmark.sh
```

The script generates 2,000 directories and 200,000 files; set `QDU_BENCH_DIRECTORIES` and `QDU_BENCH_FILES` to change the size. It reports elapsed time and peak RSS for snapshots with owner statistics, `show`, `files`, and `diff`, plus the snapshot file size. `benchmark_results.txt` records the latest comparison with 0.2.0, the last Python version.

For structural work, measure the pre-split, post-split, and final revisions five times each in the same environment and dataset. Compare median elapsed time and peak RSS; investigate regressions above 10%. The included script expects GNU `/usr/bin/time`; install GNU time or adapt only the measurement wrapper on macOS.

## Security boundaries

- Validate profile identifiers at configuration and state-path boundaries.
- Treat indexed filenames as untrusted persisted input; accept only approved snapshot basenames.
- Select SQL clauses from static templates and bind every value.
- Resolve optional executables (`fzf`, `crontab`) on `PATH` and run them without a shell.
- Keep `unsafe` to single libc calls, each with a `SAFETY` comment that states why the call is sound.
- Explain hard-link handling, mount boundaries, durability, atomic replacement, and terminal escaping with concise reason comments.
