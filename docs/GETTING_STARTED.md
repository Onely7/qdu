# Getting started with qdu

English | [日本語](GETTING_STARTED_ja.md)

## Requirements

qdu runs on Linux and macOS, on x86_64 and arm64. It is one executable with no runtime dependencies. `fzf` is optional and used only by `qdu browse`.

## Install

The [README](../README.md#install) covers the usual ways: `uvx qdu` to try it, `uv tool install qdu` (or `pipx install qdu`, or `python3 -m pip install --user qdu`) to keep it, and an executable from the latest release for machines without Python. For scheduled snapshots, use an installed qdu rather than `uvx`, because cron needs an executable that stays in place.

If `qdu` is not found after installation, add the directory that holds it to your shell path. `uv tool install` and `pip install --user` use `~/.local/bin`:

```bash
# Bash: add to ~/.bashrc; Zsh: add to ~/.zshrc
export PATH="$HOME/.local/bin:$PATH"
```

## Build from source

Building needs Rust 1.85 or later and a C compiler, which compiles the bundled SQLite. From a checkout:

```bash
make build                 # cargo build --release
make install               # copies target/release/qdu to ~/.local/bin/qdu
qdu --version
```

`make install PREFIX=/opt/qdu` installs to `/opt/qdu/bin/qdu` instead. Without a checkout, `cargo install --git https://github.com/Onely7/qdu --locked` builds and installs qdu into `~/.cargo/bin`.

## The first three commands

Record your home directory, inspect the ranking, and later compare it with a second complete snapshot. To record another directory, give it once with `--path`; the first snapshot of a new profile keeps the directory and the other settings it was given for later snapshots:

```bash
qdu snapshot
qdu show

# Run after files have changed.
qdu snapshot
qdu diff
```

Useful refinements include:

```bash
qdu show --max-depth 3 --top 50   # or: qdu -L 3 -n 50
qdu show Library/Caches           # what is inside one directory
qdu diff --growth-only
qdu history                       # the root's size in each snapshot
qdu list
```

## Terminology

- A **profile** names one scan root and its saved defaults. `default` scans the current user's home unless configured otherwise or given `--path`. All snapshots of a profile are of one root, so a profile without a configured path scans the root of its existing snapshots.
- A **snapshot** is an immutable SQLite database of measurements and paths. It does not copy file contents.
- A **complete snapshot** read everything the user may read and can become `latest`. Directories that the user may not read are listed by `qdu errors` and do not make a snapshot incomplete.
- An **incomplete snapshot** met another failure, such as an I/O error, and missed data that another scan might read. It is kept as `latest-any` for inspection, but does not replace the most recent complete snapshot.
- **Allocated bytes** approximate consumed filesystem blocks; **apparent bytes** are logical file sizes.

## Create another profile

```bash
qdu profile set data --path /srv/data --keep-snapshots 60
qdu snapshot --profile data
qdu show --profile data
```

Profile names are deliberately restricted to safe filename components. Use up to 64 letters, digits, `.`, `_`, or `-`, starting with a letter or digit; do not use an absolute path, `..`, `/`, or `\`. `profile set` also changes the given settings of an existing profile, which then needs no `--path`.

If children of the root are separate mounts, opt in to crossing filesystem boundaries:

```bash
qdu profile set shared-home \
  --path /home \
  --cross-filesystems \
  --with-users
qdu snapshot --profile shared-home
```

Continue with [Snapshots and scanning](SNAPSHOTS_AND_SCANNING.md), then use the [Analysis guide](ANALYSIS_GUIDE.md).
