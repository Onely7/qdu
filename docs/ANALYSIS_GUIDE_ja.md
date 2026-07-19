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

90日以上更新されていない候補だけへ絞り込む場合：

```bash
qdu show --stale-only 90 --max-depth 4 --top 100
```

`--stale-only` は `--stale` を暗黙に有効化します。大容量ファイルについても同じオプションを使えます。

```bash
qdu files --stale
qdu files --stale-only 180 --top 100
```

警告を開始する日数は、4段階の値を小さい順に指定できます。

```bash
qdu show --stale --stale-thresholds 14,30,90,180
```

判定には、スナップショット取得時刻と、ディレクトリ自身および配下にある項目の最も新しい更新時刻を使います。例えば、古いディレクトリの深い場所に1つでも最近更新されたファイルがあれば、その親ディレクトリは古い候補として扱われません。空のディレクトリでは、ディレクトリ自身の更新時刻を使います。

> [!NOTE]
> 除外したパスと、権限不足で読めなかったパスは判定に含まれません。不完全なスナップショットでは、表示された経過日数も読み取れた範囲に基づく値です。

#### 容量の基準を変える

```bash
# 既定：割り当て済み容量
qdu show --metric allocated_bytes

# 見かけ上のサイズ
qdu show --metric apparent_bytes
```

#### 古いスナップショットへ警告を出す

```bash
qdu show --warn-age 24h
```

時間には `s`、`m`、`h`、`d`、`w` を使えます。例：`30m`、`7d`、`2w`。

## 運用上の許容量を確認する

ストレージ全体には空きがあっても、自分に許可された容量はそれより小さいことがあります。例えば、ファイルシステム全体が66TBでも、自分が使ってよい容量を2TBと指示されているなら、2TBを基準に使用率を確認する必要があります。

一時的に2TiBを上限として表示する場合：

```bash
qdu show --capacity-limit 2TiB
```

`qdu show` には、次の情報が追加されます。

- 現在の使用量
- 許可された容量
- 使用率
- 残り容量、または超過量
- 上限内か超過中か

上限を超えている場合は、表のステータスが赤くなり、標準エラーにも警告が表示されます。`qdu show` 自体の終了コードは、表示に成功していれば `0` のままです。

#### 共有領域で自分の使用量だけを確認する

`shared-home` のように複数ユーザーを含むプロファイルでは、ユーザー名またはUIDを指定できます。

```bash
qdu show \
  --profile shared-home \
  --capacity-limit 2TiB \
  --capacity-user daiki
```

ユーザー別の判定には、`--with-users` で作成したスナップショットが必要です。プロファイルへ `--capacity-user` を保存する場合も、`--with-users` を同時に指定してください。

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
  --capacity-user daiki
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

保存済みの許容量を一時的に無効化する場合：

```bash
qdu show --profile shared-home --no-capacity-limit
```

プロファイルから削除する場合：

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
  --capacity-user daiki
```

許容量をプロファイルへ保存済みなら、オプションを省略できます。

```bash
qdu check --profile shared-home
```

この動作は、tmux内の定期実行や通知スクリプトと組み合わせるときに便利です。

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

許容量との比較には、常に**割り当て済み容量**を使用します。`qdu show --metric apparent_bytes` を指定しても、許容量の判定基準は変わりません。

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

特定の2時点を比較：

```bash
qdu diff --from 20260701T000000Z --to latest
```

スナップショットIDは `qdu list` で確認できます。完全なID、ファイル名、または重複しないIDの先頭部分を指定できます。

並び順：

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

## ディレクトリを掘り下げる

指定したディレクトリの直下を表示します。

```bash
qdu explain Library/Caches
```

同じ範囲の差分：

```bash
qdu explain Library/Caches --diff
```

`fzf` がある場合は対話的に選べます。

```bash
qdu browse
```

## ユーザー別ランキング

ユーザー別統計は、スナップショット取得時に有効化します。

```bash
qdu profile add shared --path /data/shared --with-users
qdu snapshot --profile shared
```

```bash
qdu users --profile shared --top 10 --dirs 5
qdu show --profile shared --users
```

inode数を基準にする場合：

```bash
qdu users --profile shared --metric inode_count
qdu inodes --profile shared --users
```

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

保存件数を増やす場合：

```bash
qdu snapshot --file-top 5000
```

通常の `qdu files` は、保存済み候補の中から検索します。現在のファイルシステムを再走査し、条件に合うファイルを正確に探す場合は `--live` を使います。

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

`KB`、`MB`、`GB` などの10進単位と、`KiB`、`MiB`、`GiB` などの2進単位を使えます。

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
