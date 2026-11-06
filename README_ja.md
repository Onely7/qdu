# qdu

[English](README.md) | 日本語

<img src="images/qdu.png" width=85%>

ディスクの空き容量が減ってきた。けれど、どこを片付ければよいのか分からない。

`qdu`（quick `du`）はそのようなときに使うコマンドラインツールです。名前には、容量の大きな領域でも、保存した情報を使ってディスク使用量を素早く確認できるようにする、という意味を込めています。

ディレクトリの使用量を「スナップショット」として保存するため、毎回すべてを調べ直さなくても、現在の容量ランキングや前回からの増減をあとからすぐに確認できます。qduはRustで書いた1つの実行ファイルで、複数のディレクトリを並列に走査します。

```bash
qdu snapshot   # 現在の状態を記録する
qdu show       # 容量が大きい場所を確認する
qdu diff       # 前回から増減した場所を確認する
```

`sudo`は必要ありません。一般ユーザーが読み取れる範囲を走査し、読む権限のないディレクトリは合計に含めずに記録します。記録した場所は`qdu errors`で確認できます。

## 主な機能

- ディレクトリ容量のスナップショットを保存
- 複数のディレクトリを並列に走査（NFSなどのネットワークストレージで特に速くなる）
- 前回からの増減量をランキング表示
- ディレクトリ容量の推移と1週間あたりの増加量を表示し、許容量に達する時期やディスクが満杯になる時期を予測
- 何個前か、何日前か、日付、ラベルでスナップショットを指定（`qdu diff --from 30d`など）
- ユーザーごとの容量・inode使用量を集計
- 大容量ファイルやinodeを多く使う場所を表示
- 長期間更新されていないディレクトリ・ファイルを色付きで警告
- 運用上の許容量に対する使用率と超過量を表示
- 複数の走査対象をプロファイルで管理
- autofsやNFSなど、別ファイルシステムを含む走査
- 表、TSV、JSONで出力
- 端末での走査の進捗表示と、スナップショットが使うディスク容量の表示
- スナップショットの検証、修復、保持数の管理、古いスナップショットの自動圧縮

## 動作環境

- LinuxまたはmacOS（x86_64またはarm64）
- 実行時に必要なものはありません。qduは単体で動く1つの実行ファイルです

