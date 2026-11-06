# 運用と保守

[English](OPERATIONS.md) | 日本語

長期運用に必要な検証、修復、圧縮、ラベルとスナップショットの削除、しきい値判定、定期実行を説明します。

## 検証・修復・圧縮

#### 環境を診断する

```bash
qdu doctor
```

qduとSQLiteのバージョン、設定、状態ディレクトリ、走査ルート、ロック、インデックスと保存容量、最新スナップショットの日時、走査ルートとスナップショットの一致、`qdu schedule` による crontab の登録、空き容量を確認します。設定を読めない場合は `FAILED` の行として表示し、ほかに確かめられる項目の確認を続けます。`qdu doctor` は何も変更しません。状態ディレクトリがなければ作成できるかどうかだけを確かめ、ロックを調べるためにロックファイルを作ることもしません。

#### スナップショットを検証する

```bash
qdu verify
qdu verify --all
```

SHA-256、SQLite整合性、保存件数、スナップショットIDを確認します。

#### インデックスを修復する

```bash
qdu repair --dry-run
qdu repair
```

実在するスナップショットから `index.json` を再構築し、古いインデックスをまだ読める場合はそのラベルを引き継ぎます。壊れたスナップショットは削除せず、読み飛ばした理由を表示します。中断したコマンドが残した書きかけのファイル（作成途中の `.gz.tmp` など）は削除しますが、プロファイルの `tmp` ディレクトリにある、通常のファイルではないものは残します。

#### ロック

スナップショットやラベルを変更するコマンドと `qdu profile remove --delete-data` は、プロファイルごとにロックを取ります。別の処理が実行中なら、あとから実行したコマンドは最大 2 秒待ったうえで終了コード5で終了し、ロックを持つプロセスがエラーと `qdu doctor` に表示されます。ロックを持つプロセスが終了すると、異常終了した場合もOSがロックを解放するため、手動で削除する必要はありません。ロックを待っている間に `qdu profile remove --delete-data` などでロックファイルが削除された場合、待っていたプロセスはその時点で同じパスにあるファイルをロックします。`qdu profile remove --delete-data` は何かを変更する前にロックを取るので、ほかの処理が使っているプロファイルは元のまま残ります。

#### 保持と圧縮

qdu はスナップショットを取るたびに、ラベルのないスナップショットのうち `keep_snapshots`（既定は 52 件。週 1 回の取得なら約 1 年分）を超えた分を古いものから削除し、`compact_after`（既定は `7d`）より古いスナップショットを gzip で圧縮します。ラベルの付いたスナップショットは削除せず、`keep_snapshots` の件数にも数えません。`show` と `diff` がそのまま読めるよう、最新の 2 件は圧縮しません。圧縮したスナップショットの大きさはおよそ 7 分の 1 になり、すべてのコマンドからそのまま使えます。どちらもプロファイルごとに設定でき、1 回だけ変えることもできます。

```bash
qdu profile set data --path /srv/data --keep-snapshots 104 --compact-after 30d
qdu snapshot --compact-after off
```

別の条件で手動で圧縮するには、`qdu compact` を使います。`--dry-run` を付けると圧縮の対象を表示するだけで、ロックも取りません。

```bash
qdu compact --older-than 30d
qdu compact --older-than 30d --keep-latest 3
qdu compact --older-than 30d --dry-run
```

圧縮済みスナップショットも `show`、`diff`、`verify` などから利用できます。各スナップショットのファイルがディスク上で使う容量は `qdu list` で確認でき、保持件数や圧縮の設定を決める目安になります。

## ラベルとスナップショットの削除

ラベルはプロファイルのスナップショット1つに付ける名前です。ラベルを付けたスナップショットは `@NAME` で選べ、保持件数による削除の対象から外れます。

```bash
qdu snapshot --label before-cleanup        # 新しいスナップショットにラベルを付ける
qdu label baseline --snapshot 2026-01-31   # 既存のスナップショットにラベルを付ける
qdu diff --from @baseline
qdu label baseline --remove
```

同じラベルを別のスナップショットに付けると、ラベルはそのスナップショットへ移ります。ラベルは `qdu list` に表示されます。

`qdu forget` は指定したスナップショットとそのファイルを削除します。削除するスナップショットは、`--snapshot` と同じ書き方で指定できます（[コマンドリファレンス](COMMAND_REFERENCE_ja.md)を参照）。

```bash
qdu forget latest~5 latest~6 --dry-run
qdu forget latest~5 latest~6
```

## しきい値チェック

```bash
qdu check --growth-over 10GiB
qdu check Library/Caches --growth-over 2GiB
qdu check --disk-usage-over 85
qdu check --inode-usage-over 90
qdu check --snapshot-age-over 8d
qdu check --capacity-limit 2TiB
qdu check --capacity-limit 2TiB --capacity-user "$USER"
```

