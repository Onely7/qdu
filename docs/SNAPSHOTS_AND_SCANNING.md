# スナップショットと走査範囲

[English](SNAPSHOTS_AND_SCANNING_en.md) | 日本語

スナップショット取得時の設定、読み取り権限、除外パターン、マウントポイントの扱いを説明します。

## スナップショットを取得する

基本形：

```bash
qdu snapshot
```

名前付きプロファイルを使う場合：

```bash
qdu snapshot --profile data
```

ユーザー別統計や大容量ファイル候補も保存する例：

```bash
qdu snapshot \
  --profile data \
  --with-users \
  --user-max-depth 3 \
  --file-top 2000 \
  --exclude '.cache' \
  --exclude 'node_modules'
```

コマンドラインで指定しなかった項目には、プロファイル設定または既定値が使われます。全オプションと既定値は、後半の[`qdu snapshot`オプション一覧](COMMAND_REFERENCE.md#qdu-snapshot)で確認できます。

#### `--record-max-depth` の注意

このオプションは、スナップショットへ保存するディレクトリ行の深さを制限します。深い場所の容量も親の合計には含まれますが、制限より深いディレクトリは個別に表示できません。

容量計算のために配下は走査するため、走査時間が大幅に短くなるとは限りません。

#### 読み取れない場所がある場合

一般ユーザーでは、一部のディレクトリを読めないことがあります。qduは読めた範囲を**不完全なスナップショット**として保存します。

- 終了コード：`3`
- 状態：`incomplete`
- `latest-any`：更新される
- 最新の完全な結果を指す `latest`：更新されない

不完全な結果を見る場合：

```bash
qdu show --snapshot latest-any
qdu errors --snapshot latest-any
```

通常はsudoを付けず、一般ユーザーとして実行してください。sudoで実行すると、状態ファイルがroot所有になり、普段のユーザーから扱いにくくなることがあります。

#### 走査範囲とマウントポイント

既定では、走査ルートと同じファイルシステム内だけを走査します。配下に別のディスク、NFS、autofsなどがマウントされていても、その中へは入りません。意図せず巨大な共有領域まで走査することを防ぐためです。

別のファイルシステムも含める場合は、明示的に `--cross-filesystems` を指定します。

```bash
qdu snapshot --profile shared-home --cross-filesystems
```

プロファイルへ保存して毎回有効にすることもできます。

```bash
qdu profile add shared-home \
  --path /home \
  --cross-filesystems \
  --with-users

qdu snapshot --profile shared-home
```

例えば `/home` 自体が `autofs` で、`/home/alice` や `/home/bob` が個別のNFSマウントになっている環境では、既定のままだと各ホームディレクトリを走査しません。その場合、結果が `Directories 1`、`Files 0`、`Allocated 0B` になることがあります。`--cross-filesystems` を付けると、それらのマウント先へ入ります。

ただし、一般ユーザーが読み取れないホームディレクトリは走査できません。読める範囲は保存され、結果は `incomplete` になります。sudoを使わず、`qdu errors --snapshot latest-any` で読めなかった場所を確認してください。

> [!CAUTION]
> `--cross-filesystems` は、走査ルート配下にある**すべての別ファイルシステム**を対象にします。NFSや応答の遅いマウントが含まれると、取得に時間がかかることがあります。まず対象範囲を `findmnt -R PATH` などで確認してください。

その他の規則：

- シンボリックリンクはリンク自体を数え、リンク先はたどりません
- ハードリンクはデバイス番号とinode番号で重複除外します
- 別ファイルシステムを読み飛ばした場合、スナップショット取得時と `qdu show` に件数を表示します
- `--cross-filesystems` で複数のファイルシステムを走査した場合、容量欄は重複しないデバイスごとの値を合計して表示します

## 除外パターン

```bash
qdu snapshot \
  --exclude '.git' \
  --exclude 'node_modules' \
  --exclude '*.tmp'
```

除外規則は、ディレクトリ容量、ユーザー集計、大容量ファイル候補で共通です。

- `/` を含まないパターン：どの階層のパス要素にも一致
- `/` を含むパターン：走査ルートからの相対パス全体に一致
- `*`、`?`、`[abc]` を使用可能

| パターン | 一致例 |
|---|---|
| `cache` | `cache/file`、`a/cache/file` |
| `*.tmp` | `a.tmp`、`work/a.tmp` |
| `models/*.bin` | `models/model.bin` |
| `**/cache/**` | 各階層の `cache` 配下 |

ファイルから読み込む場合：

```bash
qdu snapshot --exclude-from "$HOME/.config/qdu/excludes"
```

空行と `#` で始まる行は無視されます。
