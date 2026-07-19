# qdu 開発者向けガイド

[English](DEVELOPMENT.md) | 日本語

テスト、ビルド、ベンチマーク、リポジトリ構成を説明します。

## 開発

#### テスト

```bash
make test
```

または：

```bash
tests/run_tests.sh
```

テストでは、ソースコード版に加えて、実際に配布する `dist/qdu` もビルドして実行します。

GitHub Actionsでは、LinuxとmacOS上のPython 3.10〜3.14を対象にテストします。

#### 品質チェック

```bash
python3 -m pip install -e '.[dev]'
make lint
make format-check
pre-commit run --all-files
```

RuffはPython 3.10を基準に、品質・セキュリティ・docstringの`D`ルールを検査します。docstringはPEP 257を基本としたGoogle Styleです。

#### GitHubの自動検査

- `test.yml`: 10環境のテスト、compileall、Ruff、ShellCheck、zipapp smoke test
- `codeql.yml`: PythonのCodeQL advanced setupと`security-extended`クエリ
- `dependabot.yml`: pipとGitHub Actionsを毎週月曜09:00（Asia/Tokyo）に確認し、更新をまとめてPR化

リポジトリ設定ではDependency graph、Dependabot alerts/security updatesを有効にしてください。CodeQL default setupが有効な場合は、ワークフローと競合しないようadvanced setupへ切り替えます。

#### リリースとバージョンタグ

Release Pleaseは、`main`へマージされたConventional Commitsから1つのRelease PRを継続的に更新します。Release PRには、推奨バージョン、`CHANGELOG.md`、`.release-please-manifest.json`、`src/qdu/_version.py`の更新と、前回リリース以降の変更一覧が含まれます。人間がRelease PRをマージすると、`v<version>`タグとGitHub Releaseが作成されます。

PRはsquash mergeを使い、最終コミットをConventional Commit形式にします。

- `fix: ...`はpatch releaseを提案します。
- `feat: ...`はminor releaseを提案します。
- `feat!: ...`または`BREAKING CHANGE:` footerはbreaking releaseを提案します。1.0より前のbreaking changeは`0.x`系列を維持します。
- `docs:`、`test:`、`refactor:`、`ci:`は作業種別を記録しますが、それだけでfeature releaseを要求しません。

Release設定は`release-please-config.json`と`.release-please-manifest.json`で管理します。実行時バージョンの唯一の定義は`src/qdu/_version.py`です。`x-release-please-version`注釈をgeneric updaterが更新します。bootstrap SHAより前の既存履歴は初回Release PRの対象外です。

workflowはリポジトリの`GITHUB_TOKEN`へフォールバックします。このtokenで作成したPRやtagからは別のworkflowが起動しません。Release PR更新時のCIやtag作成イベントを起動する場合は、contentsとpull requestsへのwrite権限を持つfine-grained tokenをActions secret `RELEASE_PLEASE_TOKEN`として登録してください。tokenをGitへ追加してはいけません。

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

大規模な責務分割は、振る舞いを維持する独立コミットとして先に行います。性能変更を同じコミットに混在させず、分割前後を同一環境で各5回計測し、elapsed timeとpeak RSSの中央値を比較してください。
