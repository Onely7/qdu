# qdu 開発者向けガイド

[English](DEVELOPMENT.md) | 日本語

テスト、ビルド、ベンチマーク、リポジトリ構成を説明します。

## 開発

#### 必要なもの

qduはRustで書かれており、実行時に必要なものはありません。開発には次のものを使います。

- Rust 1.85以降と、Clippy、rustfmt（[rustup](https://rustup.rs/)などで入れます）
- 同梱のSQLiteをビルドするためのCコンパイラ
- コミット時のフックに使う[pre-commit](https://pre-commit.com/)と、PyPI向けのパッケージをビルドする[uv](https://docs.astral.sh/uv/)または[maturin](https://www.maturin.rs/)

```bash
rustup toolchain install stable --component clippy,rustfmt
make hooks
```

#### テスト

```bash
make test
```

`make test`は`cargo test --locked`を実行します。単体テストはテスト対象のコードと同じ`src/`のファイルにあります。`tests/`のテストはビルドした`qdu`を実際に実行し、専用の`HOME`、`XDG_CONFIG_HOME`、`XDG_STATE_HOME`の下にできた`index.json`とSQLiteファイルを直接読んで確かめます。コマンドの種類ごとに`snapshots.rs`、`analysis.rs`、`operations.rs`、`scanner.rs`に分かれ、`formats.rs`はスクリプトが前提とするTSVの見出し行、JSONのキー、表の列名を、`project.rs`はドキュメントのリンクとリリース設定を確かめます。`crontab`と`fzf`は`PATH`に置いた代わりのスクリプトで置き換えます。rootで実行したときの権限のテスト、UTF-8ではないファイル名を作れないファイルシステムでのテスト、Linux以外でのマウント境界のテストは、実行できないため自動的に飛ばします。

デバッグビルドは`QDU_DEBUG_IO_ERROR`に書いたディレクトリの読み取りをI/Oエラーで失敗させるので、テストで不完全なスナップショットを作れます。ディレクトリは走査ルートからの相対パスで書き、複数のときは`:`で区切ります。この変数を使うテストは、`cargo test`が既定で作るデバッグビルドでだけ実行します。リリースビルドはこの変数を無視します。

GitHub ActionsではLinuxとmacOSでテストします。

#### 品質チェック

```bash
make lint      # rustfmt の確認と、警告をエラーとして扱う Clippy
make format    # cargo fmt
shellcheck tools/benchmark.sh
pre-commit run --all-files
make secrets
```

`make hooks` は pre-commit のフックを入れます。以後のコミットごとに rustfmt による書式の確認と Gitleaks が動きます。Gitleaks はステージした変更にアクセストークンや秘密鍵などの秘密情報があるとコミットを止め、値を伏せた検出結果を表示します。Gitleaks は初回に pre-commit がビルドし、Go がなければ Go のツールチェーンも取得します。

`make secrets` は履歴のすべてのコミットを Gitleaks で走査します。フックを入れずにコミットした秘密情報も見つけられます。

#### GitHubの自動検査

- `test.yml`: LinuxとmacOSでのrustfmt、Clippy、テスト、`Cargo.toml`に書いた最低バージョンであるRust 1.85でのビルド、ShellCheck、pre-commitの全フック、Gitleaksによる全履歴の走査、wheelとソース配布物のビルド、wheelをインストールして`qdu --version`を実行する確認
- `codeql.yml`: RustコードとGitHub ActionsのワークフローのCodeQL advanced setupと`security-extended`クエリ。公開リポジトリの定期実行は、60日間活動がないとGitHubが止めます。止まった場合は`gh workflow enable codeql.yml`で再開します
- `dependabot.yml`: Cargoの依存関係、pre-commitのフック、GitHub Actionsを毎週月曜09:00（Asia/Tokyo）に確認し、更新をまとめてPR化。DependabotのPRを90日間触らないとDependabotは一時停止し、PRのマージ・クローズ・コメントで再開します

チェックの名前は`ubuntu-latest / Rust`、`macos-latest / Rust`、`Minimum supported Rust`、`Static and distribution checks`、`Analyze Rust`、`Analyze Actions`です。ブランチのルールセットで必須のチェックを指定する場合は、これらの名前を指定してください。

リポジトリ設定ではDependency graph、Dependabot alerts/security updates、シークレットスキャン（プッシュ保護を含む）を有効にしてください。CodeQL default setupが有効な場合は、ワークフローと競合しないようadvanced setupへ切り替えます。

#### リリースとバージョンタグ

Release Pleaseは`main`へマージされたConventional Commitsから1つのRelease PRを継続的に更新します。Release PRには推奨バージョン、`CHANGELOG.md`、`.release-please-manifest.json`、`Cargo.toml`、`Cargo.lock`の更新と、前回リリース以降の変更一覧が含まれます。人間がRelease PRをマージすると、`v<version>`タグと、下書きのGitHub Releaseが作成されます。続けてworkflowがそのタグから次のものをビルドします。

- Linux（manylinux2014とmusllinux、x86_64とaarch64）とmacOS（x86_64とarm64）向けのwheelと、ソース配布物。これらはPyPIへ公開します
- Linux（glibcとmusl）向けとmacOS向けの実行ファイルを、それぞれ`LICENSE`と`README.md`とともに`qdu-<target>.tar.gz`という名前でまとめたもの。これらは`SHA256SUMS`とともにGitHub Releaseへ添付し、そのあとでリリースを公開します

何かを公開する前に、スモークテストとしてLinuxの各ビルドとApple silicon向けのビルドで小さなディレクトリツリーのスナップショットを取ります。PyPIへのアップロードもこのスモークテストのあとです。

PRはsquash mergeを使い、最終コミットをConventional Commit形式にします。

- `fix: ...`はpatch releaseを提案します。
- `feat: ...`はminor releaseを提案します。
- `feat!: ...`または`BREAKING CHANGE:` footerはbreaking releaseを提案します。1.0より前のbreaking changeは`0.x`系列を維持します。
- `docs:`、`test:`、`refactor:`、`ci:`は作業種別を記録しますが、それだけでfeature releaseを要求しません。

Release設定は`release-please-config.json`と`.release-please-manifest.json`で管理します。バージョンを定義するのは`Cargo.toml`だけで、wheelと`qdu --version`はそこから読みます。`draft`と`force-tag-creation`を設定しているので、リリースはタグとともに下書きとして作られ、実行ファイルを添付するまで公開されません。

Actions secret `RELEASE_PLEASE_TOKEN` がなければ、workflowはリポジトリの`GITHUB_TOKEN`を使います。`GITHUB_TOKEN`で作成したPRやtagからは別のworkflowが起動しません。Release PR更新時のCIやtag作成イベントを起動する場合は、contentsとpull requestsへのwrite権限を持つfine-grained tokenをActions secret `RELEASE_PLEASE_TOKEN`として登録してください。tokenをGitへ追加してはいけません。

#### PyPIへの公開

workflowはPyPIのtrusted publishingで公開するので、PyPIのトークンをGitHubに保存しません。最初のリリースの前に、次の設定を一度だけします。

1. PyPIで「Your account」→「Publishing」を開き、pending publisherを追加します。project nameは`qdu`、ownerは`Onely7`、repositoryは`qdu`、workflowは`release-please.yml`、environmentは`pypi`です。
2. GitHubのSettings → Environmentsで、`pypi`という名前のenvironmentを作ります。ここでrequired reviewersを指定すると、アップロードのたびに承認を待ちます。

公開の前に設定していなかった場合など、アップロードに失敗したときは、原因を直してから、そのworkflowの実行で失敗したジョブを再実行します。

#### ビルドとインストール

```bash
make build      # target/release/qdu
make install    # ~/.local/bin へコピー。PREFIX=/opt/qdu でインストール先を変える
make wheel      # PyPI向けのパッケージを dist/ に作る
make clean
```

PyPI向けのパッケージはリリース用の実行ファイルを入れ、Pythonのコードを含まないwheelです。maturinが`pyproject.toml`の`bindings = "bin"`に従ってビルドし、バージョンは`Cargo.toml`から取ります。`make wheel`は`uvx`経由でmaturinを実行します。インストール済みのmaturinを使う場合は`MATURIN=maturin`を指定します。

#### 走査の仕組み

走査ではディレクトリ1つを1つのタスクとしてスレッドプールで処理します。スレッド数は`--threads`、なければ`QDU_THREADS`、どちらもなければCPUの数（上限16）です。ディレクトリには見つけた順に番号を振るので、どのディレクトリも親より大きい番号を持ちます。走査のあと、番号の大きい順に1回たどり、各ディレクトリの合計を親へ足します。ハードリンクのあるファイルは、パスを並べたときに先頭に来るリンクで1回だけ数え、行はパスの順に書き込みます。そのため、スナップショットの内容はスレッド数やスレッドの実行順に左右されません。

使うメモリはディレクトリの数に比例し、1つあたり約200バイト、`--with-users`を付けるとさらに約70バイトです。ハードリンクについてはリンクの数ではなくハードリンクのあるファイルの数に比例します。パスは行を書き込むときに、SQLiteが保つ順に1つずつ組み立てるので、全パスの一覧は持ちません。この順は一度だけ並べ替えて求め、ディレクトリの行にもユーザーごとの行にも使い回します。

ディレクトリは親のディレクトリを開いたまま、親のファイルディスクリプタからの相対で開くので、フルパスの長さには左右されません。同時に開いておくディレクトリの数は、ファイルディスクリプタの上限（`RLIMIT_NOFILE`）をもとに決めた数までに限られます。それを超えたディレクトリはパスで開き、パスが長すぎるときはパスの要素を1つずつたどって開きます。開いたままにしておくディレクトリについてはファイルディスクリプタだけを持ち、ディレクトリストリームとそのバッファは持ちません。ディレクトリの中身は開いたディレクトリからの相対で`fstatat`を使って調べます。標準ライブラリがLinuxで使う`statx`と違い、`fstatat`はautomountを起こしません。ディレクトリは最後の要素のシンボリックリンクをたどらずに開き、親を読んだときのデバイス番号とinode番号と照合します。デバイス番号が違うものを受け入れるのは、マウントポイントのように、開いたディレクトリの`..`がまだ親を指している間だけです。そのため、走査中にディレクトリが入れ替わっても、走査ルートの外へは出ません。ディレクトリはどれもデバイス番号とinode番号の組を記録してから読むので、bind mountやマウントのループで同じディレクトリに再びたどり着いても、読むのは1回だけです。btrfsとoverlayfsはどのディレクトリのリンク数も1と報告するため、リンク数では記録が必要なディレクトリを見分けられません。合計は桁あふれせず、上限の値で止まります。走査中の失敗は、途中で消えたもの、読み取りを拒否されたもの、それ以外のエラーに分けます（[スナップショットと走査範囲](SNAPSHOTS_AND_SCANNING_ja.md)を参照）。

スナップショットのデータベースはスキーマ4で書き込み、Python版の0.2.0が書いたスキーマ3と、0.1.0が書いたスキーマ2も読めます。SIGINT、SIGTERM、SIGHUPを受けると、専用のスレッドが書きかけのスナップショットのデータベースや展開中のコピーなどを削除し、終了コード「128＋シグナル番号」で終了します。

#### ベンチマーク

```bash
make bench
QDU=/path/to/another/qdu tools/benchmark.sh
```

スクリプトはディレクトリ 2,000 個とファイル 20 万個を生成します。規模は `QDU_BENCH_DIRECTORIES` と `QDU_BENCH_FILES` で変えられます。ユーザー別統計つきのスナップショット、`show`、`files`、`diff` の経過時間と最大 RSS、スナップショットファイルの大きさを出力します。最後のPython版である0.2.0との最新の比較は `benchmark_results.txt` にあります。

現在のベンチマークスクリプトはGNU版 `/usr/bin/time` の `-f` を使用するため、主にLinux向けです。macOSではGNU timeを導入するか、計測部分を環境に合わせて変更してください。

## プロジェクト構成

```text
qdu/
├── README.md
├── Cargo.toml、Cargo.lock
├── pyproject.toml           maturinによるPyPI向けのパッケージ設定
├── Makefile
├── src/                     qdu本体
│   ├── main.rs              入口と終了コード
│   ├── cli.rs               コマンドラインの文法
│   ├── commands/            コマンドの実装（1コマンド1ファイル）
│   ├── scan/                スナップショットの並列走査（walk、tree、write）
│   ├── traverse.rs          スナップショットの走査と qdu files --live の走査が共有する処理
│   ├── selector.rs          スナップショットの指定方法
│   ├── report.rs            表、TSV、JSONで共有する、レポートごとのレイアウト
│   └── ...                  設定、表示、保存、単位、除外パターンなど
├── tests/                   qduを実行して確かめるテスト
├── tools/benchmark.sh       ベンチマーク
└── .github/workflows/       GitHub Actions
```

大規模な責務分割は振る舞いを維持する独立コミットとして先に行います。性能変更を同じコミットに混在させず、分割前後を同一環境で各5回計測し、elapsed timeとpeak RSSの中央値を比較してください。

TSVの見出し行やJSONのキーを変えるときは、`tests/formats.rs`も更新してください。キーを削除したり意味を変えたりするときは、`src/report.rs`の`FORMAT_VERSION`も上げてください。
