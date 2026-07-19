# tmux を使って qdu を1週間ごとに定期実行する

cron や systemd timer を使えない環境でも、`tmux` のセッションを動かし続けられるなら、`qdu snapshot` を1週間ごとに実行できます。

この方法では、次の流れを繰り返します。

1. `qdu snapshot` を実行する
2. 実行結果をログへ保存する
3. 7日間待機する
4. 再び `qdu snapshot` を実行する

SSH接続やターミナルを閉じても、tmuxセッションが残っている限り処理は続きます。

> [!IMPORTANT]
> サーバーが再起動した場合や、tmuxサーバー自体が終了した場合は停止します。その場合は、tmuxセッションをもう一度起動してください。

---

## 前提

この手順では、次のコマンドを1週間ごとに実行します。

```bash
qdu snapshot \
  --profile shared-home \
  --with-users \
  --user-max-depth 4 \
  --file-top 2000
```

あらかじめ、次を確認してください。

- `qdu` が `~/.local/bin/qdu` にインストールされている
- `tmux` が利用できる
- `shared-home` プロファイルが作成済み
- `/home` のような別マウントを含む場所を走査する場合は、プロファイルで `cross_filesystems` が有効になっている

確認コマンド：

```bash
"$HOME/.local/bin/qdu" --version
tmux -V
"$HOME/.local/bin/qdu" profile show shared-home
```

`shared-home` が `/home` を対象としており、各ユーザーのホームがNFSなどの別マウントになっている場合は、次のようにプロファイルを設定します。

```bash
"$HOME/.local/bin/qdu" profile add shared-home \
  --path /home \
  --cross-filesystems \
  --with-users \
  --user-max-depth 4 \
  --file-top 2000
```

---

## 1. 定期実行スクリプトを作る

まず、スクリプトとログを置くディレクトリを作ります。

```bash
mkdir -p "$HOME/.local/bin"
mkdir -p "${XDG_STATE_HOME:-$HOME/.local/state}/qdu/logs"
```

次のコマンドを実行し、`qdu-weekly-snapshot` というスクリプトを作成します。

```bash
cat > "$HOME/.local/bin/qdu-weekly-snapshot" <<'EOF'
#!/usr/bin/env bash

set -u

QDU="${QDU:-$HOME/.local/bin/qdu}"
PROFILE="shared-home"

# 7日を秒へ換算する。
INTERVAL_SECONDS=$((7 * 24 * 60 * 60))

STATE_HOME="${XDG_STATE_HOME:-$HOME/.local/state}"
LOG_DIR="$STATE_HOME/qdu/logs"
LOG_FILE="$LOG_DIR/${PROFILE}-weekly.log"

mkdir -p "$LOG_DIR"

format_epoch() {
  local epoch="$1"

  # GNU date（主にLinux）
  if date -d "@$epoch" '+%Y-%m-%d %H:%M:%S %z' >/dev/null 2>&1; then
    date -d "@$epoch" '+%Y-%m-%d %H:%M:%S %z'
    return
  fi

  # BSD date（主にmacOS）
  date -r "$epoch" '+%Y-%m-%d %H:%M:%S %z'
}

if [[ ! -x "$QDU" ]]; then
  printf 'Error: qdu executable was not found or is not executable: %s\n' "$QDU" >&2
  exit 1
fi

# スクリプト起動直後に、最初のスナップショットを取得する。
next_run="$(date +%s)"

while true; do
  started_at="$(date '+%Y-%m-%d %H:%M:%S %z')"

  {
    printf '\n============================================================\n'
    printf 'qdu weekly snapshot started: %s\n' "$started_at"
    printf 'Profile: %s\n' "$PROFILE"
    printf '============================================================\n'

    "$QDU" snapshot \
      --profile "$PROFILE" \
      --with-users \
      --user-max-depth 4 \
      --file-top 2000

    exit_status=$?

    case "$exit_status" in
      0)
        printf 'Result: complete snapshot\n'
        ;;
      3)
        printf 'Result: incomplete snapshot\n'
        printf 'Some paths could not be read. Check them with:\n'
        printf '  qdu errors --profile %s --snapshot latest-any\n' "$PROFILE"
        ;;
      *)
        printf 'Result: qdu failed with exit status %d\n' "$exit_status"
        ;;
    esac

    printf 'Finished: %s\n' "$(date '+%Y-%m-%d %H:%M:%S %z')"
  } >>"$LOG_FILE" 2>&1

  # 前回の予定時刻を基準に7日進める。
  # qduの実行時間が長くても、実行時刻が少しずつずれるのを防ぐ。
  next_run=$((next_run + INTERVAL_SECONDS))
  now="$(date +%s)"

  # サーバー停止などで予定時刻を過ぎていた場合は、
  # 過去の回数分を連続実行せず、次の未来の時刻まで進める。
  while ((next_run <= now)); do
    next_run=$((next_run + INTERVAL_SECONDS))
  done

  sleep_seconds=$((next_run - now))

  {
    printf 'Next run: %s\n' "$(format_epoch "$next_run")"
    printf 'Sleeping for %d seconds.\n' "$sleep_seconds"
  } >>"$LOG_FILE"

  sleep "$sleep_seconds"
done
EOF
```

