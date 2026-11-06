# qdu コマンドリファレンス

[English](COMMAND_REFERENCE.md) | 日本語

全コマンドと、各オプションの既定値をまとめています。基本的な使い方は[README](../README_ja.md)または[分析ガイド](ANALYSIS_GUIDE_ja.md)を参照してください。

## コマンド一覧

| コマンド | 役割 |
|---|---|
| `qdu snapshot` | スナップショットを取得 |
| `qdu show` | 容量ランキングを表示 |
| `qdu diff` | スナップショットを比較 |
| `qdu history` | 容量の推移と、満杯になる時期の予測を表示 |
| `qdu list` | 履歴一覧を表示 |
| `qdu users [PATH]` | ユーザー別ランキングを表示 |
| `qdu errors` | 読めなかった場所を表示 |
| `qdu files` | 大容量ファイルを表示 |
| `qdu browse` | `fzf` でディレクトリを選ぶ |
| `qdu check` | 容量・使用率のしきい値を判定 |
| `qdu schedule` | cronで定期的にスナップショットを取得 |
| `qdu compact` | 古いスナップショットを圧縮 |
| `qdu verify` | スナップショットを検証 |
| `qdu doctor` | 実行環境を診断 |
| `qdu repair` | インデックスを再構築し、書きかけのファイルを削除 |
| `qdu label` | スナップショットにラベルを付ける |
| `qdu forget` | スナップショットを削除 |
| `qdu profile` | プロファイルを管理 |
| `qdu config` | 設定ファイルのパスを表示し、ひな形を作成 |
| `qdu completions` | シェルの補完スクリプトを出力 |

コマンドを付けずに実行すると、指定したオプションで `qdu show` を実行します。例えば `qdu -L 2` は `qdu show -L 2` と同じです。

全オプションは `--help` で確認できます。

```bash
qdu --help
qdu snapshot --help
qdu show --help
qdu diff --help
```

## コマンド別オプション一覧

既定値が「プロファイル設定」となっている項目は、コマンドラインで指定しなければ設定ファイルの値が使われます。設定ファイルにも値がなければ、表に示した初期値が使われます。

サイズには `GB` などの10進単位と `GiB` などの2進単位を使えます。`du` と同じく接頭辞だけの書き方は2進単位で、`2T` は `2TiB` と同じです。期間は数値と `s`、`m`、`h`、`d`、`w` のいずれかを組み合わせて、`90d` のように指定します。