`qdu browse`を使う場合だけ、別途[`fzf`](https://github.com/junegunn/fzf)が必要です。

## インストール

qduは実行ファイルをインストールするパッケージとしてPyPIで公開しています。[uv](https://docs.astral.sh/uv/)があれば、インストールせずに試せます。

```bash
uvx qdu --help
```

使い続ける場合や、定期的にスナップショットを取る場合は、ツールとしてインストールします。`pipx`や`pip`でも同じようにインストールできます。

```bash
uv tool install qdu    # または pipx install qdu、python3 -m pip install --user qdu
```

インストールできたことを確認します。

```bash
command -v qdu
qdu --version
```

`command -v qdu`で何も表示されない場合は、現在のターミナルで`PATH`を追加してください。

```bash
export PATH="$HOME/.local/bin:$PATH"
```

Pythonがない環境では、[最新のリリース](https://github.com/Onely7/qdu/releases/latest)から、使っている環境向けの実行ファイルをダウンロードします。次の例ではx86_64のLinux向けの実行ファイルを`~/.local/bin`へ展開します。

```bash
mkdir -p "$HOME/.local/bin"
curl -fL https://github.com/Onely7/qdu/releases/latest/download/qdu-x86_64-unknown-linux-gnu.tar.gz \
  | tar -xz -C "$HOME/.local/bin" qdu
```

リリースには次の6つのアーカイブと、それらのチェックサムを記した`SHA256SUMS`を添付しています。

| アーカイブ | 対象 |
|---|---|
| `qdu-x86_64-unknown-linux-gnu`、`qdu-aarch64-unknown-linux-gnu` | glibc 2.17以降を使うLinux。ほとんどのディストリビューションが当てはまります |
| `qdu-x86_64-unknown-linux-musl`、`qdu-aarch64-unknown-linux-musl` | Alpineなど、glibcを使わないLinux。静的リンクしてありますが、muslのメモリアロケータはスレッドが多いと遅くなるので、glibc版が動く環境ではglibc版を使ってください |
| `qdu-x86_64-apple-darwin`、`qdu-aarch64-apple-darwin` | IntelとApple siliconのmacOS |

各アーカイブには実行ファイルの`qdu`のほか、`LICENSE`と`README.md`が入っています。

ソースからビルドする方法は、[導入ガイド](docs/GETTING_STARTED_ja.md)を参照してください。

## まずはコマンドを試す

### 1. 現在の状態を記録する

```bash
qdu snapshot
```

何も指定しない場合は、自分のホームディレクトリを走査します。別のディレクトリを記録する場合は`--path`で指定します。設定もスナップショットもない新しいプロファイルでは、指定したディレクトリと、`--exclude`や`--with-users`などの設定がそのプロファイルに保存されます。次回からは指定しなくても、同じディレクトリを同じ設定で走査します。

```bash
qdu snapshot --path /data/my-project
```

設定が保存されたあとに付けた`--with-users`などのオプションは、その回の走査だけに使われます。次回以降も使うには`--save`も付けます。

qduが保存するのは、容量、ファイル数、inode数、取得日時などの情報です。ファイルの内容そのものはコピーしません。

### 2. 容量ランキングを見る

```bash
qdu show
```

深さ2までの上位30件を見るには、`--max-depth`と`--top`を指定します。

```bash
qdu show --max-depth 2 --top 30
```

`--max-depth`は`-L`、`--top`は`-n`と短く書けます。コマンドを省略してオプションだけを指定すると`qdu show`として動くので、次のコマンドも同じ表示になります。

```bash
qdu -L 2 -n 30
```

### 3. 後でもう一度記録して差分を見る

```bash
qdu snapshot
qdu diff
```

増えた場所だけを見るには、`--growth-only`を付けます。

```bash
qdu diff --growth-only
```

`qdu diff`には完全なスナップショットが2つ以上必要です。

### 4. 定期的に記録する

cronを使える環境では、毎週月曜03:15にスナップショットを取るよう登録できます。

```bash
qdu schedule --install
```

`qdu schedule`を使う前に、`uv tool install`などでqduをインストールしてください。cronには置き場所の変わらない実行ファイルが必要ですが、`uvx`で動かすqduはuvのキャッシュにあり、uvが削除することがあります。

詳しくは[運用と保守](docs/OPERATIONS_ja.md)を参照してください。

## 複数の場所を管理する

走査対象と設定は**プロファイル**という名前で分けられます。

```bash
qdu profile set project-data --path /data/my-project

qdu snapshot --profile project-data
qdu show --profile project-data
qdu diff --profile project-data
```

`--profile`を指定しない場合は、`default`プロファイルが使われます。

プロファイル名には英数字、`.`、`_`、`-`だけを使用でき、先頭は英数字、長さは64文字までです。絶対パス、`..`、パス区切り文字を含む名前は、状態ディレクトリ外へのアクセスを防ぐため拒否されます。

設定もスナップショットもないプロファイル名を`--path`なしで指定すると、`default`以外はエラーになり、qduは名前がいちばん近いプロファイルを示します。そのため、名前を打ち間違えてもホームディレクトリを走査することはありません。

`/home`がautofsで、各ユーザーのホームが別のNFSマウントになっている環境では、別ファイルシステムを含める設定が必要です。

```bash
qdu profile set shared-home \
  --path /home \
  --cross-filesystems \
  --with-users
```

詳しくは[スナップショットと走査範囲](docs/SNAPSHOTS_AND_SCANNING_ja.md)を参照してください。

## よく使う分析

### 1つのディレクトリの中身と増減を見る

```bash
qdu show data/models
qdu diff data/models --from 30d
```

パスを指定すると、そのディレクトリの中だけを対象にします。パスは走査対象のディレクトリからの相対パスか、その中を指す絶対パスで指定します。`--from 30d`を付けると、30日以上前に取った完全なスナップショットのうち、いちばん新しいものと比べます。

### 容量の推移と満杯になる時期を見る

```bash
qdu history data
```

スナップショットごとの容量と増減を表示し、1週間あたりの増加量と、その増え方が続いた場合にディスクが満杯になる時期を示します。運用上の許容量を設定したプロファイルでパスを省略すると、許容量に達する時期も示します。

### 長期間更新されていない場所を探す

```bash
qdu show --stale
qdu show --stale-only 90 --max-depth 4 --top 100
```

### 運用上の許容量を確認する

```bash
qdu show --capacity-limit 2TiB
```

共有領域から自分の使用量だけを確認するには、`--capacity-user`に自分のユーザー名を指定します。

```bash
qdu show \
  --profile shared-home \
  --capacity-limit 2TiB \
  --capacity-user "$USER"
```

### 大容量ファイルやinodeを確認する

```bash
qdu files --top 30
qdu show --metric inodes --max-depth 3 --top 30
```

### スナップショットにラベルを付ける

```bash
qdu label before-cleanup

# 後でもう一度記録して、ラベルを付けたスナップショットと比べる
qdu snapshot
qdu diff --from @before-cleanup
```

`qdu label`は最新の完全なスナップショットにラベルを付けます。ほかのスナップショットに付けるには`--snapshot`で選びます。ラベルを付けたスナップショットは`@ラベル名`で指定でき、保持数の上限を超えても削除されません。スナップショットを取るときに付けるには、`qdu snapshot --label before-cleanup`のように指定します。

詳しい使い方は[分析ガイド](docs/ANALYSIS_GUIDE_ja.md)を参照してください。

## データの保存場所

設定ファイルは次の場所にあります。

```text
${XDG_CONFIG_HOME:-$HOME/.config}/qdu/config.ini
```

スナップショットと履歴は次の場所に保存されます。

```text
${XDG_STATE_HOME:-$HOME/.local/state}/qdu/profiles/<profile>/
```

保存内容や設定ファイルの詳細は、[設定と保存場所](docs/CONFIGURATION_AND_STORAGE_ja.md)を参照してください。

## ドキュメント

| 文書 | 内容 |
|---|---|
| [ドキュメント一覧](docs/README_ja.md) | 目的に合う文書を探す |
| [導入ガイド](docs/GETTING_STARTED_ja.md) | インストール、基本用語、最初の操作 |
| [スナップショットと走査範囲](docs/SNAPSHOTS_AND_SCANNING_ja.md) | 権限、除外、NFS、autofs、マウントポイント |
| [分析ガイド](docs/ANALYSIS_GUIDE_ja.md) | 容量、差分、推移、放置候補、許容量、ユーザー、ファイル、inode |
| [設定と保存場所](docs/CONFIGURATION_AND_STORAGE_ja.md) | 設定ファイル、プロファイル、状態データ |
| [運用と保守](docs/OPERATIONS_ja.md) | 検証、修復、ラベル、圧縮、しきい値、定期実行、終了コード |
| [トラブルシューティング](docs/TROUBLESHOOTING_ja.md) | よくあるエラーと確認手順 |
| [コマンドリファレンス](docs/COMMAND_REFERENCE_ja.md) | 全コマンドと各オプションの既定値 |
| [開発者向けガイド](docs/DEVELOPMENT_ja.md) | テスト、ビルド、ベンチマーク、構成 |
| [tmuxによる週次実行](docs/QDU_TMUX_WEEKLY_SNAPSHOT_GUIDE_ja.md) | cronを使えない環境での定期実行 |

英語版は[English documentation index](docs/README.md)から参照できます。

## コマンド一覧

| コマンド | 役割 |
|---|---|
| `qdu snapshot` | スナップショットを取得 |
| `qdu show` | 容量ランキングを表示 |
| `qdu diff` | スナップショットを比較 |
| `qdu history` | 容量の推移と満杯になる時期を表示 |
| `qdu list` | スナップショットの一覧を表示 |
| `qdu users` | ユーザー別ランキングを表示 |
| `qdu errors` | 読めなかった場所を表示 |
| `qdu files` | 大容量ファイルを表示 |
| `qdu browse` | `fzf`でディレクトリを選ぶ |
| `qdu check` | 容量・使用率のしきい値を判定 |
| `qdu schedule` | cronで定期的にスナップショットを取得 |
| `qdu compact` | 古いスナップショットを圧縮 |
| `qdu verify` | スナップショットを検証 |
| `qdu doctor` | 実行環境を診断 |
| `qdu repair` | インデックスを再構築 |
| `qdu label` | スナップショットにラベルを付ける |
| `qdu forget` | スナップショットを削除 |
| `qdu profile` | プロファイルを管理 |
| `qdu config` | 設定ファイルの場所を表示、ひな形を作成 |
| `qdu completions` | シェルの補完スクリプトを出力 |

全オプションは`--help`または[コマンドリファレンス](docs/COMMAND_REFERENCE_ja.md)で確認できます。

```bash
qdu --help
qdu snapshot --help
qdu show --help
qdu diff --help
```
