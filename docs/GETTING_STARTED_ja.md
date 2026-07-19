# qdu 導入ガイド

[English](GETTING_STARTED.md) | 日本語

インストールから最初の差分確認までを説明します。

## 動作環境

- LinuxまたはmacOS
- Python 3.10以上
- 追加のPythonパッケージは不要

Pythonのバージョンは、次のコマンドで確認できます。

```bash
python3 --version
```

`qdu browse`を使う場合だけ、別途[`fzf`](https://github.com/junegunn/fzf)が必要です。

## インストール

#### 配布された`qdu`を使う

配布用の実行ファイルは`dist/qdu`です。一般ユーザーが書き込める`~/.local/bin`へ配置します。

```bash
mkdir -p "$HOME/.local/bin"
install -m 755 ./dist/qdu "$HOME/.local/bin/qdu"
```

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

#### インストールスクリプトを使う

リポジトリのルートで実行します。

```bash
./install.sh
```

既定では`~/.local/bin/qdu`へインストールされます。別の場所へ入れる場合は、`QDU_INSTALL_DIR`を指定します。

```bash
QDU_INSTALL_DIR="$HOME/bin" ./install.sh
```

#### 配布用の単一実行ファイルをビルドする

```bash
python3 tools/build_zipapp.py
```

生成先：

```text
dist/qdu
```

仮想環境や`pip install`は必要ありません。

## まずは3つのコマンドを試す

#### 1. 現在の状態を記録する

```bash
qdu snapshot
```

何も指定しない場合は、自分のホームディレクトリを走査します。

qduが保存するのは、容量、ファイル数、inode数、取得日時などの情報です。ファイルの内容そのものはコピーしません。

#### 2. 容量ランキングを見る

```bash
qdu show
```

深さ2までの上位30件を見る場合：

```bash
qdu show --max-depth 2 --top 30
```

引数なしの`qdu`も、`qdu show`と同じ動作です。

#### 3. 後でもう一度記録し、差分を見る

```bash
qdu snapshot
qdu diff
```

増えた場所だけを見る場合：

```bash
qdu diff --growth-only
```

`qdu diff`には、完全なスナップショットが2つ以上必要です。

## 基本用語

#### スナップショット

ある時点の容量、ファイル数、inode数、取得日時などを保存したものです。

qduは、スナップショットをSQLiteデータベースとしてユーザー領域へ保存します。SQLiteサーバーを起動する必要はありません。

#### プロファイル

走査対象と設定をまとめる名前です。

```text
default      → /home/alice
project-data → /data/project
shared-home  → /home
```

`--profile`を指定しない場合は、`default`プロファイルが使われます。

1つのプロファイルは、1つの走査ルートに対応します。異なる場所を管理する場合は、プロファイルを分けてください。

#### 割り当て済み容量と見かけ上のサイズ

- **割り当て済み容量**：ファイルシステム上で実際に割り当てられた容量。qduの既定値
- **見かけ上のサイズ**：アプリケーションから見えるファイルサイズ

スパースファイル、圧縮、コピーオンライトなどがあると、両者は一致しないことがあります。

#### inode

ファイルやディレクトリを管理するために、ファイルシステムが使う情報です。

小さなファイルが大量にある場合、空き容量が残っていてもinodeが先に不足することがあります。

#### 運用上の許容量

実際のファイルシステム容量とは別に、管理者やチームから「このユーザーは2TBまで」と指示されることがあります。

qduでは、この上限を**運用上の許容量**として指定できます。OSやストレージ側のquotaは変更せず、qduが記録した使用量と比較して、使用率や超過量を表示します。

## 複数の走査対象を管理する

1つのプロファイルは1つの走査ルートに対応します。最初のスナップショットを取得したあと、同じプロファイルで別のルートを指定するとエラーになります。

複数のルートを扱う場合は、プロファイルを分けてください。

```bash
qdu profile add home --path "$HOME"
qdu profile add data --path /data/my-project

# /home が autofs で、各ユーザーのホームが別マウントの場合
qdu profile add shared-home \
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

設定だけを削除：

```bash
qdu profile remove data
```

設定とスナップショットを削除：

```bash
qdu profile remove data --delete-data
```

`--delete-data` で削除したスナップショットは元に戻せません。
