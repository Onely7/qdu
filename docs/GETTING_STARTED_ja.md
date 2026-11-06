# qdu 導入ガイド

[English](GETTING_STARTED.md) | 日本語

インストールから最初の差分確認までを説明します。

## 動作環境

- LinuxまたはmacOS（x86_64またはarm64）
- 実行時に必要なものはありません。qduは単体で動く1つの実行ファイルです

`qdu browse`を使う場合だけ、別途[`fzf`](https://github.com/junegunn/fzf)が必要です。

## インストール

ふつうは次のどれかの方法でインストールします。手順は[README](../README_ja.md#インストール)にあります。

- 試すだけなら、インストールせずに`uvx qdu`で動かす
- 使い続けるなら、`uv tool install qdu`（または`pipx install qdu`、`python3 -m pip install --user qdu`）でインストールする
- Pythonがない環境では、最新のリリースから実行ファイルをダウンロードする

定期的にスナップショットを取る場合は、`uvx`ではなくインストールしたqduを使ってください。cronには置き場所の変わらない実行ファイルが必要だからです。

インストールできたか確認します。

```bash
command -v qdu
qdu --version
```

`command -v qdu`で何も表示されない場合は、`~/.local/bin`が`PATH`に含まれていません。まず、現在のターミナルで追加します。

```bash
export PATH="$HOME/.local/bin:$PATH"
```

次回からも有効にするには、使用中のシェルに合わせて設定ファイルへ追記してください。

```bash
# Bash
printf '%s\n' 'export PATH="$HOME/.local/bin:$PATH"' >> "$HOME/.bashrc"

# Zsh
printf '%s\n' 'export PATH="$HOME/.local/bin:$PATH"' >> "$HOME/.zshrc"
```

新しいターミナルを開いたあと、動作を確認します。

```bash
qdu --version
qdu doctor
```

#### ソースからビルドする

ビルドにはRust 1.85以降と、同梱のSQLiteをコンパイルするためのCコンパイラが必要です。リポジトリのルートで実行します。

```bash
make build      # cargo build --release
make install    # target/release/qdu を ~/.local/bin/qdu へコピー
qdu --version
```

別の場所へインストールする場合は、`PREFIX`を指定します。次の例では`/opt/qdu/bin/qdu`へインストールします。

```bash
make install PREFIX=/opt/qdu
```

リポジトリを取得せずにビルドする場合は、`cargo install`を使います。`~/.cargo/bin`へインストールされます。

```bash
cargo install --git https://github.com/Onely7/qdu --locked
```

## まずは3つのコマンドを試す

#### 1. 現在の状態を記録する

```bash
qdu snapshot
```

何も指定しない場合は、自分のホームディレクトリを走査します。別のディレクトリを記録する場合は`--path`で指定します。設定もスナップショットもない新しいプロファイルでは、指定したディレクトリと、`--exclude`や`--with-users`などの設定がそのプロファイルに保存されます。次回からは指定しなくても、同じディレクトリを同じ設定で走査します。

```bash
qdu snapshot --path /data/my-project
```

設定が保存されたあとに付けた`--with-users`などのオプションは、その回の走査だけに使われます。次回以降も使うには`--save`も付けます。

qduが保存するのは、容量、ファイル数、inode数、取得日時などの情報です。ファイルの内容そのものはコピーしません。

#### 2. 容量ランキングを見る

```bash
qdu show
```

深さ2までの上位30件を見るには、`--max-depth`と`--top`を指定します。

```bash
qdu show --max-depth 2 --top 30
```

引数なしの`qdu`も`qdu show`と同じ動作です。コマンドを省略してオプションだけを指定した場合も`qdu show`として動きます。`--max-depth`は`-L`、`--top`は`-n`と短く書けるので、次のコマンドは上の例と同じ表示になります。

```bash
qdu -L 2 -n 30
```

1つのディレクトリの中を見るには、そのパスを指定します。パスは走査したディレクトリからの相対パスか、その中を指す絶対パスで指定します。

```bash
qdu show .cache
```

#### 3. 後でもう一度記録して差分を見る

```bash
qdu snapshot
qdu diff
```

増えた場所だけを見るには、`--growth-only`を付けます。

```bash
qdu diff --growth-only
```

`qdu diff`には完全なスナップショットが2つ以上必要です。

スナップショットごとの容量の推移は`qdu history`で確認できます。

```bash
qdu history
```

## 基本用語

#### スナップショット

ある時点の容量、ファイル数、inode数、取得日時などを保存したものです。

qduはスナップショットをSQLiteデータベースとしてユーザー領域へ保存します。SQLiteサーバーを起動する必要はありません。

#### 完全なスナップショットと不完全なスナップショット

完全なスナップショットは読む権限のある場所をすべて読み取れたスナップショットです。`latest`は最新の完全なスナップショットを指します。読む権限のないディレクトリがあると`qdu snapshot`は警告を表示しますが、スナップショットは完全なままです。そのディレクトリは合計に含まれず、`qdu errors`で確認できます。

不完全なスナップショットはI/Oエラーなど権限以外の理由で読み取れなかった場所があり、走査し直せば読めるかもしれないデータが欠けたスナップショットです。`latest-any`で確認できますが、最新の完全なスナップショットの代わりにはなりません。

#### プロファイル

走査対象と設定をまとめる名前です。

```text
default      → /home/alice
project-data → /data/project
shared-home  → /home
```

`--profile`を指定しない場合は、`default`プロファイルが使われます。

1つのプロファイルは1つの走査ルートに対応します。異なる場所を管理する場合は、プロファイルを分けてください。設定で`path`を指定していないプロファイルは、すでにあるスナップショットと同じディレクトリを走査します。

#### 割り当て済み容量と見かけ上のサイズ

- 割り当て済み容量：ファイルシステム上で実際に割り当てられた容量。qduの既定値
- 見かけ上のサイズ：アプリケーションから見えるファイルサイズ

スパースファイル、圧縮、コピーオンライトなどがあると、両者は一致しないことがあります。

#### inode

ファイルやディレクトリを管理するために、ファイルシステムが使う情報です。

小さなファイルが大量にある場合、空き容量が残っていてもinodeが先に不足することがあります。

#### 運用上の許容量

実際のファイルシステム容量とは別に、管理者やチームから「このユーザーは2TBまで」と指示されることがあります。

qduではこの上限を**運用上の許容量**として指定できます。OSやストレージ側のquotaは変更せず、qduが記録した使用量と比較して、使用率や超過量を表示します。

## 複数の走査対象を管理する

1つのプロファイルは1つの走査ルートに対応します。最初のスナップショットを取得したあと、同じプロファイルで別のルートを指定するとエラーになります。

複数のルートを扱う場合は、プロファイルを分けてください。

```bash
qdu profile set home --path "$HOME"
qdu profile set data --path /data/my-project

# /home が autofs で、各ユーザーのホームが別マウントの場合
qdu profile set shared-home \
  --path /home \
  --cross-filesystems \
  --with-users
```

```bash
qdu snapshot --profile data
qdu show --profile data
qdu diff --profile data
```

登録内容を確認します。

```bash
qdu profile list
qdu profile show data
```

すでにあるプロファイルの設定を変えるときも`qdu profile set`を使います。ディレクトリが保存されているプロファイルでは`--path`を省略でき、指定した設定だけが変わります。

```bash
qdu profile set data --keep-snapshots 60
```

`qdu profile add`も`qdu profile set`と同じように使えます。

設定だけを削除する場合は、次のように実行します。

```bash
qdu profile remove data
```

設定とスナップショットの両方を削除するには、`--delete-data`を付けます。

```bash
qdu profile remove data --delete-data
```

`--delete-data` で削除したスナップショットは元に戻せません。