---

## 2. 実行権限を付ける

作成した直後のファイルには、実行権限が付いていないことがあります。

そのままtmuxから起動すると、スクリプトが即座に終了し、tmuxセッションも消えてしまいます。

実行権限を付けてください。

```bash
chmod +x "$HOME/.local/bin/qdu-weekly-snapshot"
```

確認：

```bash
ls -l "$HOME/.local/bin/qdu-weekly-snapshot"
```

次のように、権限表示に `x` が含まれていれば実行できます。

```text
-rwxr-xr-x ... /home/USER/.local/bin/qdu-weekly-snapshot
```

`-rw-r--r--` のように `x` がない場合は、まだ直接実行できません。

---

## 3. 起動前に動作を確認する

### シェルスクリプトの構文を確認する

```bash
bash -n "$HOME/.local/bin/qdu-weekly-snapshot"
```

何も表示されなければ、構文上の問題はありません。

### 前面で試しに実行する

```bash
bash -x "$HOME/.local/bin/qdu-weekly-snapshot"
```

正常なら、次の順で動きます。

1. `qdu snapshot` が実行される
2. ログが保存される
3. `sleep 604800` に入る

`604800`秒は7日です。

動作確認が終わったら、`Ctrl+C` で停止してください。

> [!NOTE]
> `bash -x` は、実行したコマンドを画面に表示するデバッグ用の起動方法です。通常運用では使いません。

---

## 4. tmuxセッションで起動する

既に同名のセッションがある場合は重複起動しないようにします。

```bash
if tmux has-session -t qdu-weekly 2>/dev/null; then
  echo "tmux session 'qdu-weekly' is already running."
else
  tmux new-session \
    -d \
    -s qdu-weekly \
    "exec bash '$HOME/.local/bin/qdu-weekly-snapshot'"
fi
```

ここで作られるのは、`qdu-weekly` という名前の**tmuxセッション**です。

既存セッション内の新しいwindowを作るコマンドではありません。

---

## 5. 起動できたことを確認する

```bash
tmux has-session -t qdu-weekly 2>/dev/null \
  && echo "qdu-weekly is running." \
  || echo "qdu-weekly is not running."
```

tmuxセッションの一覧も確認できます。

```bash
tmux ls
```

正常なら、次のような行が表示されます。

```text
qdu-weekly: 1 windows (created ...)
```

待機中のプロセスを確認する場合：

```bash
tmux list-panes \
  -t qdu-weekly \
  -F 'pid=#{pane_pid} command=#{pane_current_command} dead=#{pane_dead}'
```

7日間の待機中は、`command=sleep` と表示されても正常です。

---

## 6. ログを確認する

ログの保存先は次です。

```text
${XDG_STATE_HOME:-$HOME/.local/state}/qdu/logs/shared-home-weekly.log
```

最後の100行を表示：

```bash
tail -n 100 \
  "${XDG_STATE_HOME:-$HOME/.local/state}/qdu/logs/shared-home-weekly.log"
```

追記される様子をリアルタイムで確認：

```bash
tail -f \
  "${XDG_STATE_HOME:-$HOME/.local/state}/qdu/logs/shared-home-weekly.log"
```

`tail -f` を終了するには `Ctrl+C` を押します。tmux内の定期実行は停止しません。

ログには、次の情報が記録されます。

- 実行開始時刻
- 使用したプロファイル
- qduの実行結果
- 完全または不完全なスナップショットの判定
- 終了コード
- 次回実行予定時刻

---

## 7. qduの結果を確認する

スナップショット一覧：

```bash
qdu list --profile shared-home --top 10
```

容量ランキング：

```bash
qdu show --profile shared-home
```

ユーザーごとの容量と上位ディレクトリ：

```bash
qdu users \
  --profile shared-home \
  --top 20 \
  --dirs 10
```

