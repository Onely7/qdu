# 運用と保守

[English](OPERATIONS_en.md) | 日本語

長期運用に必要な検証、修復、圧縮、しきい値判定、定期実行を説明します。

## 検証・修復・圧縮

#### 環境を診断する

```bash
qdu doctor
```

Python・SQLite、設定、状態ディレクトリ、走査ルート、ロック、インデックス、空き容量を確認します。

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

#### 残ったロック情報を削除する

```bash
qdu unlock
```

別のqdu処理が実行中の場合は解除できません。通常、プロセス終了時にOSがロックを解放します。

#### 古いスナップショットを圧縮する

```bash
qdu compact --older-than 30d
qdu compact --older-than 30d --keep-latest 3
qdu compact --older-than 30d --dry-run
```

圧縮済みスナップショットも `show`、`diff`、`verify` などから利用できます。

## しきい値チェック

```bash
qdu check --growth-over 10GiB
qdu check --path Library/Caches --growth-over 2GiB
qdu check --disk-usage-over 85
qdu check --inode-usage-over 90
qdu check --capacity-limit 2TiB
qdu check --capacity-limit 2TiB --capacity-user daiki
```

複数条件も指定できます。

```bash
qdu check \
  --growth-over 10GiB \
  --disk-usage-over 85 \
  --inode-usage-over 90
```

しきい値を超えると終了コード `10` になります。`--growth-over` には完全なスナップショットが2つ以上必要です。プロファイルに許容量を保存している場合、`qdu check --profile NAME` だけでも許容量を判定します。

## 定期実行

#### cronを使える場合

毎日午前3時に`home`プロファイルを記録する例：

```cron
0 3 * * * "$HOME/.local/bin/qdu" snapshot --profile home --quiet
```

`--quiet`は、完全に成功した場合の標準出力だけを抑えます。不完全な場合は警告を標準エラーへ出し、終了コード`3`を返します。

#### cronを使えない場合

tmuxセッションを動かし続けられる環境では、シェルスクリプトと`sleep`を組み合わせて定期実行できます。

`--profile`を指定しない例と、`--profile shared-home`を指定する例は、次の文書にまとめています。

[tmuxを使ってqduを1週間ごとに定期実行する](QDU_TMUX_WEEKLY_SNAPSHOT_GUIDE.md)

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
