# qdu コマンドリファレンス

[English](COMMAND_REFERENCE.md) | 日本語

全コマンドと、各オプションの既定値をまとめています。基本的な使い方は、[README](../README_ja.md)または[分析ガイド](ANALYSIS_GUIDE_ja.md)を参照してください。

## コマンド一覧

| コマンド | 役割 |
|---|---|
| `qdu snapshot` | スナップショットを取得 |
| `qdu show` | 容量ランキングを表示 |
| `qdu diff` | スナップショットを比較 |
| `qdu list` | 履歴一覧を表示 |
| `qdu users` | ユーザー別ランキングを表示 |
| `qdu errors` | 読めなかった場所を表示 |
| `qdu files` | 大容量ファイルを表示 |
| `qdu inodes` | inode使用量を表示 |
| `qdu explain` | 1つのディレクトリを掘り下げる |
| `qdu browse` | `fzf` でディレクトリを選ぶ |
| `qdu check` | 容量・使用率のしきい値を判定 |
| `qdu compact` | 古いスナップショットを圧縮 |
| `qdu verify` | スナップショットを検証 |
| `qdu doctor` | 実行環境を診断 |
| `qdu repair` | インデックスを再構築 |
| `qdu unlock` | 残ったロック情報を削除 |
| `qdu profile` | プロファイルを管理 |
| `qdu config` | 設定を管理 |

全オプションは `--help` で確認できます。

```bash
qdu --help
qdu snapshot --help
qdu show --help
qdu diff --help
```

## コマンド別オプション一覧

既定値が「プロファイル設定」となっている項目は、コマンドラインで指定しなければ設定ファイルの値が使われます。設定ファイルにも値がなければ、表に示した初期値が使われます。

#### qdu全体のオプション

| オプション | 既定値 | 説明 |
|---|---|---|
| `--version` | なし | qduのバージョンを表示して終了 |
| `-h, --help` | なし | qdu全体のヘルプを表示して終了 |

#### 共通オプション

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `--format FORMAT` | `table` | `table`、`tsv`、`json` から出力形式を選ぶ |
| `--style STYLE` | `auto` | `auto`、`rich`、`plain` から表の形式を選ぶ |
| `--color MODE` | `auto` | `auto`、`always`、`never` から色表示を選ぶ |
| `-h, --help` | なし | コマンドのヘルプを表示して終了 |

#### 許容量に関する共通オプション

`show`、`check`、`profile add` で使用します。

| オプション | 既定値 | 説明 |
|---|---|---|
| `--capacity-limit SIZE` | プロファイル設定。初期値は未設定 | 運用上の許容量を指定する。例：`2TB`、`2TiB` |
| `--no-capacity-limit` | 無効 | 保存済みの許容量を今回だけ無視する。`profile add` では設定を削除する |
| `--capacity-user USER` | プロファイル設定。初期値は未指定 | 指定ユーザーが所有するデータだけを許容量と比較する |
| `--capacity-profile` | 無効 | 保存済みのユーザー指定を無視し、プロファイル全体を比較対象にする |

<a id="qdu-snapshot"></a>

<details>
<summary><code>qdu snapshot</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `-p, --path PATH` | プロファイル設定。初期値は `$HOME` | 走査ルートを今回だけ指定する |
| `--exclude PATTERN` | プロファイル設定 | 除外パターンを追加する。複数回指定可能 |
| `--exclude-from FILE` | なし | ファイルから除外パターンを読み込む |
| `--keep-snapshots N` | プロファイル設定。初期値は `100` | 保持するスナップショット数。`0` は無制限 |
| `--with-users` | プロファイル設定。初期値は無効 | ユーザー別統計を保存する |
| `--without-users` | 無効 | ユーザー別統計を今回だけ無効にする |
| `--user-max-depth N` | プロファイル設定。初期値は `3` | ユーザー別ディレクトリ集計の深さ |
| `--file-top N` | プロファイル設定。初期値は `1000` | 保存する大容量ファイル候補数 |
| `--record-max-depth N` | 制限なし | スナップショットへ保存するディレクトリ深さ |
| `--cross-filesystems` | プロファイル設定。初期値は無効 | 別ファイルシステムのマウント先にも入る |
| `--one-file-system` | 有効 | 走査ルートと同じファイルシステム内だけを走査する |
| `--quiet` | 無効 | 完全に成功した場合の標準出力を抑える |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

</details>

