# qdu development guide

English | [日本語](DEVELOPMENT_ja.md)

## Local setup

qdu has no runtime dependencies. Install pinned development tools from the optional dependency group:

```bash
python3 -m pip install -e '.[dev]'
```

Supported interpreters are Python 3.10 through 3.14 on Linux and macOS.

## Tests and quality gates

```bash
make test
make lint
make format-check
python3 -m compileall -q src tools tests
shellcheck install.sh tests/run_tests.sh tools/benchmark.sh
pre-commit run --all-files
```

The test suite exercises source modules and rebuilds/runs the exact `dist/qdu` zipapp. Tests are divided by snapshot behavior, analysis, operations, query boundaries, scanner boundaries, rendering/utilities, and distribution.

Ruff targets Python 3.10 and enforces correctness, modernization, security, and docstring (`D`) rules. Public production and development APIs use PEP 257 with Google-style docstrings. Types belong in annotations; docstrings describe meaning, constraints, side effects, and recoverable exceptions. Tests rely on descriptive names instead of mandatory docstrings.

## Build

```bash
make build
./dist/qdu --version
make install
make clean
```

`tools/build_zipapp.py` copies the package into a temporary staging directory and creates the single-file executable. `dist/` is generated and ignored in this checkout.

## Continuous integration and security

`.github/workflows/test.yml` runs the test matrix on Linux and macOS with Python 3.10, 3.11, 3.12, 3.13, and 3.14. It also runs compileall, Ruff check/format-check, ShellCheck, and a zipapp smoke test.

`.github/workflows/codeql.yml` is the Python CodeQL v4 advanced setup. It runs on default-branch pushes, pull requests, a weekly schedule, and manual dispatch with `security-extended` queries. Workflow permissions are limited to `contents: read`, adding `security-events: write` only to analysis. Every Action reference is pinned to a full commit SHA.

`.github/dependabot.yml` checks pip and GitHub Actions every Monday at 09:00 Asia/Tokyo and groups updates per ecosystem.

Repository administrators must enable Dependency graph, Dependabot alerts, and Dependabot security updates. If CodeQL default setup exists, switch it to advanced setup before merging the workflow to avoid duplicate configurations.

## Project layout

```text
qdu/
├── .github/                 CI, CodeQL, and Dependabot
├── docs/                    paired Japanese and English guides
├── src/qdu/
│   ├── commands/            command facade and responsibility modules
│   ├── cli.py               parsing and exit-code boundary
│   ├── query.py             read-only snapshot queries
│   ├── scanner.py           filesystem traversal
│   └── storage.py           index and SQLite persistence
├── tests/                   responsibility-oriented regression tests
├── tools/                   zipapp and benchmark utilities
├── Makefile
└── pyproject.toml
```

`qdu.commands` remains a public compatibility facade that re-exports command handlers from analysis, snapshots, operations, and configuration modules. The package version has a single source in `qdu._version` and remains available as `qdu.__version__`.

## Change discipline and benchmarking

Perform a large responsibility split as a behavior-preserving commit before adding performance changes. Mixing the two makes regressions hard to attribute. Preserve CLI syntax, exit codes, table/TSV/JSON contracts, storage locations, index/schema versions, and import compatibility.

On Linux, build the artifact and run:

```bash
QDU=dist/qdu tools/benchmark.sh
```

For structural work, measure the pre-split, post-split, and final revisions five times each in the same environment and dataset. Compare median elapsed time and peak RSS; investigate regressions above 10%. The included script expects GNU `/usr/bin/time`; install GNU time or adapt only the measurement wrapper on macOS.

## Security boundaries

- Validate profile identifiers at configuration and state-path boundaries.
- Treat indexed filenames as untrusted persisted input; accept only approved snapshot basenames.
- Select SQL clauses from static templates and bind every value.
- Resolve optional executables to absolute paths and never invoke a shell.
- Prefer explicit runtime validation to control-flow `assert` statements.
- Explain hard-link handling, mount boundaries, durability, atomic replacement, and terminal escaping with concise reason comments.
