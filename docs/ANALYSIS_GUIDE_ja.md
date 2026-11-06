# qdu 分析ガイド

[English](ANALYSIS_GUIDE.md) | 日本語

保存したスナップショットから、容量増加や整理候補を調べる方法を説明します。

## 容量ランキング

```bash
qdu show [options] [PATH]
```

```bash
# 深さ3までの上位50件
qdu show --max-depth 3 --top 50

# Library/Caches の中を表示
qdu show Library/Caches --max-depth 2

# 走査ルートの中を絶対パスで指定
qdu show ~/Library/Caches

# 1 GiB以上だけ表示
qdu show --min-size 1GiB

# パスに cache を含むディレクトリ
qdu show --match cache

# models 直下のディレクトリ
qdu show --match 'models/*'
```

`qdu show` の対象は**ディレクトリ**です。個別ファイルは `qdu files` で調べます。

`PATH` には走査ルートからの相対パスか、走査ルートの中を指す絶対パスを指定します。`PATH` を指定すると、`Share` は指定したディレクトリを100%として計算されます。

コマンドを付けずに実行すると、指定したオプションで `qdu show` を実行します。例えば `qdu -L 2` は `qdu show -L 2` と同じです。

#### 長期間更新されていない場所を見つける

作成したまま整理を忘れているディレクトリは、容量ランキングだけでは見つけにくいことがあります。`--stale` を付けると、各ディレクトリ配下で最後に更新された項目からの経過日数を表示し、長期間動きのない行へ色を付けます。

```bash
qdu show --stale
```

既定の警告段階は次のとおりです。

| 最後の更新からの経過日数 | 表示 |
|---:|---|
| 0〜29日 | 色なし |
| 30〜89日 | 青 |
| 90〜179日 | 黄 |
| 180〜364日 | オレンジ |
| 365日以上 | 赤 |

色が使えない端末や `--color never` を指定した場合でも、`Stale` 列に経過日数が表示されます。

```bash
qdu show --stale --color never
```

90日以上更新されていない候補だけへ絞り込むには、`--stale-only` で日数を指定します。

```bash
qdu show --stale-only 90 --max-depth 4 --top 100
```

日数は `90` のような数のほか、`90d` や `26w` のような期間でも指定できます。`36h` のように1日未満の端数が出る期間は使えません。

`--stale-only` は `--stale` を暗黙に有効化します。大容量ファイルについても同じオプションを使えます。

```bash
qdu files --stale
qdu files --stale-only 26w --top 100
```

警告を開始する日数は4段階の値を小さい順に指定できます。

```bash
qdu show --stale --stale-thresholds 14,30,90,180
```

判定にはスナップショット取得時刻と、ディレクトリ自身および配下にある項目の最も新しい更新時刻を使います。例えば、古いディレクトリの深い場所に1つでも最近更新されたファイルがあれば、その親ディレクトリは古い候補として扱われません。空のディレクトリでは、ディレクトリ自身の更新時刻を使います。

> [!NOTE]
> 除外したパスと、権限不足で読めなかったパスは判定に含まれません。不完全なスナップショットでは、表示された経過日数も読み取れた範囲に基づく値です。

#### 容量の基準を変える

```bash
# 既定：割り当て済み容量
qdu show --metric allocated

# 見かけ上のサイズ
qdu show --metric apparent
```

#### 古いスナップショットへ警告を出す

スナップショットが古くなっていないかを確かめるには、`qdu check` の `--snapshot-age-over` を使います。最新の完全なスナップショットが指定した時間より古ければ、終了コード `10` を返します。

```bash
qdu check --snapshot-age-over 8d
```

時間には `s`、`m`、`h`、`d`、`w` を使えます。例：`30m`、`7d`、`2w`。

## 運用上の許容量を確認する

ストレージ全体には空きがあっても、自分に許可された容量はそれより小さいことがあります。例えば、ファイルシステム全体が100TBでも、自分が使ってよい容量を2TBと指示されているなら、2TBを基準に使用率を確認する必要があります。

一時的に2TiBを上限として表示するには、`--capacity-limit` を指定します。

```bash
qdu show --capacity-limit 2TiB
```

`qdu show` には、次の情報が追加されます。

- 現在の使用量
- 許可された容量
- 使用率
- 残り容量、または超過量
- 上限内か超過中か

上限を超えている場合は、表のステータスが赤くなり、標準エラーにも警告が表示されます。`qdu show` 自体の終了コードは表示に成功していれば `0` のままです。

#### 共有領域で自分の使用量だけを確認する

`shared-home` のように複数ユーザーを含むプロファイルでは、ユーザー名またはUIDを指定できます。

```bash
qdu show \
  --profile shared-home \
  --capacity-limit 2TiB \
  --capacity-user "$USER"
```