<details>
<summary><code>qdu show</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `--snapshot SELECTOR` | `latest` | 表示するスナップショット |
| `-L, --max-depth N` | `1` | 表示する相対深さ |
| `-n, --top N` | `20` | 表示件数 |
| `--under PATH` | `.` | 表示範囲の起点 |
| `--min-size SIZE` | `0` | 指定容量以上だけを表示 |
| `--match PATTERN` | なし | パス名を絞り込む |
| `--metric METRIC` | `allocated_bytes` | `allocated_bytes` または `apparent_bytes` |
| `--warn-age DURATION` | なし | スナップショットが古い場合の警告時間 |
| `--stale` | 無効 | 更新されていない期間を表示・色分けする |
| `--stale-only DAYS` | なし | 指定日数以上更新されていない項目だけを表示 |
| `--stale-thresholds D1,D2,D3,D4` | `30,90,180,365` | 色分けする日数 |
| `--capacity-limit SIZE` | プロファイル設定。初期値は未設定 | 運用上の許容量 |
| `--no-capacity-limit` | 無効 | 許容量表示を今回だけ無効にする |
| `--capacity-user USER` | プロファイル設定。初期値は未指定 | 指定ユーザーの使用量を許容量と比較する |
| `--capacity-profile` | 無効 | プロファイル全体を比較対象にする |
| `--users` | 無効 | 容量ランキングに続けてユーザー別統計を表示 |
| `--user-top N` | `10` | 表示するユーザー数 |
| `--user-dir-top N` | `5` | ユーザーごとに表示する上位ディレクトリ数 |
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

</details>

<details>
<summary><code>qdu diff</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `--from SELECTOR` | `latest` の直前 | 比較元のスナップショット |
| `--to SELECTOR` | `latest` | 比較先のスナップショット |
| `--previous` | 無効 | 直前と最新の完全なスナップショットを比較 |
| `-L, --max-depth N` | `1` | 表示する相対深さ |
| `-n, --top N` | `20` | 表示件数 |
| `--under PATH` | `.` | 比較範囲の起点 |
| `--min-size SIZE` | `0` | 現在容量が指定値以上の項目だけを表示 |
| `--match PATTERN` | なし | パス名を絞り込む |
| `--growth-only` | 無効 | 増加した項目だけを表示 |
| `--shrink-only` | 無効 | 減少した項目だけを表示 |
| `--order ORDER` | `growth` | `growth`、`shrink`、`absolute`、`current` |
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

</details>

<details>
<summary><code>qdu list</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `-n, --top N` | `20` | 表示件数 |
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

</details>

<details>
<summary><code>qdu users</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `--snapshot SELECTOR` | `latest` | 使用するスナップショット |
| `-n, --top N` | `10` | 表示するユーザー数 |
| `--dirs N` | `5` | ユーザーごとの上位ディレクトリ数 |
| `--metric METRIC` | `allocated_bytes` | `allocated_bytes` または `inode_count` |
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

</details>

<details>
<summary><code>qdu errors</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `--snapshot SELECTOR` | `latest-any` | エラーを確認するスナップショット |
| `-n, --top N` | `100` | 表示件数 |
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

</details>

<details>
<summary><code>qdu files</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `--snapshot SELECTOR` | `latest` | 使用するスナップショット |
| `--live` | 無効 | 現在のファイルシステムを再走査する |
| `--path PATH` | 選択したスナップショットのルート | `--live` の走査ルート |
| `-n, --top N` | `30` | 表示件数 |
| `--under PATH` | `.` | 検索範囲の起点 |
| `--min-size SIZE` | `0` | 指定容量以上のファイルだけを表示 |
| `--user USER` | なし | 所有ユーザーで絞り込む |
| `--stale` | 無効 | 更新されていない期間を表示・色分けする |
| `--stale-only DAYS` | なし | 指定日数以上更新されていないファイルだけを表示 |
| `--stale-thresholds D1,D2,D3,D4` | `30,90,180,365` | 色分けする日数 |
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

</details>

<details>
<summary><code>qdu inodes</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `--snapshot SELECTOR` | `latest` | 使用するスナップショット |
| `-L, --max-depth N` | `1` | 表示する相対深さ |
| `-n, --top N` | `20` | 表示件数 |
| `--under PATH` | `.` | 表示範囲の起点 |
| `--match PATTERN` | なし | パス名を絞り込む |
| `--users` | 無効 | ユーザー別inodeランキングを表示 |
| `--dirs N` | `5` | ユーザーごとの上位ディレクトリ数 |
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

</details>

<details>
<summary><code>qdu explain</code></summary>

位置引数 `PATH` は必須です。

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `--snapshot SELECTOR` | `latest` | 使用するスナップショット |
| `--diff` | 無効 | 指定ディレクトリ直下の差分を表示 |
| `--from SELECTOR` | `latest` の直前 | 差分の比較元 |
| `--to SELECTOR` | `latest` | 差分の比較先 |
| `--growth-only` | 無効 | 増加した項目だけを表示 |
| `--shrink-only` | 無効 | 減少した項目だけを表示 |
| `--order ORDER` | `growth` | 差分の並び順 |
| `-n, --top N` | `20` | 表示件数 |
| `--min-size SIZE` | `0` | 指定容量以上だけを表示 |
| `--match PATTERN` | なし | パス名を絞り込む |
| `--metric METRIC` | `allocated_bytes` | 容量の基準 |
| `--warn-age DURATION` | なし | 古いスナップショットへ警告 |
| `--stale` | 無効 | 更新されていない期間を表示 |
| `--stale-only DAYS` | なし | 更新されていない日数で絞り込む |
| `--stale-thresholds D1,D2,D3,D4` | `30,90,180,365` | 色分けする日数 |
| `--users` | 無効 | ユーザー別統計も表示 |
| `--user-top N` | `10` | 表示するユーザー数 |
| `--user-dir-top N` | `5` | ユーザーごとのディレクトリ数 |
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

