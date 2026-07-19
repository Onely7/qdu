# Getting started with qdu

English | [日本語](GETTING_STARTED_ja.md)

## Requirements

qdu supports Linux and macOS with Python 3.10 through 3.14. Runtime operation uses only the Python standard library. `fzf` is optional and used only by `qdu browse`.

```bash
python3 --version
```

## Build and install

From a checkout:

```bash
make build
./dist/qdu --version
./install.sh
```

The installer copies the generated zipapp to `~/.local/bin/qdu`. If the command is not found, add that directory to your shell path:

```bash
# Bash: add to ~/.bashrc; Zsh: add to ~/.zshrc
export PATH="$HOME/.local/bin:$PATH"
```

You may instead install directly:

```bash
mkdir -p "$HOME/.local/bin"
install -m 755 dist/qdu "$HOME/.local/bin/qdu"
```

## The first three commands

Record your home directory, inspect the ranking, and later compare it with a second complete snapshot:

```bash
qdu snapshot
qdu show

# Run after files have changed.
qdu snapshot
qdu diff
```

Useful refinements include:

```bash
qdu show --max-depth 3 --top 50
qdu show --under Library/Caches
qdu diff --growth-only
qdu list
```

## Terminology

- A **profile** names one scan root and its saved defaults. `default` uses the current user's home unless configured otherwise.
- A **snapshot** is an immutable SQLite database of measurements and paths. It does not copy file contents.
- A **complete snapshot** had no recoverable scan errors and can become `latest`.
- An **incomplete snapshot** is retained as `latest-any` for inspection, but does not replace the most recent complete snapshot.
- **Allocated bytes** approximate consumed filesystem blocks; **apparent bytes** are logical file sizes.

## Create another profile

```bash
qdu profile add data --path /srv/data --keep-snapshots 60
qdu snapshot --profile data
qdu show --profile data
```

Profile names are deliberately restricted to safe filename components. Use letters, digits, `.`, `_`, or `-`; do not use an absolute path, `..`, `/`, or `\`.

If children of the root are separate mounts, opt in to crossing filesystem boundaries:

```bash
qdu profile add shared-home \
  --path /home \
  --cross-filesystems \
  --with-users
qdu snapshot --profile shared-home
```

Continue with [Snapshots and scanning](SNAPSHOTS_AND_SCANNING.md), then use the [Analysis guide](ANALYSIS_GUIDE.md).