複数条件も指定できます。

```bash
qdu check \
  --growth-over 10GiB \
  --disk-usage-over 85 \
  --inode-usage-over 90
```

しきい値を超えると終了コード `10` になります。`--growth-over` には完全なスナップショットが2つ以上必要です。

`--snapshot-age-over` は、判定するスナップショット（既定では最新の完全なスナップショット）が指定した時間より古いと警告します。これで、定期取得が止まったり失敗し続けたりしていることに気づけます。

プロファイルに許容量を保存している場合、`qdu check --profile NAME` だけでも許容量を判定します。

`--disk-usage-over` と `--inode-usage-over` は、いまのファイルシステムを調べます。ディスクの使用率は `df` と同じく「使用量 ÷（使用量 + 空き容量）」で計算するので、root 用に予約された領域はどちらにも数えず、満杯のディスクは 100% になります。ほかのホストで取ったスナップショット（複数のサーバーから集めたダッシュボード用の写しなど）では、スナップショットに記録された使用量で判定します。

TSV と JSON では、各チェックの `observed` と `threshold` を数値で出力し、その単位（`bytes`、`percent`、`seconds` のいずれか）を `unit` に示します。表に表示する文字列もあわせて出力します。

## 定期実行

#### cronを使える場合

`qdu schedule` が crontab の行を作ります。何も指定しなければ行を表示するだけで、`--install` を付けると登録します。

```bash
qdu schedule --profile home                       # 毎週月曜 03:15 の行を表示する
qdu schedule --profile home --every daily --at 02:30 --install
qdu schedule --profile home --remove
```

`--weekday` は毎週スナップショットを取る曜日を指定します。毎日実行する `--every daily` と、毎月1日に実行する `--every monthly` では、`--weekday` を指定するとエラーになります。

登録する行はインストール済みの qdu を絶対パスで `--quiet` 付きで実行し、警告とエラーをプロファイルの状態ディレクトリの `snapshot.log` へ追記します。`uv tool install` や `pipx` が作る `~/.local/bin/qdu` のように、`PATH` 上のリンクから qdu を起動した場合は、そのリンクのパスを書くので、qdu を更新しても行はそのまま使えます。`XDG_CONFIG_HOME` と `XDG_STATE_HOME` を設定している場合は、その値も引き継ぎます。行はプロファイルごとに 1 つで、`# qdu:<プロファイル>` の印が付きます。もう一度登録すると前の行を置き換え、crontab のほかの行は1バイトも変えずに残します。`qdu profile remove` はそのプロファイルの行も削除します。定期実行が止まったことに気づけるよう、`qdu check --snapshot-age-over 8d` と組み合わせてください。

`qdu schedule` はインストールした qdu で実行してください。`uvx qdu` で動く qdu は uv のキャッシュにあり、uv が削除することがあるためです。`uvx` で動かした qdu で `qdu schedule` を実行すると、qdu は警告を表示します。

`--quiet`は完全に成功した場合の標準出力と、端末への進捗表示を抑えます。不完全な場合は警告を標準エラーへ出し、終了コード`3`を返します。読み取りを拒否された場所があるだけなら、警告は標準エラーへ出しますが、スナップショットは完全なままで終了コードは`0`です。

#### cronを使えない場合

tmuxセッションを動かし続けられる環境では、シェルスクリプトと`sleep`を組み合わせて定期実行できます。

`--profile`を指定しない例と、`--profile shared-home`を指定する例は、次の文書にまとめています。

[tmuxを使ってqduを1週間ごとに定期実行する](QDU_TMUX_WEEKLY_SNAPSHOT_GUIDE_ja.md)

サーバーが再起動した場合や、tmuxサーバーが終了した場合は、tmuxセッションを起動し直す必要があります。

`qdu check`を通知処理へ組み込む場合は、終了コード`10`と、それ以外のエラーを分けて扱ってください。

## 終了コード

| コード | 意味 |
|---:|---|
| `0` | 成功 |
| `1` | 一般的な実行エラー。`doctor` の確認で失敗した項目があった場合も含む |
| `2` | 引数・設定・プロファイルのエラー。`profile list` が読み取れないプロファイルを見つけた場合も含む |
| `3` | 不完全なスナップショットを保存。読み取りを拒否されただけの場合は含まない |
| `4` | スナップショット・インデックス形式エラー。スナップショットのファイルがSQLiteのスナップショットとして読めない場合も含む |
| `5` | 同じプロファイルで別のqdu処理が実行中 |
| `6` | 検証エラー |
| `10` | しきい値超過 |
| `130` | `Ctrl+C`（SIGINT）による中断。SIGTERM では `143`、SIGHUP では `129` |
