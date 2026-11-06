# 設定と保存場所

[English](CONFIGURATION_AND_STORAGE.md) | 日本語

設定ファイル、プロファイル、スナップショットの保存場所を説明します。

## 設定ファイル

初期設定ファイルを作成します。

```bash
qdu config init
```

既存のファイルは上書きしません。`qdu profile set` が作ったファイルのように `[defaults]` セクションがない場合は、ほかのセクションを残したまま、先頭にこのセクションを追加します。

設定ファイルの場所は `qdu config path` で、プロファイルに適用される設定は `qdu profile show` で確認できます。`qdu profile show` で名前を省略したときは、環境変数 `QDU_PROFILE` に指定したプロファイル、指定がなければ `default` の設定を表示します。

```bash
qdu config path
qdu profile show
qdu profile show data
```

既定の保存先は次のとおりです。

```text
${XDG_CONFIG_HOME:-$HOME/.config}/qdu/config.ini
```

`qdu config init` で作られる設定は次のとおりです。

```ini
[defaults]
keep_snapshots = 52
compact_after = 7d
collect_users = false
user_max_depth = 3
large_file_limit = 1000
record_max_depth =
cross_filesystems = false
capacity_limit =
capacity_user =
exclude =
    .git
    node_modules
```

プロファイルごとの設定は `[profile:名前]` のセクションに書きます。

```ini
[profile:data]
path = /data/my-project
keep_snapshots = 60
collect_users = true
user_max_depth = 3
large_file_limit = 2000
record_max_depth =
cross_filesystems = true
capacity_limit = 2TiB
capacity_user = alice
exclude =
    .cache
    tmp
```

`compact_after` には `7d` のような期間か `off` を指定します（[運用と保守](OPERATIONS_ja.md)を参照）。`collect_users` と `cross_filesystems` に指定できる値は、`true`、`yes`、`on`、`1` と、`false`、`no`、`off`、`0` です。それ以外の値はエラーになります。

`path` を指定しない場合の走査ルートはホームディレクトリです。ただし、`path` を指定していないプロファイルにスナップショットがあれば、そのスナップショットと同じディレクトリを走査します。走査するディレクトリは `qdu profile show` で確認できます。

コマンドラインオプションは設定ファイルより優先されます。設定ファイルを直接編集するほかに、次の2つの操作でも設定が書き換わります。

- `qdu profile set 名前` は指定したオプションだけを書き込みます。指定しなかった設定は既存のプロファイルでは保存済みの値のまま、新しいプロファイルでは `[defaults]` の値になります。新しいプロファイルには `--path` の指定が必要です。スナップショットのあるプロファイルでは、以後のスナップショットも同じディレクトリを走査する必要があるため、ディレクトリを変更できません。`qdu profile add` は `qdu profile set` の別名です。
- `[profile:名前]` のセクションもスナップショットもない新しいプロファイルでは、最初のスナップショットで指定した走査ディレクトリ、除外パターン、設定を、新しいセクションに書き込みます。そのため、一度 `qdu snapshot --path DIR` を実行すれば、次からは `qdu snapshot` だけで `DIR` を走査します。それ以降は `--save` を付けて `qdu snapshot` を実行したときに書き込みます。

プロファイル名は64文字以内で、英数字で始まり、英数字、`.`、`_`、`-`だけで構成する必要があります。`..`、絶対パス、`/`、`\`を含む名前は使用できません。この制約は設定の読取・削除と状態パスの生成のすべてで適用されます。`QDU_PROFILE=` のように名前を空にすると、`default` プロファイルを使います。

プロファイルを読むコマンドは、設定もスナップショットもないプロファイルを受け付けず、既知のプロファイルのうち最も近い名前を示します。`--path` を付けて `qdu snapshot` を実行すると、新しいプロファイルを始められます。`qdu profile list` は設定がなくスナップショットだけがあるプロファイルも表示します。

設定ファイルを作成していない場合、除外パターンは空です。`.git` と `node_modules` を既定で除外したい場合は、`qdu config init` を実行してください。`#` か `;` で始まる除外パターンは設定ファイルでコメントとして読まれるため、qdu は保存を拒否します（[除外パターン](SNAPSHOTS_AND_SCANNING_ja.md#除外パターン)を参照）。

## 保存場所

設定ファイルは次の場所にあります。

```text
${XDG_CONFIG_HOME:-$HOME/.config}/qdu/config.ini
```

スナップショットと履歴は、プロファイルごとに次のディレクトリへ保存されます。

```text
${XDG_STATE_HOME:-$HOME/.local/state}/qdu/profiles/<profile>/
```

このディレクトリには次のファイルがあります。

```text
index.json                 スナップショット一覧とラベル
snapshots/*.sqlite3        スナップショット
snapshots/*.sqlite3.gz     圧縮済みスナップショット
.lock                      同時実行防止用ロック
tmp/                       作成中のスナップショット
```

SQLiteサーバーを起動する必要はありません。圧縮済みのスナップショットを読むときは、システムの一時ディレクトリ（`$TMPDIR`。通常は `/tmp`）に所有者だけが読める一時ファイルとして展開し、読み終えたら削除します。遅いネットワーク上のホームや、空きの少ないホームに大きな複製を書かないためです。qduが新しく作る状態ディレクトリとファイルは、所有ユーザーだけがアクセスできる権限で保存されます。

`index.json`内のスナップショット名もbasenameと`.sqlite3`または`.sqlite3.gz`拡張子に制限されます。不正な参照はファイルの読取・削除より前に破損したインデックスとして拒否されます。`qdu repair` でインデックスを作り直すときは、読める `index.json` があれば、そこにあるラベルを引き継ぎます。

スナップショットはスキーマ 4 の SQLite データベースとして保存されます。スキーマ 4 の `scan_errors` テーブルは、失敗ごとに権限の不足によるものかどうかも記録します。Python 版の qdu 0.1.0 と 0.2.0 が作ったスキーマ 2 と 3 のスナップショットもそのまま読めます。その場合、記録された失敗はすべてエラーとして数えます。Python 版の qdu は、スキーマ 4 のスナップショットを読めません。読めるスキーマのスナップショットではないファイルを開くと形式エラーになり、終了コード 4 で終了します。