</details>

<details>
<summary><code>qdu browse</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `--snapshot SELECTOR` | `latest` | 使用するスナップショット |
| `-L, --max-depth N` | `1` | 選択後に表示する深さ |
| `-n, --top N` | `20` | 選択後の表示件数 |
| `--min-size SIZE` | `0` | 指定容量以上だけを表示 |
| `--match PATTERN` | なし | パス名を絞り込む |
| `--metric METRIC` | `allocated_bytes` | 容量の基準 |
| `--warn-age DURATION` | なし | 古いスナップショットへ警告 |
| `--stale` | 無効 | 更新されていない期間を表示 |
| `--stale-only DAYS` | なし | 更新されていない日数で絞り込む |
| `--stale-thresholds D1,D2,D3,D4` | `30,90,180,365` | 色分けする日数 |
| `--users` | 無効 | ユーザー別統計も表示 |
| `--user-top N` | `10` | 表示するユーザー数 |
| `--user-dir-top N` | `5` | ユーザーごとのディレクトリ数 |
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

</details>

<details>
<summary><code>qdu check</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `--snapshot SELECTOR` | `latest` | 判定に使うスナップショット |
| `--path PATH` | `.` | 増加量を判定するパス |
| `--growth-over SIZE` | 未指定 | 許容する増加量 |
| `--disk-usage-over PERCENT` | 未指定 | 現在のファイルシステム使用率の上限 |
| `--inode-usage-over PERCENT` | 未指定 | 現在のinode使用率の上限 |
| `--capacity-limit SIZE` | プロファイル設定。初期値は未設定 | 運用上の許容量 |
| `--no-capacity-limit` | 無効 | 保存済みの許容量を今回だけ無視する |
| `--capacity-user USER` | プロファイル設定。初期値は未指定 | 指定ユーザーの使用量を判定する |
| `--capacity-profile` | 無効 | プロファイル全体を判定する |
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

少なくとも1つの判定条件が必要です。ただし、プロファイルに許容量を保存している場合は、追加オプションなしでも許容量を判定します。

</details>

<details>
<summary><code>qdu compact</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `--older-than DURATION` | `30d` | この期間より古いスナップショットを圧縮 |
| `--keep-latest N` | `2` | 新しいスナップショットを非圧縮で残す件数 |
| `--dry-run` | 無効 | 実際には変更せず対象だけを表示 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

</details>

<details>
<summary><code>qdu verify</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `--snapshot SELECTOR` | `latest` | 検証するスナップショット |
| `--all` | 無効 | 全スナップショットを検証 |
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

</details>

<details>
<summary><code>qdu doctor</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 診断するプロファイル |
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

</details>

<details>
<summary><code>qdu repair</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 修復するプロファイル |
| `--dry-run` | 無効 | 実際には変更せず処理内容を表示 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

</details>

<details>
<summary><code>qdu unlock</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 対象プロファイル |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

</details>

<details>
<summary><code>qdu profile</code></summary>

##### `qdu profile list`

| オプション | 既定値 | 説明 |
|---|---|---|
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

##### `qdu profile show NAME`

| オプション | 既定値 | 説明 |
|---|---|---|
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

##### `qdu profile add NAME`

| オプション | 既定値 | 説明 |
|---|---|---|
| `--path PATH` | 必須 | 走査ルート |
| `--exclude PATTERN` | なし | 除外パターン。複数回指定可能 |
| `--keep-snapshots N` | `100` | 保持件数。`0` は無制限 |
| `--with-users` | 無効 | ユーザー別統計を保存 |
| `--user-max-depth N` | `3` | ユーザー別ディレクトリ集計の深さ |
| `--file-top N` | `1000` | 保存する大容量ファイル候補数 |
| `--record-max-depth N` | 制限なし | 保存するディレクトリ深さ |
| `--capacity-limit SIZE` | 既存設定を維持。新規は未設定 | 運用上の許容量 |
| `--no-capacity-limit` | 無効 | 許容量設定を削除 |
| `--capacity-user USER` | 既存設定を維持。新規は未指定 | 指定ユーザーを許容量の対象にする |
| `--capacity-profile` | 無効（既存設定を維持） | プロファイル全体を対象にする |
| `--cross-filesystems` | 無効 | 別ファイルシステムにも入る |
| `--one-file-system` | 有効 | 同じファイルシステム内だけを走査 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

##### `qdu profile remove NAME`

| オプション | 既定値 | 説明 |
|---|---|---|
| `--delete-data` | 無効 | 設定とともにスナップショットも削除 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

</details>

<details>
<summary><code>qdu config</code></summary>

##### `qdu config path`

| オプション | 既定値 | 説明 |
|---|---|---|
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

##### `qdu config init`

| オプション | 既定値 | 説明 |
|---|---|---|
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

##### `qdu config show`

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 表示するプロファイル |
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

</details>
