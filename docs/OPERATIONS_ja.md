# 運用と保守

[English](OPERATIONS.md) | 日本語

長期運用に必要な検証、修復、圧縮、しきい値判定、定期実行を説明します。

## 検証・修復・圧縮

#### 環境を診断する

```bash
qdu doctor
```

Python・SQLite、設定、状態ディレクトリ、走査ルート、ロック、インデックスと保存容量、最新スナップショットの日時、走査ルートとスナップショットの一致、`qdu schedule` による crontab の登録、空き容量を確認します。

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

実在するスナップショットから `index.json` を再構築します。壊れたスナップショットは削除せず、読み飛ばした理由を表示します。

#### ロック

スナップショットを変更するコマンドは、プロファイルごとにロックを取ります。別の処理が実行中なら、あとから実行したコマンドは終了コード5で終了し、ロックを持つプロセスがエラーと `qdu doctor` に表示されます。ロックを持つプロセスが終了すると、異常終了した場合もOSがロックを解放するため、手動で削除する必要はありません。

#### 保持と圧縮

qdu はスナップショットを取るたびに、`keep_snapshots`（既定は 52 件。週 1 回の取得なら約 1 年分）を超える古いスナップショットを削除し、`compact_after`（既定は `7d`）より古いスナップショットを gzip で圧縮します。`show` と `diff` がそのまま読めるよう、最新の 2 件は圧縮しません。圧縮したスナップショットの大きさはおよそ 7 分の 1 になり、すべてのコマンドからそのまま使えます。どちらもプロファイルごとに設定でき、1 回だけ変えることもできます。

```bash
qdu profile add data --path /srv/data --keep-snapshots 104 --compact-after 30d
qdu snapshot --compact-after off
```

別の条件で手動で圧縮するには、`qdu compact` を使います。

```bash
qdu compact --older-than 30d
qdu compact --older-than 30d --keep-latest 3
qdu compact --older-than 30d --dry-run
```

圧縮済みスナップショットも `show`、`diff`、`verify` などから利用できます。各スナップショットのファイルがディスク上で使う容量は `qdu list` で確認でき、保持件数や圧縮の設定を決める目安になります。

## しきい値チェック

```bash
qdu check --growth-over 10GiB
qdu check --under Library/Caches --growth-over 2GiB
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

## 定期実行

#### cronを使える場合

`qdu schedule` が crontab の行を作ります。何も指定しなければ行を表示するだけで、`--install` を付けると登録します。

```bash
qdu schedule --profile home                       # 毎週月曜 03:15 の行を表示する
qdu schedule --profile home --every daily --at 02:30 --install
qdu schedule --profile home --remove
```

登録する行はインストール済みの qdu を絶対パスで `--quiet` 付きで実行し、警告とエラーをプロファイルの状態ディレクトリの `snapshot.log` へ追記します。`XDG_CONFIG_HOME` と `XDG_STATE_HOME` を設定している場合は、その値も引き継ぎます。行はプロファイルごとに 1 つで、`# qdu:<プロファイル>` の印が付きます。もう一度登録すると前の行を置き換え、crontab のほかの行には触れません。定期実行が止まったことに気づけるよう、`qdu check --snapshot-age-over 8d` と組み合わせてください。

`--quiet`は、完全に成功した場合の標準出力と、端末への進捗表示を抑えます。不完全な場合は警告を標準エラーへ出し、終了コード`3`を返します。

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
| `1` | 一般的な実行エラー |
| `2` | 引数・設定エラー |
| `3` | 不完全なスナップショットを保存 |
| `4` | スナップショット・インデックス形式エラー |
| `5` | 同じプロファイルで別のqdu処理が実行中 |
| `6` | 検証エラー |
| `10` | しきい値超過 |
| `130` | `Ctrl+C` による中断 |
