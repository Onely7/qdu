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

```bash
qdu show --snapshot latest-any
qdu errors --snapshot latest-any
```

#### `profile ... holds snapshots of ...`

qdu は保存済みの履歴を別の走査ルートに流用しません。`qdu doctor` では `Snapshot root` の不一致として表示されます。元の走査ルートを `--path` で指定するか、別のプロファイルを作ります。

```bash
qdu profile add another-data --path /another/data
```

#### `another qdu operation is using this profile`

同じプロファイルを変更する別のコマンドがまだ動いています。多くは定期実行の `qdu snapshot` です。エラーと `qdu doctor` にそのプロセスが表示されます。終わるまで待ってください。ロックは、プロセスが終了するとOSが解放します。異常終了した場合も同じなので、手動で削除するものはありません。

#### `qdu browse` が使えない

`fzf` があるか確認します。

```bash
command -v fzf
```

`fzf` がなくても、`qdu show` と `qdu explain` は使用できます。

#### `/home` を指定するとすぐに終わって `0B` になる

`/home` がautofsで、各ユーザーのホームディレクトリがNFSとして個別にマウントされている可能性があります。次のコマンドでマウントの状態を確認します。

```bash
findmnt -T /home
findmnt -R /home | head -n 100
stat -c 'path=%n device=%d' /home /home/"$USER"
```

`/home` と `/home/$USER` のデバイス番号が異なる場合、既定のqduは別ファイルシステムとして読み飛ばします。プロファイルを次のように作成してください。

```bash
qdu profile add shared-home \
  --path /home \
  --cross-filesystems \
  --with-users

qdu snapshot --profile shared-home
```

既存のプロファイル設定を変えずに今回だけ試すには、`--cross-filesystems` を付けて実行します。

```bash
qdu snapshot --profile home --cross-filesystems
```

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

見かけ上のサイズで比較するには、`--metric apparent` を指定します。

```bash
qdu show --metric apparent
```

#### 深いディレクトリが表示されない

`--max-depth`、`--under`、`--match`、`--min-size` と、取得時の `--record-max-depth` を確認してください。

`--record-max-depth` より深い情報は保存されていないため、必要な深さでスナップショットを取り直す必要があります。
