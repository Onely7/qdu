# Run qdu weekly with tmux

English | [日本語](QDU_TMUX_WEEKLY_SNAPSHOT_GUIDE_ja.md)

This recipe is for an ordinary user on a host where cron or a user service manager is unavailable, but a long-running tmux server is acceptable. A reboot or tmux-server shutdown stops the loop; it is not a durable scheduler.

## Prerequisites

```bash
command -v qdu
qdu --version
command -v tmux
```

Take one foreground snapshot first and confirm the intended profile, permissions, exclusions, and filesystem scope:

```bash
qdu snapshot --profile shared-home
qdu list --profile shared-home
```

## Create the loop

Save the following as `~/.local/bin/qdu-weekly-snapshot`. Change `PROFILE` to a valid qdu profile name.

```bash
#!/usr/bin/env bash
set -u

QDU_BIN="$HOME/.local/bin/qdu"
PROFILE="shared-home"
INTERVAL_SECONDS=$((7 * 24 * 60 * 60))
LOG_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/qdu/logs"
LOG_FILE="$LOG_DIR/weekly-snapshot.log"

mkdir -p "$LOG_DIR"

while :; do
    started=$(date -Is)
    if "$QDU_BIN" snapshot --profile "$PROFILE" --quiet >>"$LOG_FILE" 2>&1; then
        printf '%s snapshot complete: profile=%s\n' "$started" "$PROFILE" >>"$LOG_FILE"
    else
        status=$?
        printf '%s snapshot failed: profile=%s status=%s\n' \
            "$started" "$PROFILE" "$status" >>"$LOG_FILE"
    fi
    sleep "$INTERVAL_SECONDS"
done
```

The first snapshot starts immediately. The loop waits seven days after each attempt, so its start time can drift by the scan duration.

```bash
chmod 755 "$HOME/.local/bin/qdu-weekly-snapshot"
bash -n "$HOME/.local/bin/qdu-weekly-snapshot"
```

Run it once in the foreground if the selected root is small enough, then interrupt with Ctrl-C after confirming the log. For a large root, validate the equivalent `qdu snapshot` command directly instead.

## Start and inspect tmux

```bash
tmux new-session -d -s qdu-weekly "$HOME/.local/bin/qdu-weekly-snapshot"
tmux has-session -t qdu-weekly
tmux list-sessions
tail -n 100 "${XDG_STATE_HOME:-$HOME/.local/state}/qdu/logs/weekly-snapshot.log"
```

Attach interactively with `tmux attach -t qdu-weekly`. Detach without stopping the loop by pressing Ctrl-B, then D.

Confirm qdu results independently:

```bash
qdu list --profile shared-home
qdu show --profile shared-home
qdu errors --profile shared-home --snapshot latest-any
```

## Stop and restart

```bash
tmux kill-session -t qdu-weekly
tmux new-session -d -s qdu-weekly "$HOME/.local/bin/qdu-weekly-snapshot"
```

If the session disappears immediately, run the script in the foreground and inspect the log. Common causes are an incorrect qdu path, invalid profile name, missing profile, shell syntax error, or an unwritable log directory.

## Operational cautions

- The first run is immediate, not one week later.
- Scanning all of `/home`, especially across NFS mounts, can be expensive.
- qdu records only what the ordinary user can read; do not add `sudo` to this loop.
- The profile lock prevents overlapping mutations, but repeated busy exits should be fixed by changing the schedule.
- tmux does not survive a host reboot by itself. Restart the session after reboot or use a supported user service/scheduler.
- Rotate or otherwise manage the log for long-running use.

See [Operations and maintenance](OPERATIONS.md) for verification, compaction, and stable exit codes.

