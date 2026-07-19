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

既定の保存先：

```text
${XDG_CONFIG_HOME:-$HOME/.config}/qdu/config.ini
```

`qdu config init` で作られる設定：

```ini
[defaults]
keep_snapshots = 100
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

プロファイル別の設定例：

```ini
[profile:data]
path = /data/my-project
keep_snapshots = 60
collect_users = true
user_max_depth = 3
large_file_limit = 2000
record_max_depth =
cross_filesystems = true
capacity_limit = 2199023255552B
capacity_user = daiki
exclude =
    .cache
    tmp
```

コマンドラインオプションは設定ファイルより優先されます。

プロファイル名は英数字で始まり、英数字、`.`、`_`、`-`だけで構成する必要があります。`..`、絶対パス、`/`、`\\`を含む名前は使用できません。この制約は設定の読取・削除と状態パスの生成のすべてで適用されます。

設定ファイルを作成していない場合、除外パターンは空です。`.git` と `node_modules` を既定で除外したい場合は、`qdu config init` を実行してください。

## 保存場所

設定：

```text
${XDG_CONFIG_HOME:-$HOME/.config}/qdu/config.ini
```

スナップショットと履歴：

```text
${XDG_STATE_HOME:-$HOME/.local/state}/qdu/profiles/<profile>/
```

```text
index.json                 スナップショット一覧
snapshots/*.sqlite3        スナップショット
snapshots/*.sqlite3.gz     圧縮済みスナップショット
.lock                      同時実行防止用ロック
tmp/                       一時ファイル
```

SQLiteサーバーを起動する必要はありません。qduが新しく作る状態ディレクトリとファイルは、所有ユーザーだけがアクセスできる権限で保存されます。

`index.json`内のスナップショット名もbasenameと`.sqlite3`または`.sqlite3.gz`拡張子に制限されます。不正な参照は、ファイルの読取・削除より前に破損したインデックスとして拒否されます。
