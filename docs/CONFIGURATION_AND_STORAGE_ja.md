# 設定と保存場所

[English](CONFIGURATION_AND_STORAGE.md) | 日本語

設定ファイル、プロファイル、スナップショットの保存場所を説明します。

## 設定ファイル

初期設定ファイルを作成します。

```bash
qdu config init
```

既存ファイルは上書きしません。

```bash
qdu config path
qdu config show
qdu config show --profile data
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

`compact_after` には `7d` のような期間か `off` を指定します（[運用と保守](OPERATIONS_ja.md)を参照）。

コマンドラインオプションは設定ファイルより優先されます。`qdu profile add` は指定したオプションだけを書き込みます。指定しなかった設定は、既存のプロファイルでは保存済みの値のまま、新しいプロファイルでは `[defaults]` の値になります。

プロファイル名は英数字で始まり、英数字、`.`、`_`、`-`だけで構成する必要があります。`..`、絶対パス、`/`、`\`を含む名前は使用できません。この制約は設定の読取・削除と状態パスの生成のすべてで適用されます。

設定ファイルを作成していない場合、除外パターンは空です。`.git` と `node_modules` を既定で除外したい場合は、`qdu config init` を実行してください。

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
index.json                 スナップショット一覧
snapshots/*.sqlite3        スナップショット
snapshots/*.sqlite3.gz     圧縮済みスナップショット
.lock                      同時実行防止用ロック
tmp/                       作成中のスナップショット
```

SQLiteサーバーを起動する必要はありません。圧縮済みのスナップショットを読むときは、システムの一時ディレクトリ（`$TMPDIR`。通常は `/tmp`）に所有者だけが読める一時ファイルとして展開し、読み終えたら削除します。遅いネットワーク上のホームや、空きの少ないホームに大きな複製を書かないためです。qduが新しく作る状態ディレクトリとファイルは、所有ユーザーだけがアクセスできる権限で保存されます。

`index.json`内のスナップショット名もbasenameと`.sqlite3`または`.sqlite3.gz`拡張子に制限されます。不正な参照はファイルの読取・削除より前に破損したインデックスとして拒否されます。

スナップショットはスキーマ 3 の SQLite データベースとして保存されます。qdu 0.1.0 が作ったスキーマ 2 のスナップショットもそのまま読めますが、qdu 0.1.0 はスキーマ 3 を読めません。
