# トラブルシューティング

[English](TROUBLESHOOTING.md) | 日本語

qduの実行中に起こりやすい問題と確認手順をまとめています。

## よくある問題

#### `qdu: command not found`

```bash
export PATH="$HOME/.local/bin:$PATH"
```

`~/.local/bin/qdu` があるかも確認します。

```bash
ls -l "$HOME/.local/bin/qdu"
```

#### `no snapshots are available`

```bash
qdu snapshot
```

プロファイルを使っている場合は、取得と表示で同じ名前を指定します。

#### `no complete snapshot is available`

これまでに取得したスナップショットはどれも、権限が足りないこととは別の失敗（I/O エラーなど）があって不完全です。最新の不完全な結果と、記録されたエラーを確認します。

```bash
qdu show --snapshot latest-any
qdu errors --snapshot latest-any
```

不完全になる原因は、種類が `error` のエラーです。エラーの原因を取り除くか、その場所を除外するか、走査するマウントの範囲を狭めてから、スナップショットを取り直してください。種類が `denied` の場所があっても、スナップショットは不完全になりません。

#### 最新のスナップショットが不完全だという注記が出る

コマンドが最新の完全なスナップショット（`latest`）を読むとき、それより新しい不完全なスナップショットがあると、qdu は標準エラーに注記を出します。新しいほうは `--snapshot latest-any` で読めます。不完全になった理由は `qdu errors` で確認できます。

#### `unknown profile`

指定したプロファイルには設定もスナップショットもありません。メッセージには、既知のプロファイルのうち最も近い名前が示されます。`qdu profile set 名前 --path DIR` でプロファイルを作るか、`qdu snapshot --profile 名前 --path DIR` で最初のスナップショットを取得してください。

#### `profile ... holds snapshots of ...`

qdu は保存済みの履歴を別の走査ルートに流用しません。1つのプロファイルのスナップショットは、すべて同じディレクトリのものです。このエラーは、`--path` かプロファイルに設定した `path` が、既存のスナップショットとは別のディレクトリを指すときに出ます。設定した `path` が違う場合は、`qdu doctor` でも `Snapshot root` の不一致として表示されます。元の走査ルートを `--path` で指定するか、`qdu profile set 名前 --path 元の走査ルート` で設定を直すか、別のプロファイルを作ります。

```bash
qdu profile set another-data --path /another/data
```

#### `another qdu operation is using this profile`

同じプロファイルを変更する別のコマンドがまだ動いています。多くは定期実行の `qdu snapshot` です。エラーと `qdu doctor` にそのプロセスが表示されます。終わるまで待ってください。ロックは、プロセスが終了するとOSが解放します。異常終了した場合も同じなので、手動で削除するものはありません。

#### `qdu browse` が使えない

`fzf` があるか確認します。

```bash
command -v fzf
```

`fzf` がなくても、`qdu show PATH` と `qdu diff PATH` は使用できます。

#### `/home` を指定するとすぐに終わって `0B` になる

`/home` がautofsで、各ユーザーのホームディレクトリがNFSとして個別にマウントされている可能性があります。次のコマンドでマウントの状態を確認します。

```bash
findmnt -T /home
findmnt -R /home | head -n 100
stat -c 'path=%n device=%d' /home /home/"$USER"
```

`/home` と `/home/$USER` のデバイス番号が異なる場合、既定のqduは別ファイルシステムとして読み飛ばします。プロファイルを次のように作成してください。

```bash
qdu profile set shared-home \
  --path /home \
  --cross-filesystems \
  --with-users

qdu snapshot --profile shared-home
```

既存のプロファイル設定を変えずに今回だけ試すには、`--cross-filesystems` を付けて実行します。

```bash
qdu snapshot --profile home --cross-filesystems
```

#### 走査ルートが automount のマウントポイントだという警告が出る

automount のマップは走査で読んだ時点でマウントされているエントリーしか一覧に出しません。そのため、しばらく誰も使っていないホームディレクトリなどがスナップショットから漏れます。[スナップショットと走査](SNAPSHOTS_AND_SCANNING_ja.md)の手順でエントリーを開いてから、スナップショットを取り直してください。

反対に、プロファイルで別ファイルシステムへの走査を有効にしていても、今回だけ境界を越えたくないときは `--one-file-system` を付けます。

```bash
qdu snapshot --profile home --one-file-system
```

#### `du` やファイルマネージャーと容量が違う

次の違いが影響します。

- 割り当て済み容量と見かけ上のサイズ
- 除外パターン
- 読み取り権限
- 別ファイルシステム
- ハードリンクの重複除外
- スナップショット取得後の変更

読む権限のないディレクトリは合計に含まれず、`qdu errors` に表示されます。

見かけ上のサイズで比較するには、`--metric apparent` を指定します。

```bash
qdu show --metric apparent
```

#### 深いディレクトリが表示されない

表示するディレクトリ（`PATH`）、`--max-depth`、`--match`、`--min-size` と、取得時の `--record-max-depth` を確認してください。

`--record-max-depth` より深い情報は保存されていないため、必要な深さでスナップショットを取り直す必要があります。

#### `cannot read snapshot` または終了コード 4

スナップショットのファイルが壊れているか、SQLite のデータベースではないか、この qdu が読めないスキーマかのどれかです。`qdu verify --all` で該当するファイルを探し、`qdu repair --dry-run` でインデックスをどう作り直すかを確認してください。
