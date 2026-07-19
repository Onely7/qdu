# qdu 開発者向けガイド

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

GitHub Actionsでは、LinuxとmacOS上のPython 3.10〜3.13を対象にテストします。

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
├── tests/                   自動テスト
├── tools/                   ビルド・ベンチマーク
├── dist/qdu                 配布用の単一実行ファイル
└── .github/workflows/       GitHub Actions
```