ユーザー別の判定には `--with-users` で作成したスナップショットが必要です。ユーザー別統計のないスナップショットでは、`qdu show` は警告を出して許容量の判定だけを省き、`qdu check` は終了コード2で失敗します。プロファイルへ `--capacity-user` を保存する場合も、`--with-users` を同時に指定してください。

```bash
qdu snapshot \
  --profile shared-home \
  --with-users \
  --user-max-depth 4 \
  --file-top 2000
```

ユーザー名を解決できない環境では、数値のUIDも指定できます。

```bash
qdu show \
  --profile shared-home \
  --capacity-limit 2TiB \
  --capacity-user "$(id -u)"
```

#### プロファイルへ許容量を保存する

毎回オプションを付けたくない場合は、プロファイルへ保存します。

```bash
qdu profile set shared-home \
  --path /home \
  --cross-filesystems \
  --with-users \
  --user-max-depth 4 \
  --file-top 2000 \
  --capacity-limit 2TiB \
  --capacity-user "$USER"
```

以降は、通常の `qdu show` で許容量が表示されます。

```bash
qdu show --profile shared-home
```

プロファイル全体を対象にする場合は、`--capacity-user` を付けません。

```bash
qdu profile set project-data \
  --path /data/project \
  --capacity-limit 5TiB
```

保存済みの許容量を一時的に無効化するには、`--no-capacity-limit` を付けます。

```bash
qdu show --profile shared-home --no-capacity-limit
```

プロファイルから削除するには、`qdu profile set` に `--no-capacity-limit` を付けます。既存のプロファイルを変更するときは、`--path` を省略できます。

```bash
qdu profile set shared-home --no-capacity-limit
```

#### 自動処理で超過を判定する

`qdu check` は、上限内なら終了コード `0`、超過していれば終了コード `10` を返します。

```bash
qdu check \
  --profile shared-home \
  --capacity-limit 2TiB \
  --capacity-user "$USER"
```

許容量をプロファイルへ保存済みなら、オプションを省略できます。

```bash
qdu check --profile shared-home
```

この動作はtmux内の定期実行や通知スクリプトと組み合わせるときに便利です。

```bash
if qdu check --profile shared-home; then
  echo "容量は上限内です。"
else
  status=$?
  if [[ "$status" -eq 10 ]]; then
    echo "許容量を超えています。"
  else
    echo "qduの実行に失敗しました。" >&2
  fi
fi
```

#### 容量単位

`TB` と `TiB` は同じではありません。

| 指定 | バイト数 | 用途の目安 |
|---|---:|---|
| `2TB` | `2,000,000,000,000` | 10進単位で2TBと指示された場合 |
| `2TiB` | `2,199,023,255,552` | 2進単位で2TiBと指示された場合 |

管理者から単位が明示されていない場合は、どちらを意味するか確認してください。

#### 判定に使う容量

許容量との比較には常に**割り当て済み容量**を使用します。`qdu show --metric apparent` を指定しても、許容量の判定基準は変わりません。

また、`PATH` を指定して一部のディレクトリだけを表示している場合も、許容量はプロファイル全体、または `--capacity-user` で指定したユーザー全体に対して判定されます。

## 差分ランキング

```bash
qdu diff
```

既定では、最新の完全なスナップショットと、その直前の完全なスナップショットを比較します。

```bash
qdu diff --growth-only
qdu diff --shrink-only
qdu diff datasets --max-depth 3
```

特定の2時点を比較するには、`--from` と `--to` を指定します。`--from` には `--to` より古いスナップショットを指定します。

```bash
qdu diff --from 20260701T000000Z --to latest

# 30日前の時点で最新だったスナップショットと比べる
qdu diff --from 30d

# ラベル before-cleanup を付けたスナップショットと比べる
qdu diff --from @before-cleanup
```

