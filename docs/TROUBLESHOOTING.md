# トラブルシューティング

qduの実行中に起こりやすい問題と確認手順をまとめています。

## よくある問題

#### `qdu: command not found`

```bash
export PATH="$HOME/.local/bin:$PATH"
```

ファイルも確認します。

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

#### `profile ... is already bound to ...`

別の走査ルートには、別のプロファイルを作ります。

```bash
qdu profile add another-data --path /another/data
```

#### `qdu browse` が使えない

`fzf` があるか確認します。

```bash
command -v fzf
```

`fzf` がなくても、`qdu show` と `qdu explain` は使用できます。

#### `/home` を指定すると、すぐに終わって `0B` になる

`/home` がautofsで、各ユーザーのホームディレクトリがNFSとして個別にマウントされている可能性があります。確認します。

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

既存のプロファイル設定を変えず、今回だけ試す場合：

```bash
qdu snapshot --profile home --cross-filesystems
```

反対に、プロファイルでは有効だが今回だけ境界を越えたくない場合：

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

見かけ上のサイズで比較する場合：

```bash
qdu show --metric apparent_bytes
```

#### 深いディレクトリが表示されない

`--max-depth`、`--under`、`--match`、`--min-size` と、取得時の `--record-max-depth` を確認してください。

`--record-max-depth` より深い情報は保存されていないため、必要な深さでスナップショットを取り直す必要があります。
