# スナップショットと走査範囲

[English](SNAPSHOTS_AND_SCANNING.md) | 日本語

スナップショット取得時の設定、読み取り権限、除外パターン、マウントポイントの扱いを説明します。

## スナップショットを取得する

基本の形は次のとおりです。

```bash
qdu snapshot
```

名前付きプロファイルを使うには、`--profile`で名前を指定します。

```bash
qdu snapshot --profile data
```

次の例では、ユーザー別統計や大容量ファイル候補も保存します。

```bash
qdu snapshot \
  --profile data \
  --with-users \
  --user-max-depth 3 \
  --file-top 2000 \
  --exclude '.cache' \
  --exclude 'node_modules'
```

コマンドラインで指定しなかった項目には、プロファイル設定または既定値が使われます。全オプションと既定値は[`qdu snapshot`オプション一覧](COMMAND_REFERENCE_ja.md#qdu-snapshot)で確認できます。

標準エラーが端末のとき、`qdu snapshot` は進捗を 1 行で表示し続けます。表示するのは、ここまでに走査したエントリ数とディレクトリ数、数えた割り当て容量、経過時間、読んでいるディレクトリです。`--quiet` を付けると表示しません。標準エラーをファイルやパイプへリダイレクトした場合も表示しません。

#### `--record-max-depth` の注意

このオプションはスナップショットへ保存するディレクトリ行の深さを制限します。深い場所の容量も親の合計には含まれますが、制限より深いディレクトリは個別に表示できません。

容量を計算するために配下は走査するので、走査時間が大幅に短くなるとは限りません。

#### 読み取れない場所がある場合

一般ユーザーでは、一部のディレクトリを読めないことがあります。qduは読めた範囲を**不完全なスナップショット**として保存します。

- 終了コード：`3`
- 状態：`incomplete`
- `latest-any`：更新される
- 最新の完全な結果を指す `latest`：更新されない

不完全な結果を見るには、`--snapshot latest-any`を指定します。

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

例えば `/home` 自体が `autofs` で、`/home/alice` や `/home/bob` が個別のNFSマウントになっている環境では、既定のままだと各ホームディレクトリを走査しません。その場合、使用量は `0B in 0 files and 1 directory` のように表示され、マウント先を読み飛ばしたという警告が出ます。`--cross-filesystems` を付けると、それらのマウント先へ入ります。

ただし、一般ユーザーが読み取れないホームディレクトリは走査できません。読める範囲は保存され、結果は `incomplete` になります。sudoを使わず、`qdu errors --snapshot latest-any` で読めなかった場所を確認してください。

> [!CAUTION]
> `--cross-filesystems` は走査ルート配下にある**すべての別ファイルシステム**を対象にします。NFSや応答の遅いマウントが含まれると、取得に時間がかかることがあります。まず対象範囲を `findmnt -R PATH` などで確認してください。

その他の規則は次のとおりです。

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

除外規則はディレクトリ容量、ユーザー集計、大容量ファイル候補で共通です。

パターンは gitignore の書き方に従います。ただし、否定（`!`）には対応していません。

- `/` を含まないパターン（`.git`、`*.tmp` など）は、どの階層のファイル名・ディレクトリ名とも照合します。
- 先頭か途中に `/` を含むパターン（`/build`、`data/raw` など）は、走査ルートからのパスと照合します。どの階層でも照合したいときは、先頭に `**/` を付けます。
- 末尾の `/`（`logs/` など）はディレクトリだけに一致します。
- `*` と `?` は 1 つの名前の中だけで一致し、`[abc]` と `[!abc]` は文字クラスです。`**/` は任意の数のディレクトリ、`/**` はディレクトリの中身すべて、`a/**/b` は間の 0 個以上のディレクトリに一致します。
- バックスラッシュは次の 1 文字をそのままの文字として扱います。大文字と小文字は区別します。
- 除外したディレクトリの中には入らないため、その配下は数えません。

| パターン | 除外されるもの | 除外されないもの |
|---|---|---|
| `node_modules` | `node_modules`、`a/node_modules` とその配下 | `node_modules2` |
| `*.tmp` | `a.tmp`、`work/a.tmp` | `a.tmp.gz` |
| `/build` | 走査ルート直下の `build` | `src/build` |
| `data/raw` | 走査ルートからの `data/raw` | `p/data/raw` |
| `**/models/*.bin` | `models/a.bin`、`run/models/a.bin` | `models/sub/a.bin` |
| `logs/` | `logs` という名前のディレクトリ | `logs` という名前のファイル |
| `cache/**` | 走査ルート直下の `cache` の中身すべて | `cache` ディレクトリ自体 |

除外パターンをファイルから読み込むには、`--exclude-from`を使います。

```bash
qdu snapshot --exclude-from "$HOME/.config/qdu/excludes"
```

空行と `#` で始まる行は無視されます。

## UTF-8 ではないファイル名

UTF-8 として正しくないファイル名（別の文字コードで作られたアーカイブを展開した名前など）は、デコードできないバイトを `\xNN` と書いて保存します（例：`report-\x93\xfa.txt`）。除外パターンではバックスラッシュが次の 1 文字のエスケープになるため、この名前は `report-\\x93*` と書くか、`report-*` のようにワイルドカードで指定してください。