読み取れない場所があり、不完全なスナップショットになった場合：

```bash
qdu errors \
  --profile shared-home \
  --snapshot latest-any \
  --top 100
```

不完全なスナップショットは、最後に成功した完全なスナップショットを置き換えません。

---

## 8. 停止・再起動する

### 停止する

```bash
tmux kill-session -t qdu-weekly
```

### 再起動する

```bash
tmux new-session \
  -d \
  -s qdu-weekly \
  "exec bash '$HOME/.local/bin/qdu-weekly-snapshot'"
```

スクリプトを書き換えた場合は、一度tmuxセッションを停止してから再起動してください。

---

## 9. tmux画面へ接続する

tmuxセッションへ接続：

```bash
tmux attach-session -t qdu-weekly
```

スクリプトはログファイルへ出力しているため、tmux画面にはほとんど何も表示されません。

tmuxセッションを停止せずに画面から離れるには、次の順でキーを押します。

1. `Ctrl+B`
2. `D`

これは**デタッチ**と呼ばれます。デタッチ後も処理は続きます。

---

## 10. 起動直後にtmuxセッションが消える場合

tmuxセッション内で起動したコマンドが終了すると、そのセッションも自動的に消えます。

まず、スクリプトの権限を確認してください。

```bash
ls -l "$HOME/.local/bin/qdu-weekly-snapshot"
```

実行権限がなければ付与します。

```bash
chmod +x "$HOME/.local/bin/qdu-weekly-snapshot"
```

次に、前面で実行してエラーを確認します。

```bash
bash -x "$HOME/.local/bin/qdu-weekly-snapshot"
```

エラーになってもtmuxセッションを残したい場合は、デバッグ用セッションを使います。

```bash
tmux new-session \
  -d \
  -s qdu-weekly-debug \
  "bash -x '$HOME/.local/bin/qdu-weekly-snapshot'; status=\$?; echo; echo 'Script exited with status:' \$status; exec bash"
```

接続：

```bash
tmux attach-session -t qdu-weekly-debug
```

確認後に削除：

```bash
tmux kill-session -t qdu-weekly-debug
```

---

## 11. サーバー再起動後の扱い

SSH接続を切っただけなら、tmuxセッションは残ります。

しかし、次の場合は定期実行も停止します。

- サーバーが再起動した
- tmuxサーバーが終了した
- `qdu-weekly` セッションを削除した
- 常駐スクリプトが異常終了した

サーバー再起動後は、次のコマンドでもう一度起動してください。

```bash
tmux new-session \
  -d \
  -s qdu-weekly \
  "exec bash '$HOME/.local/bin/qdu-weekly-snapshot'"
```

重複起動を避ける場合：

```bash
if tmux has-session -t qdu-weekly 2>/dev/null; then
  echo "qdu-weekly is already running."
else
  tmux new-session \
    -d \
    -s qdu-weekly \
    "exec bash '$HOME/.local/bin/qdu-weekly-snapshot'"
fi
```

---

## 注意点

### 最初の実行はすぐに始まる

このスクリプトは、tmuxセッションを起動すると直ちに最初のスナップショットを取得します。

その後、7日ごとに繰り返します。

### `/home` 全体の走査には時間がかかる

NFS上の多数のホームディレクトリを走査する場合、完了まで長時間かかることがあります。

NFSサーバーの応答が遅いと、処理が止まったように見える場合もあります。

### 一般ユーザーが読めない場所は取得できない

`--cross-filesystems` を有効にしても、アクセス権限が増えるわけではありません。

読み取れない場所がある場合、qduは読めた範囲を不完全なスナップショットとして保存し、終了コード`3`を返します。

### sudoは使わない

sudoで実行すると、設定やスナップショットがroot所有になり、普段のユーザーから扱いにくくなる場合があります。

一般ユーザーのまま実行してください。

---

## よく使うコマンド

```bash
# 起動状態を確認
tmux has-session -t qdu-weekly && echo "running"

# tmuxセッション一覧
tmux ls

# ログを確認
tail -n 100 \
  "${XDG_STATE_HOME:-$HOME/.local/state}/qdu/logs/shared-home-weekly.log"

# スナップショット一覧
qdu list --profile shared-home --top 10

# ユーザー別ランキング
qdu users --profile shared-home --top 20 --dirs 10

# 停止
tmux kill-session -t qdu-weekly

# 再起動
tmux new-session \
  -d \
  -s qdu-weekly \
  "exec bash '$HOME/.local/bin/qdu-weekly-snapshot'"
```