位置引数の `PATH` には走査ルートからの相対パスか、走査ルートの中を指す絶対パスを指定します。省略すると走査ルート全体が対象になります。スナップショットの指定方法は[スナップショットの指定](#snapshot-selectors)、TSV と JSON の構成は[出力形式](#output-formats)にまとめています。

#### qdu全体のオプション

| オプション | 既定値 | 説明 |
|---|---|---|
| `-V, --version` | なし | qduのバージョンを表示して終了 |
| `-h, --help` | なし | qdu全体のヘルプを表示して終了 |

#### 共通オプション

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル。空の名前は `default` を表す |
| `--format FORMAT` | `table` | `table`、`tsv`、`json` から出力形式を選ぶ |
| `--style STYLE` | `auto` | `auto`、`rich`、`plain` から表の形式を選ぶ |
| `--color MODE` | `auto` | `auto`、`always`、`never` から色表示を選ぶ。`auto` は、環境変数 `NO_COLOR` を設定していなければ、標準出力が端末のときは出力に、標準エラーが端末のときは警告に色を付ける |
| `-h, --help` | なし | コマンドのヘルプを表示して終了 |

設定ファイルにもスナップショットにもないプロファイルを `--profile` で指定すると、`qdu snapshot` 以外のコマンドはエラーになります。名前が近いプロファイルがあれば、その名前も示します。

#### 許容量に関する共通オプション

`show`、`check`、`profile set` で使用します。

| オプション | 既定値 | 説明 |
|---|---|---|
| `--capacity-limit SIZE` | プロファイル設定。初期値は未設定 | 運用上の許容量を指定する。例：`2TB`、`2TiB` |
| `--no-capacity-limit` | 無効 | 保存済みの許容量を今回だけ無視する。`profile set` では設定を削除する |
| `--capacity-user USER` | プロファイル設定。初期値は未指定 | 指定ユーザーが所有するデータだけを許容量と比較する |
| `--capacity-profile` | 無効 | 保存済みのユーザー指定を無視し、プロファイル全体を比較対象にする |

<a id="qdu-snapshot"></a>

<details>
<summary><code>qdu snapshot</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `-p, --path PATH` | プロファイル設定。未設定なら既存のスナップショットのルート、それもなければ `$HOME` | 走査ルートを指定する |
| `--exclude PATTERN` | プロファイル設定 | 除外パターンを追加する。複数回指定可能 |
| `--exclude-from FILE` | なし | ファイルから除外パターンを読み込む。複数回指定可能 |
| `--save` | 無効 | 今回指定した走査ルート、除外パターン、プロファイル設定を、以降のスナップショットのために保存する |
| `--label NAME` | なし | 取得したスナップショットにラベルを付ける。複数回指定可能 |
| `--keep-snapshots N` | プロファイル設定。初期値は `52` | 保持する、ラベルのないスナップショットの数。古いものから削除する。`0` は無制限 |
| `--compact-after DURATION` | プロファイル設定。初期値は `7d` | 取得後、これより古いスナップショットを圧縮する。最新の 2 件は除く。`off` で無効 |
| `--with-users` | プロファイル設定。初期値は無効 | ユーザー別統計を保存する |
| `--without-users` | 無効 | ユーザー別統計を保存しない |
| `--user-max-depth N` | プロファイル設定。初期値は `3` | ユーザー別ディレクトリ集計の深さ |
| `--file-top N` | プロファイル設定。初期値は `1000` | 保存する大容量ファイル候補数 |
| `--record-max-depth N` | プロファイル設定。初期値は制限なし | スナップショットへ保存するディレクトリ深さ |
| `--cross-filesystems` | プロファイル設定。初期値は無効 | 別ファイルシステムのマウント先にも入る |
| `--one-file-system` | 有効 | 走査ルートと同じファイルシステム内だけを走査する |
| `--quiet` | 無効 | 完全に成功した場合の標準出力と、端末への進捗表示を抑える |
| `--threads N` | `$QDU_THREADS`、未設定なら CPU の数（上限 16） | 走査に使うスレッド数 |
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

設定ファイルのセクションもスナップショットもない新しいプロファイルでは、最初のスナップショットを取得したときに、指定した走査ルート、除外パターン、プロファイル設定が保存されます。それ以降は `--save` を付けたときだけ保存されます。`--format json` では、新しいスナップショットの記録、走査エラー、削除・圧縮したスナップショット、保存した設定を出力します。

</details>

<details>
<summary><code>qdu show [PATH]</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `--snapshot SELECTOR` | `latest` | 表示するスナップショット |
| `-L, --max-depth N` | `1` | 表示する相対深さ |
| `-n, --top N` | `20` | 表示件数 |
| `PATH`（位置引数） | 走査ルート | 表示範囲の起点 |
| `--min-size SIZE` | `0` | 指定容量以上だけを表示。`--metric inodes` では inode 数 |
| `--match PATTERN` | なし | パス名を絞り込む |
| `--metric METRIC` | `allocated` | `allocated`（割り当て容量）、`apparent`（見かけのサイズ）、`inodes`（inode 数） |
| `--owners` | 無効 | 各ディレクトリで最も多く使っている所有者と、その割合を表示する。`--with-users` で取ったスナップショットの `user_max_depth` までの深さが対象 |
| `--stale` | 無効 | 更新されていない期間を表示・色分けする |
| `--stale-only DAYS` | なし | 指定日数以上更新されていない項目だけを表示。`90` のような日数か、`90d`、`26w` のような1日の整数倍の期間で指定する |
| `--stale-thresholds D1,D2,D3,D4` | `30,90,180,365` | 色分けする日数 |
| `--capacity-limit SIZE` | プロファイル設定。初期値は未設定 | 運用上の許容量 |
| `--no-capacity-limit` | 無効 | 許容量表示を今回だけ無効にする |
| `--capacity-user USER` | プロファイル設定。初期値は未指定 | 指定ユーザーの使用量を許容量と比較する |
| `--capacity-profile` | 無効 | プロファイル全体を比較対象にする |
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

TSV と JSON の各行は `rank`、`allocated_bytes`、`apparent_bytes`、`inode_count`、`share_percent`、`file_count`、`directory_count`、`path` です。`--stale` を付けると、`path` の前に `latest_modified_epoch`、`latest_modified_at`、`stale_age_days`、`stale_level` が加わります。`--owners` を付けると、`path` の前に `owner_uid`、`owner`、`owner_share_percent` が加わります。許容量の判定結果は JSON の `capacity` に入り、TSV には出力されません。

</details>

<details>
<summary><code>qdu diff [PATH]</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `--from SELECTOR` | `--to` の直前の完全なスナップショット | 比較元のスナップショット。`--to` より古いものを指定する |
| `--to SELECTOR` | `latest` | 比較先のスナップショット |
| `-L, --max-depth N` | `1` | 表示する相対深さ |
| `-n, --top N` | `20` | 表示件数 |
| `PATH`（位置引数） | 走査ルート | 比較範囲の起点 |
| `--min-size SIZE` | `0` | 前回と今回の値の大きいほうが指定値以上の項目だけを表示。`--metric inodes` では inode 数 |
| `--match PATTERN` | なし | パス名を絞り込む |
| `--growth-only` | 無効 | 増加した項目だけを表示 |
| `--shrink-only` | 無効 | 減少した項目だけを表示 |
| `--order ORDER` | `--shrink-only` では `shrink`、それ以外は `growth` | `growth`、`shrink`、`absolute`、`current` |
| `--metric METRIC` | `allocated` | 比べる値。`allocated`（割り当て容量）、`apparent`（見かけのサイズ）、`inodes`（inode 数） |
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

並べる値が同じ行はパスの順に並びます。走査ルート、除外パターン、別のファイルシステムに入るかどうか、ディレクトリを保存する深さ（`--record-max-depth`）のいずれかが異なるスナップショットを比較すると、警告を表示します。TSV と JSON の各行は `rank`、`change`、`change_percent`、`current`、`previous`、`path` です。

</details>

<details>
<summary><code>qdu history [PATH]</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `PATH`（位置引数） | 走査ルート | 推移を表示するディレクトリ |
| `-n, --top N` | `20` | 新しいほうから数えて使う、完全なスナップショットの数 |
| `--metric METRIC` | `allocated` | `allocated`（割り当て容量）、`apparent`（見かけのサイズ）、`inodes`（inode 数） |
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

各行には取得日時（`Taken`）、値（`Size`、`--metric inodes` では `Inodes`）、1つ前のスナップショットからの増減（`Change`）、スナップショットID（`Snapshot`）を表示します。要約には1週間あたりの増加量が入ります。割り当て容量の推移では、その増え方が続いた場合にファイルシステムが満杯になる時期も示します。プロファイル全体を対象にした許容量を保存していれば、走査ルート全体の割り当て容量の推移に、許容量に達する時期も加わります。

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

各スナップショットのラベルと、スナップショットのファイルがディスク上で使う容量、プロファイル全体の合計も表示します。スナップショットIDは省略しません。

</details>

<details>
<summary><code>qdu users [PATH]</code></summary>

`PATH` を指定すると、そのディレクトリの下にあるファイルを所有者ごとに集計し、各所有者の上位ディレクトリもその下から選びます。`PATH` は、スナップショットの `user_max_depth` までの深さにある必要があります。

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `--snapshot SELECTOR` | `latest` | 使用するスナップショット |
| `-n, --top N` | `10` | 表示するユーザー数 |
| `--dirs N` | `5` | ユーザーごとの上位ディレクトリ数 |
| `--metric METRIC` | `allocated` | `allocated`（割り当て容量）または `inodes`（inode 数） |
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

表ではユーザーの順位に続けて、ユーザーごとの上位ディレクトリを表示します。TSV と JSON ではユーザーとそのディレクトリをそれぞれ1行として出力し、`type` 列の `user` と `directory` で区別します。

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

走査で読み取れなかったパスを、失敗した操作、種類、メッセージとともに表示します。種類は権限不足なら `denied`、それ以外なら `error` です。権限不足（`denied`）で読めなかったパスだけなら、スナップショットは不完全になりません。

</details>

<details>
<summary><code>qdu files [PATH]</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `--snapshot SELECTOR` | `latest` | 使用するスナップショット |
| `--live` | 無効 | 現在のファイルシステムを再走査する |
| `--path DIR` | 選択したスナップショットのルート、なければプロファイルのルート | `--live` の走査ルート。`--live` と一緒にだけ指定できる |
| `--threads N` | `$QDU_THREADS`、未設定なら CPU の数（上限 16） | `--live` の走査に使うスレッド数。`--live` と一緒にだけ指定できる |
| `-n, --top N` | `30` | 表示件数 |
| `PATH`（位置引数） | 走査ルート | 検索範囲の起点 |
| `--min-size SIZE` | `0` | 指定容量以上のファイルだけを表示 |
| `--user USER` | なし | 所有ユーザーで絞り込む |
| `--stale` | 無効 | 更新されていない期間を表示・色分けする |
| `--stale-only DAYS` | なし | 指定日数以上更新されていないファイルだけを表示。`90` のような日数か、`90d`、`26w` のような1日の整数倍の期間で指定する |
| `--stale-thresholds D1,D2,D3,D4` | `30,90,180,365` | 色分けする日数 |
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

端末で実行すると、`--live` の走査中は標準エラーに進捗を表示します。`--live` で走査するディレクトリを読み取れない場合は、使い方のエラー（終了コード `2`）になります。

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
| `--metric METRIC` | `allocated` | 容量の基準。`allocated` または `apparent` |
| `--stale` | 無効 | 更新されていない期間を表示 |
| `--stale-only DAYS` | なし | 更新されていない日数で絞り込む |
| `--stale-thresholds D1,D2,D3,D4` | `30,90,180,365` | 色分けする日数 |
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

選択を取り消した場合は何も表示せずに終了し、`fzf` 自体が失敗した場合はその終了ステータスを表示します。

</details>

<details>
<summary><code>qdu check [PATH]</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `--snapshot SELECTOR` | `latest` | 判定に使うスナップショット |
| `PATH`（位置引数） | 走査ルート | 増加量を判定するディレクトリ。比べる 2 つのスナップショットの少なくとも一方に記録されている必要がある |
| `--growth-over SIZE` | 未指定 | 許容する増加量 |
| `--disk-usage-over PERCENT` | 未指定 | 現在のファイルシステム使用率の上限 |
| `--inode-usage-over PERCENT` | 未指定 | 現在のinode使用率の上限 |
| `--snapshot-age-over DURATION` | 未指定 | 判定するスナップショットの経過時間の上限。例：`8d` |
| `--capacity-limit SIZE` | プロファイル設定。初期値は未設定 | 運用上の許容量 |
| `--no-capacity-limit` | 無効 | 保存済みの許容量を今回だけ無視する |
| `--capacity-user USER` | プロファイル設定。初期値は未指定 | 指定ユーザーの使用量を判定する |
| `--capacity-profile` | 無効 | プロファイル全体を判定する |
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

少なくとも1つの判定条件が必要です。ただし、プロファイルに許容量を保存している場合は、追加オプションなしでも許容量を判定します。

TSV と JSON の各行は `name`、`passed`、`observed`、`threshold`、`unit`、`observed_text`、`threshold_text`、`target` です。`observed` と `threshold` は数値で、その単位を `unit`（`bytes`、`percent`、`seconds`）が示します。

</details>

<details>
<summary><code>qdu schedule</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `--every PERIOD` | `weekly` | `daily`、`weekly`、`monthly`（毎月 1 日） |
| `--at HH:MM` | `03:15` | 実行する時刻 |
| `--weekday DAY` | `mon` | 週 1 回のときの曜日。`--every weekly` のときだけ指定できる |
| `--qdu PATH` | 実行中の qdu | cron が実行する qdu のパス |
| `--install` | 無効 | crontab に登録する。このプロファイルの古い行は置き換える |
| `--remove` | 無効 | このプロファイルの行を crontab から削除する |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

</details>

<details>
<summary><code>qdu compact</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `--older-than DURATION` | `7d` | この期間より古いスナップショットを圧縮 |
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

診断だけを行い、何も変更しません。設定ファイルを読めない場合は、`Config` の行を `FAILED` として報告します。`FAILED` の行があると、終了コード `1` で終わります。

</details>

<details>
<summary><code>qdu repair</code></summary>

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 修復するプロファイル |
| `--dry-run` | 無効 | 実際には変更せず処理内容を表示 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

読み取れるインデックスがあれば、そこに記録されたラベルを引き継ぎます。

</details>

<details>
<summary><code>qdu label NAME</code></summary>

位置引数 `NAME` には付けるラベルを指定します。ラベルには英数字で始まる64文字までの名前を使います。使える文字は英数字と `.`、`_`、`-` です。

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `--snapshot SELECTOR` | `latest` | ラベルを付けるスナップショット |
| `--remove` | 無効 | そのラベルを付けたスナップショットから、ラベルを外す |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

同じラベルがほかのスナップショットに付いている場合は、指定したスナップショットへ移します。ラベルを付けたスナップショットは `@NAME` で選べ、保持件数を超えても削除されません。取得と同時にラベルを付けるには、`qdu snapshot --label NAME` を使います。

</details>

<details>
<summary><code>qdu forget SELECTOR...</code></summary>

位置引数 `SELECTOR` には削除するスナップショットを1つ以上指定します。指定方法は `--snapshot` と同じです。

| オプション | 既定値 | 説明 |
|---|---|---|
| `--profile NAME` | `$QDU_PROFILE`、未設定なら `default` | 使用するプロファイル |
| `--dry-run` | 無効 | 実際には削除せず対象だけを表示 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

指定したスナップショットを、そのファイルとともに削除します。

</details>

<details>
<summary><code>qdu profile</code></summary>

プロファイル名には英数字で始まる64文字までの名前を使います。使える文字は英数字と `.`、`_`、`-` で、パスの形をした名前は使えません。

##### `qdu profile list`

設定ファイルにあるプロファイルと、スナップショットのあるプロファイルについて、スナップショット数、最新スナップショットの日時、許容量、走査ルートを表示します。

| オプション | 既定値 | 説明 |
|---|---|---|
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

##### `qdu profile show [NAME]`

設定ファイルのキー名で、プロファイルの設定値を表示します。`NAME` を省略すると、`QDU_PROFILE` で指定したプロファイル、未設定なら `default` を表示します。TSV は見出しの行（`setting`、`value`）に続けて1行に1つの設定と値を出力し、除外パターンは1パターン1行です。JSON では設定を `settings` の下にまとめます。

| オプション | 既定値 | 説明 |
|---|---|---|
| `--format FORMAT` | `table` | 出力形式 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

##### `qdu profile set NAME`

プロファイルを作成するか、既存のプロファイルの指定した設定を変更します。`qdu profile add` も同じコマンドです。

| オプション | 既定値 | 説明 |
|---|---|---|
| `--path PATH` | 既存設定を維持。新規は必須 | 走査ルート |
| `--exclude PATTERN` | 既存設定を維持。新規は `[defaults]` | 除外パターン。複数回指定可能で、指定すると保存済みの一覧を置き換える |
| `--keep-snapshots N` | 既存設定を維持。新規は `[defaults]`、なければ `52` | 保持する、ラベルのないスナップショットの数。`0` は無制限 |
| `--compact-after DURATION` | 既存設定を維持。新規は `[defaults]`、なければ `7d` | 取得後に圧縮するスナップショットの経過時間。`off` で無効 |
| `--with-users` | 既存設定を維持。新規は `[defaults]`、なければ無効 | ユーザー別統計を保存 |
| `--without-users` | 既存設定を維持 | ユーザー別統計を保存しない |
| `--user-max-depth N` | 既存設定を維持。新規は `[defaults]`、なければ `3` | ユーザー別ディレクトリ集計の深さ |
| `--file-top N` | 既存設定を維持。新規は `[defaults]`、なければ `1000` | 保存する大容量ファイル候補数 |
| `--record-max-depth N` | 既存設定を維持。新規は `[defaults]`、なければ制限なし | 保存するディレクトリ深さ |
| `--capacity-limit SIZE` | 既存設定を維持。新規は未設定 | 運用上の許容量 |
| `--no-capacity-limit` | 無効 | 許容量設定を削除 |
| `--capacity-user USER` | 既存設定を維持。新規は未指定 | 指定ユーザーを許容量の対象にする |
| `--capacity-profile` | 無効（既存設定を維持） | プロファイル全体を対象にする |
| `--cross-filesystems` | 既存設定を維持。新規は `[defaults]`、なければ無効 | 別ファイルシステムにも入る |
| `--one-file-system` | 既存設定を維持。新規は `[defaults]`、なければ有効 | 同じファイルシステム内だけを走査 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

指定しなかったオプションは、既存のプロファイルでは保存済みの値を保ちます。新しいプロファイルでは `[defaults]` の値、なければ表の既定値になります。スナップショットのあるプロファイルでは、以後のスナップショットも同じディレクトリを走査する必要があるため、`--path` で別のディレクトリを指定するとエラーになります。

##### `qdu profile remove NAME`

| オプション | 既定値 | 説明 |
|---|---|---|
| `--delete-data` | 無効 | 設定とともにスナップショットも削除 |
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

設定ファイルからプロファイルを削除し、`qdu schedule` で crontab に登録した行も削除します。

</details>

<details>
<summary><code>qdu config</code></summary>

プロファイルの設定値は `qdu profile show` で表示します。

##### `qdu config path`

| オプション | 既定値 | 説明 |
|---|---|---|
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

##### `qdu config init`

`[defaults]` セクションを含む初期設定ファイルを作成します。既存のファイルは上書きせず、`[defaults]` セクションがない場合だけ、ほかのセクションを残したまま先頭に追加します。

| オプション | 既定値 | 説明 |
|---|---|---|
| `--style STYLE` | `auto` | 表示スタイル |
| `--color MODE` | `auto` | 色表示 |

</details>

<details>
<summary><code>qdu completions SHELL</code></summary>

位置引数 `SHELL` には `bash`、`zsh`、`fish`、`elvish`、`powershell` のいずれかを指定します。指定したシェル用の補完スクリプトを標準出力に書き出します。

```bash
qdu completions bash > ~/.local/share/bash-completion/completions/qdu
qdu completions zsh > ~/.zfunc/_qdu
qdu completions fish > ~/.config/fish/completions/qdu.fish
```

zsh では `~/.zfunc` を `fpath` に加えておきます。

</details>

<a id="snapshot-selectors"></a>

## スナップショットの指定

`--snapshot`、`--from`、`--to` と `qdu forget` では、次の方法でスナップショットを指定できます。

| 指定 | 選ばれるスナップショット |
|---|---|
| `latest` | 最新の完全なスナップショット |
| `latest-any` | 完全・不完全を問わない最新のスナップショット |
| `previous`、`latest~1` | `latest` の1つ前の完全なスナップショット。`latest~N` は N 個前 |
| `30d`、`2w`、`12h` | 取得から指定した期間以上たったスナップショットのうち、最新の完全なもの |
| `2026-01-31` | その日の終わり（ローカル時刻）までに取得したスナップショットのうち、最新の完全なもの |
| `@NAME` | ラベル `NAME` を付けたスナップショット |
| ID、ファイル名、重複しないIDの先頭部分 | 指定したスナップショット。完全かどうかは問わない |

`qdu errors`、`qdu verify`、`qdu label`、`qdu forget` では、不完全なものも含めたすべてのスナップショットを数えて `latest~N` を選びます。`latest` で選んだとき、それより新しい不完全なスナップショットがあれば、標準エラーにその旨を表示します。インデックスに安全でないファイル名があると、保存データの形式が正しくないものとして拒否します。

<a id="output-formats"></a>

## 出力形式

表は人が読むための形式です。端末に表示するときは幅に収まらない長いパスの中央を省略しますが、スナップショットIDは省略しません。TSV と JSON はスクリプトで使うための形式で、構成が変わるのは `format_version` が変わるときだけです。

- JSON は1つのオブジェクトです。`format_version`（`1`）、`command`、`profile` のあとに、`snapshot`、`scope`、`metric` など結果の前提となる値が続き、各行は `rows` に入ります。
- TSV は JSON と同じ行を出力し、見出しの行には JSON のキーを使います。値の中のタブ、改行、復帰、バックスラッシュは `\t`、`\n`、`\r`、`\\` とエスケープします。値がない欄は空になります。
- 順位のある行には `rank` が付きます。サイズはバイト、経過期間は日数で表します。時刻は Unix エポック秒の数値で表し、キーが `_at` で終わる列には同じ時刻を ISO 8601 形式の文字列で出力します。
