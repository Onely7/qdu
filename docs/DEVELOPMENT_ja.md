# qdu 開発者向けガイド

[English](DEVELOPMENT.md) | 日本語

テスト、ビルド、ベンチマーク、リポジトリ構成を説明します。

## 開発

#### テスト

```bash
make test
```

テストスクリプトを直接実行することもできます。

```bash
tests/run_tests.sh
```

テストでは、ソースコード版に加えて、実際に配布する `dist/qdu` もビルドして実行します。

GitHub Actionsでは、LinuxとmacOS上のPython 3.10〜3.15を対象にテストします。

#### 品質チェック

```bash
python3 -m pip install -e '.[dev]'
make hooks
make lint
make format-check
pre-commit run --all-files
make secrets
```

`make hooks` は pre-commit のフックを入れます。以後のコミットごとに Ruff と Gitleaks が動きます。Gitleaks はステージした変更にアクセストークンや秘密鍵などの秘密情報があるとコミットを止め、値を伏せた検出結果を表示します。Gitleaks は初回に pre-commit がビルドし、Go がなければ Go のツールチェーンも取得します。

`make secrets` は、履歴のすべてのコミットを Gitleaks で走査します。フックを入れずにコミットした秘密情報も見つけられます。

RuffはPython 3.10を基準に、品質・セキュリティ・docstringの`D`ルールを検査します。docstringはPEP 257を基本としたGoogle Styleです。

#### GitHubの自動検査

- `test.yml`: LinuxとmacOSのテスト、compileall、Ruff、ShellCheck、pre-commitの全フック、Gitleaksによる全履歴の走査、zipapp smoke test
- `codeql.yml`: PythonコードとGitHub ActionsのワークフローのCodeQL advanced setupと`security-extended`クエリ。公開リポジトリの定期実行は、60日間活動がないとGitHubが止めます。止まった場合は`gh workflow enable codeql.yml`で再開します
- `dependabot.yml`: pip、pre-commitのフック、GitHub Actionsを毎週月曜09:00（Asia/Tokyo）に確認し、更新をまとめてPR化。DependabotのPRを90日間触らないとDependabotは一時停止し、PRのマージ・クローズ・コメントで再開します

リポジトリ設定ではDependency graph、Dependabot alerts/security updates、シークレットスキャン（プッシュ保護を含む）を有効にしてください。CodeQL default setupが有効な場合は、ワークフローと競合しないようadvanced setupへ切り替えます。

#### リリースとバージョンタグ

Release Pleaseは`main`へマージされたConventional Commitsから1つのRelease PRを継続的に更新します。Release PRには推奨バージョン、`CHANGELOG.md`、`.release-please-manifest.json`、`src/qdu/_version.py`の更新と、前回リリース以降の変更一覧が含まれます。人間がRelease PRをマージすると、`v<version>`タグとGitHub Releaseが作成されます。続けてworkflowがそのタグから`dist/qdu`をビルドし、チェックサム`qdu.sha256`とともにReleaseへ添付します。

PRはsquash mergeを使い、最終コミットをConventional Commit形式にします。

- `fix: ...`はpatch releaseを提案します。
- `feat: ...`はminor releaseを提案します。
- `feat!: ...`または`BREAKING CHANGE:` footerはbreaking releaseを提案します。1.0より前のbreaking changeは`0.x`系列を維持します。
- `docs:`、`test:`、`refactor:`、`ci:`は作業種別を記録しますが、それだけでfeature releaseを要求しません。

Release設定は`release-please-config.json`と`.release-please-manifest.json`で管理します。実行時バージョンの唯一の定義は`src/qdu/_version.py`です。`x-release-please-version`注釈をgeneric updaterが更新します。bootstrap SHAより前の既存履歴は初回Release PRの対象外です。

Actions secret `RELEASE_PLEASE_TOKEN` がなければ、workflowはリポジトリの`GITHUB_TOKEN`を使います。`GITHUB_TOKEN`で作成したPRやtagからは別のworkflowが起動しません。Release PR更新時のCIやtag作成イベントを起動する場合は、contentsとpull requestsへのwrite権限を持つfine-grained tokenをActions secret `RELEASE_PLEASE_TOKEN`として登録してください。tokenをGitへ追加してはいけません。

#### ビルドとインストール

```bash
make build
make install
make clean
```

#### ベンチマーク

```bash
make build
QDU=dist/qdu tools/benchmark.sh
```

スクリプトはディレクトリ 2,000 個とファイル 20 万個を生成します。規模は `QDU_BENCH_DIRECTORIES` と `QDU_BENCH_FILES` で変えられます。ユーザー別統計つきのスナップショット、`show`、`files`、`diff` の経過時間と最大 RSS、スナップショットファイルの大きさを出力します。v0.1.0 との最新の比較は `benchmark_results.txt` にあります。

現在のベンチマークスクリプトはGNU版 `/usr/bin/time` の `-f` を使用するため、主にLinux向けです。macOSではGNU timeを導入するか、計測部分を環境に合わせて変更してください。

## プロジェクト構成

```text
qdu/
├── README.md
├── pyproject.toml
├── Makefile
├── install.sh
├── src/qdu/                 qdu本体
│   └── commands/            表示、分析、運用、設定のコマンド責務
├── tests/                   責務別の自動テスト
├── tools/                   ビルド・ベンチマーク
├── dist/qdu                 配布用の単一実行ファイル
└── .github/workflows/       GitHub Actions
```

大規模な責務分割は振る舞いを維持する独立コミットとして先に行います。性能変更を同じコミットに混在させず、分割前後を同一環境で各5回計測し、elapsed timeとpeak RSSの中央値を比較してください。