スナップショットIDは `qdu list` で確認できます。完全なID、ファイル名、または重複しないIDの先頭部分を指定できます。ほかの指定方法は[スナップショットの一覧と選択](#スナップショットの一覧と選択)を参照してください。

並び順は `--order` で指定します。並べる値が同じ行はパスの順に並びます。

| 値 | 意味 |
|---|---|
| `growth` | 増加量が大きい順。`--shrink-only` を付けないときの既定値 |
| `shrink` | 減少量が大きい順。`--shrink-only` を付けたときの既定値 |
| `absolute` | 増減の絶対値が大きい順 |
| `current` | 現在容量が大きい順 |

```bash
qdu diff --order absolute --top 30
```

走査ルート、除外パターン、別のファイルシステムに入るかどうか、ディレクトリを保存する深さ（`--record-max-depth`）のいずれかが異なるスナップショットを比較した場合は、警告が表示されます。

比べる値は `--metric` で選べます。既定は割り当て容量の `allocated` で、ほかに見かけのサイズの `apparent` と inode 数の `inodes` があります。inode 数で比べると、容量は小さくてもファイルが大量に増えた場所が分かります。

```bash
qdu diff --metric inodes --growth-only
```

各行には、前回と今回の値、増減量、前回からの増減率が表示されます（JSON では `change_percent`）。前回に存在しなかった場所の増減率は、表では `new`、JSON では `null` です。`--min-size` は、前回と今回の値の大きいほうが指定値以上の行だけを残します。`--metric inodes` のときは inode 数として扱います。

## 容量の推移と予測

`qdu history` は指定したディレクトリか走査ルート全体の値を、新しいほうから20件の完全なスナップショットについて表示します。件数は `--top` で変えられます。

```bash
# 走査ルート全体
qdu history

# data/models の直近52件
qdu history data/models --top 52

# inode 数の推移
qdu history --metric inodes
```

各行には取得日時（`Taken`）、そのスナップショットでの値（`Size` 列、`--metric inodes` では `Inodes` 列）、1つ前のスナップショットからの増減（`Change`）、スナップショットID（`Snapshot`）が表示されます。ディレクトリが存在しないスナップショットでは、値が `absent` になります。

表の上には、すべての行から求めた1週間あたりの増加量が表示されます。割り当て容量（既定の `--metric allocated`）の推移では、その増え方が続いた場合にファイルシステムが満杯になる時期も示します。空き容量にはいまの値を使い、ほかのホストで取ったスナップショットでは、最新のスナップショットに記録された値を使います。プロファイル全体を対象にした許容量を保存していれば、走査ルート全体の割り当て容量の推移に、許容量に達する時期も加わります。

走査ルート全体の推移はスナップショットのインデックスから読み取ります。ディレクトリの推移は各スナップショットを開いて読むため、圧縮したスナップショットでは時間がかかります。

## ディレクトリを掘り下げる

`qdu show` に `PATH` を指定すると、そのディレクトリの直下を表示します。

```bash
qdu show Library/Caches
```

同じ範囲の差分を見るには、`qdu diff` に `PATH` を指定します。

```bash
qdu diff Library/Caches
```

`fzf` がある場合は対話的に選べます。

```bash
qdu browse
```

ディレクトリは割り当て容量の大きい順に、容量とともに並びます。入力した文字列はパスだけと照合します。選択を取り消した場合は何も表示せずに終了し、`fzf` 自体が失敗した場合はその終了ステータスを表示します。

## ユーザー別ランキング

ユーザー別統計はスナップショット取得時に有効化します。

```bash
qdu profile set shared --path /data/shared --with-users
qdu snapshot --profile shared
```

```bash
qdu users --profile shared --top 10 --dirs 5
```

ディレクトリを指定すると、その下のファイルの所有者を順位付けします。共有ディレクトリでは、ディレクトリ自体の所有者に関係なく、中のファイルを誰が置いたかが分かります。指定できるのは、スナップショットの `user_max_depth` までの深さにあるディレクトリです。

```bash
qdu users projects/shared --profile shared
```

`qdu show` に `--owners` を付けると、表示した各ディレクトリで最も多く使っている所有者と、その割合が加わります。`user_max_depth` より深いディレクトリでは空欄になります。

```bash
qdu show projects --profile shared --owners
```

inode数を基準にするには、`--metric inodes` を指定します。

```bash
qdu users --profile shared --metric inodes
```

表ではユーザーの順位に続けて、ユーザーごとの上位ディレクトリを表示します。TSV と JSON ではユーザーとそのディレクトリをそれぞれ1行として出力し、`type` 列の `user` と `directory` で区別します。ユーザー別統計のないスナップショットに `qdu users` を実行すると、使い方のエラーになります。

qduが集計できるのは、実行した一般ユーザーが読み取れた範囲だけです。ユーザー名を解決できないUIDは、数値で表示されます。

ユーザー別ディレクトリの値は祖先へ積み上げられるため、表示されたディレクトリ容量を足してもユーザー合計にはなりません。

## 大容量ファイル

スナップショット取得時には、容量が大きいファイルを上位から一定件数保存します。既定は1000件です。

```bash
qdu files --top 30
qdu files models
qdu files --min-size 1GiB
qdu files --user alice
```

保存件数を増やすには、`--file-top` を指定します。

```bash
qdu snapshot --file-top 5000
```

通常の `qdu files` は保存済み候補の中から検索します。現在のファイルシステムを再走査し、条件に合うファイルを正確に探す場合は `--live` を使います。`--live` でも、別のファイルシステムに入るのは、スナップショットと同じくプロファイルで `cross_filesystems` を有効にした場合だけです。

```bash
qdu files --live models --user alice --top 30
qdu files --live --path /data/my-project --min-size 1GiB
```

端末で実行すると、`--live` の走査中は標準エラーに進捗が表示されます。`--path` と `--threads` は `--live` と一緒に指定したときだけ使えます。`--live` で走査するディレクトリを読み取れない場合は、使い方のエラー（終了コード `2`）になります。

## inode使用量

inode 数で並べるには、`qdu show` に `--metric inodes` を指定します。

```bash
qdu show --metric inodes
qdu show --metric inodes --max-depth 3 --top 30
qdu show cache --metric inodes --max-depth 2
```

表の `Size` 列は `Inodes` 列になり、`--min-size` は inode 数として扱われます。

ユーザー別に表示する場合は、`--with-users` で取得したスナップショットが必要です。

```bash
qdu users --metric inodes
```

## スナップショットの一覧と選択

```bash
qdu list
qdu list --top 50
```

一覧には各スナップショットのラベル（`Labels` 列）も表示されます。スナップショットIDは省略せずに表示します。

多くの表示コマンドで、`--snapshot` を使えます。

```bash
qdu show --snapshot latest
qdu show --snapshot latest-any
qdu show --snapshot previous
qdu show --snapshot latest~3
qdu show --snapshot 30d
qdu show --snapshot 2026-01-31
qdu show --snapshot @before-cleanup
qdu show --snapshot 20260719T030000Z
```

| 値 | 意味 |
|---|---|
| `latest` | 最新の完全なスナップショット |
| `latest-any` | 完全・不完全を問わない最新 |
| `previous` | `latest` より1つ前の完全なスナップショット。`latest~1` と同じ |
| `latest~N` | `latest` よりN個前の完全なスナップショット |
| `30d`、`2w`、`12h` などの期間 | 取得から指定した期間以上たったスナップショットのうち、最新の完全なもの |
| `2026-01-31` などの日付 | その日の終わり（ローカル時刻）までに取得したスナップショットのうち、最新の完全なもの |
| `@NAME` | ラベル `NAME` を付けたスナップショット |
| ID、ファイル名、またはID接頭辞 | 指定したスナップショット。完全かどうかは問わない |

`latest` より新しい不完全なスナップショットがある場合は、標準エラーにその旨が表示されます。

スナップショットにラベルを付けるには、既存のスナップショットなら `qdu label NAME` を、取得時なら `qdu snapshot --label NAME` を使います。

```bash
# 最新の完全なスナップショットにラベルを付ける
qdu label before-cleanup

# ラベルを外す
qdu label before-cleanup --remove
```

ラベルを付けたスナップショットは `@NAME` で選べ、保持件数を超えても削除されません。

## 絞り込み

#### サイズ

```bash
qdu show --min-size 500MiB
qdu diff --min-size 1GiB
```

`KB`、`MB`、`GB` などの10進単位と、`KiB`、`MiB`、`GiB` などの2進単位を使えます。`du` と同じく `K`、`M`、`G` のような接頭辞だけの書き方は2進単位で、`2T` は `2TiB` と同じです。

#### パス名

ワイルドカードを含まない `--match` は、大文字・小文字を区別しない部分一致です。

```bash
qdu show --match cache
```

`*`、`?`、`[` を含む場合はパターンとして照合します。

```bash
qdu show --match 'models/*'
```

## 出力形式

```bash
qdu show --format table
qdu show --format tsv
qdu show --format json
```

TSV と JSON の列とキーの構成は[コマンドリファレンス](COMMAND_REFERENCE_ja.md#output-formats)で説明しています。

`jq` がある場合は、JSONを整形できます。

```bash
qdu show --format json | jq .
```

`jq` はqduの必須依存ではありません。

#### 表示スタイル

```bash
qdu show --style auto
qdu show --style rich
qdu show --style plain
```

`auto` では、UTF-8対応の対話端末で罫線付き表示を使い、パイプやファイル出力ではプレーン表示へ切り替えます。

```bash
qdu show --color auto
qdu show --color always
qdu show --color never
```

`--color auto` は標準出力が端末なら表示する結果に、標準エラーが端末なら警告に色を付けます。環境変数 `NO_COLOR` を設定している場合は、色を付けません。

日本語や全角文字の表示幅も考慮されます。端末に表示する表では幅に収まらない長いパスの中央を省略しますが、スナップショットIDは省略しません。

タブや改行を含むパスは、出力形式に合わせて安全にエスケープされます。名前に含まれる制御文字や双方向テキストの制御文字は、表だけでなく警告とエラーメッセージでもエスケープされます。JSON の値は元の文字列のままです。
