# qdu 分析ガイド

[English](ANALYSIS_GUIDE.md) | 日本語

保存したスナップショットから、容量増加や整理候補を調べる方法を説明します。

## 容量ランキング

```bash
qdu show [options]
```

```bash
# 深さ3までの上位50件
qdu show --max-depth 3 --top 50

# Library/Caches の中を表示
qdu show --under Library/Caches --max-depth 2

# 1 GiB以上だけ表示
qdu show --min-size 1GiB

# パスに cache を含むディレクトリ
qdu show --match cache

# models 直下のディレクトリ
qdu show --match 'models/*'
```

`qdu show` の対象は**ディレクトリ**です。個別ファイルは `qdu files` で調べます。

`--under` を指定すると、`Share` は指定したディレクトリを100%として計算されます。

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

`--stale-only` は `--stale` を暗黙に有効化します。大容量ファイルについても同じオプションを使えます。

```bash
qdu files --stale
qdu files --stale-only 180 --top 100
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

```bash
qdu show --warn-age 24h
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
qdu profile add shared-home \
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
qdu profile add project-data \
  --path /data/project \
  --capacity-limit 5TiB
```

保存済みの許容量を一時的に無効化するには、`--no-capacity-limit` を付けます。

```bash
qdu show --profile shared-home --no-capacity-limit
```

プロファイルから削除するには、`qdu profile add` に `--no-capacity-limit` を付けます。

```bash
qdu profile add shared-home \
  --path /home \
  --no-capacity-limit
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

また、`--under` で一部のディレクトリだけを表示している場合も、許容量はプロファイル全体、または `--capacity-user` で指定したユーザー全体に対して判定されます。

## 差分ランキング

```bash
qdu diff
```

既定では、最新の完全なスナップショットと、その直前の完全なスナップショットを比較します。

```bash
qdu diff --growth-only
qdu diff --shrink-only
qdu diff --under datasets --max-depth 3
```

特定の2時点を比較するには、`--from` と `--to` を指定します。

```bash
qdu diff --from 20260701T000000Z --to latest
```

スナップショットIDは `qdu list` で確認できます。完全なID、ファイル名、または重複しないIDの先頭部分を指定できます。

並び順は `--order` で指定します。

| 値 | 意味 |
|---|---|
| `growth` | 増加量が大きい順。既定値 |
| `shrink` | 減少量が大きい順 |
| `absolute` | 増減の絶対値が大きい順 |
| `current` | 現在容量が大きい順 |

```bash
qdu diff --order absolute --top 30
```

走査ルートや除外設定が異なるスナップショットを比較した場合は警告が表示されます。

比べる値は `--metric` で選べます。既定は割り当て容量の `allocated` で、ほかに見かけのサイズの `apparent` と inode 数の `inodes` があります。inode 数で比べると、容量は小さくてもファイルが大量に増えた場所が分かります。

```bash
qdu diff --metric inodes --growth-only
```

各行には、前回と今回の値、増減量、前回からの増減率が表示されます（JSON では `change_percent`）。前回に存在しなかった場所の増減率は、表では `new`、JSON では `null` です。`--min-size` は、前回と今回の値の大きいほうが指定値以上の行だけを残します。`--metric inodes` のときは inode 数として扱います。

## ディレクトリを掘り下げる

指定したディレクトリの直下を表示します。

```bash
qdu explain Library/Caches
```

同じ範囲の差分を見るには、`--diff` を付けます。

```bash
qdu explain Library/Caches --diff
```

`fzf` がある場合は対話的に選べます。

```bash
qdu browse
```

ディレクトリは割り当て容量の大きい順に、容量とともに並びます。入力した文字列はパスだけと照合します。

## ユーザー別ランキング

ユーザー別統計はスナップショット取得時に有効化します。

```bash
qdu profile add shared --path /data/shared --with-users
qdu snapshot --profile shared
```

```bash
qdu users --profile shared --top 10 --dirs 5
qdu show --profile shared --users
```

inode数を基準にするには、`--metric inodes` を指定するか、`qdu inodes --users` を使います。

```bash
qdu users --profile shared --metric inodes
qdu inodes --profile shared --users
```

`show --users` を JSON で出力すると、`users` 配列が加わります。TSV は 1 つの表しか出せないため、ユーザー別の行は `qdu users --format tsv` で取得してください。ユーザー別統計のないスナップショットに `--users` を指定すると、`qdu users` と同じ使い方のエラーになります。

qduが集計できるのは、実行した一般ユーザーが読み取れた範囲だけです。ユーザー名を解決できないUIDは、数値で表示されます。

ユーザー別ディレクトリの値は祖先へ積み上げられるため、表示されたディレクトリ容量を足してもユーザー合計にはなりません。

## 大容量ファイル

スナップショット取得時には、容量が大きいファイルを上位から一定件数保存します。既定は1000件です。

```bash
qdu files --top 30
qdu files --under models
qdu files --min-size 1GiB
qdu files --user alice
```

保存件数を増やすには、`--file-top` を指定します。

```bash
qdu snapshot --file-top 5000
```

通常の `qdu files` は保存済み候補の中から検索します。現在のファイルシステムを再走査し、条件に合うファイルを正確に探す場合は `--live` を使います。`--live` でも、別のファイルシステムに入るのは、スナップショットと同じくプロファイルで `cross_filesystems` を有効にした場合だけです。

```bash
qdu files --live --under models --user alice --top 30
qdu files --live --path /data/my-project --min-size 1GiB
```

## inode使用量

```bash
qdu inodes
qdu inodes --max-depth 3 --top 30
qdu inodes --under cache --max-depth 2
```

ユーザー別に表示する場合は、`--with-users` で取得したスナップショットが必要です。

```bash
qdu inodes --users
```

## スナップショットの一覧と選択

```bash
qdu list
qdu list --top 50
```

多くの表示コマンドで、`--snapshot` を使えます。

```bash
qdu show --snapshot latest
qdu show --snapshot latest-any
qdu show --snapshot previous
qdu show --snapshot 20260719T030000Z
```

| 値 | 意味 |
|---|---|
| `latest` | 最新の完全なスナップショット |
| `latest-any` | 完全・不完全を問わない最新 |
| `previous` | `latest` より1つ前の完全なスナップショット |
| IDまたはID接頭辞 | 指定したスナップショット |

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

日本語や全角文字の表示幅も考慮されます。タブや改行を含むパスは、出力形式に合わせて安全にエスケープされます。
