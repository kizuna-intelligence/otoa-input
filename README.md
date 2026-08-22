# Otoa Input

話した内容をその場のカーソル位置へ貼り付ける、PC 向けの音声入力ツール。
**軽くて速い。GPU は要りません。**

| | |
|---|---|
| **CPU 1 コアで動く** | 7.68 秒の発話を **0.32 秒**で認識（実時間比 0.042） |
| **待ち受け中はほぼ無負荷** | CPU **2%** / メモリ **44 MB** |
| **1 つ起動するだけ** | 実行ファイルは 1 つ。ASR サーバーは必要なときに自分で立ち上がる |
| **外部サービス不要** | クライアントと ASR サーバーの両方が入っている。音声は外へ出ない |

**コア数を増やしても速くなりません。1 コアで足ります。**
20 コアで 0.185 秒、1 コアで 0.324 秒。実測は
[どれくらいの機械で動くか](#どれくらいの機械で動くか)に。

- クライアント — マイク取得、VAD、オーバーレイ、貼り付け（Linux / macOS / Windows）
- サーバー — [ReazonSpeech k2-v2](https://huggingface.co/reazon-research/reazonspeech-k2-v2) による日本語音声認識
- 両者は WebSocket の **Otoa ASR Protocol v1** で話す（[`docs/otoa_asr_protocol.md`](docs/otoa_asr_protocol.md)）

## 使ってみる

ビルドは要りません。**2 つ落として、起動するだけです。**

### 1. 本体を落とす

[リリースページ](https://github.com/kizuna-intelligence/otoa-input/releases/latest)から
自分の OS のものを取ります。

| OS | ファイル |
|---|---|
| Linux | `otoa-input-<版>-linux-x86_64.AppImage` |
| macOS (Apple Silicon) | `otoa-input-<版>-macos-arm64.dmg` |
| Windows | `otoa-input-<版>-windows-x86_64.zip` |

- **Linux**: 実行できるようにします。`chmod +x otoa-input-*.AppImage`
- **macOS**: `.dmg` を開いて `Otoa Input.app` を「アプリケーション」へドラッグ。
  署名と Apple の公証を通してあるので、そのまま開けます
- **Windows**: `.zip` を展開して `otoa-input.exe`

### 2. 認識モデルを落とす

音声認識のモデルは大きい（数百 MB）ので同梱していません。別に取ります。

```bash
pip install -U "huggingface_hub[cli]"
hf download reazon-research/reazonspeech-k2-v2 --local-dir reazonspeech-k2-v2
```

**取れたフォルダを、次のどちらかに `reazonspeech-k2-v2` という名前で置きます。**

| | 置き場所 |
|---|---|
| 本体の隣（持ち歩くならこちら） | `<本体と同じ場所>/models/reazonspeech-k2-v2` |
| ユーザーごとの場所 | Linux: `~/.local/share/otoa-input-oss/models/reazonspeech-k2-v2`<br>macOS: `~/Library/Application Support/otoa-input-oss/models/reazonspeech-k2-v2`<br>Windows: `%APPDATA%\otoa-input-oss\models\reazonspeech-k2-v2` |

例（Linux で本体の隣に置く場合）:

```
otoa-input-0.1.3-linux-x86_64.AppImage
models/
  reazonspeech-k2-v2/
    encoder-epoch-99-avg-1.onnx
    decoder-epoch-99-avg-1.onnx
    joiner-epoch-99-avg-1.onnx
    tokens.txt
```

### 3. 起動する

ダブルクリックするだけです。**認識サーバーは自分で立ち上がるので、別に何かを
起動する必要はありません。** 初回はモデルの読み込みに数秒かかります。

話し終えて少し黙ると、認識結果がカーソル位置へ貼り付きます。

### Linux だけ、追加で要るもの

デスクトップ環境が入っていれば大半は既に入っています。**最小構成の Ubuntu で
確認した、これだけあれば起動するという一覧です。**

```bash
sudo apt install \
  libasound2t64 libgtk-3-0t64 libxcb1 \
  libxkbcommon-x11-0 libayatana-appindicator3-1 libegl1 libgl1 \
  xdotool
```

- `xdotool` は**貼り付けに使います**（Wayland では `wtype`）。入っていないと、
  認識はできるのに貼り付けだけが失敗します
- `libxkbcommon-x11-0` と `libayatana-appindicator3-1` が無いと、**画面が
  出る前に落ちます**
- AppImage を FUSE 無しで動かす場合は `./otoa-input-*.AppImage --appimage-extract`
  で展開し、`squashfs-root/AppRun` を実行してください

## うまくいかないとき

```bash
./otoa-input --check-connection   # 認識サーバーへ繋がるか
./otoa-input --paste-test         # 貼り付けだけを試す
./otoa-input --help               # オプション一覧
```

- **「認識モデルが見つかりません」** → 上の置き場所を確認してください。
  フォルダ名は `reazonspeech-k2-v2`、中に `tokens.txt` がある状態です
- **認識はできるのに貼り付かない（Linux）** → `xdotool` / `wtype` を入れます
- **貼り付かない（macOS）** → システム設定 → プライバシーとセキュリティ →
  アクセシビリティ で `Otoa Input` を許可します

## ソースからビルドする

利用するだけなら不要です。

必要なもの: Rust（stable）と、ビルド時のネットワーク（`sherpa-onnx` が
ONNX Runtime を取得します）。Linux では次も要ります。

```bash
sudo apt install libgtk-3-dev libasound2-dev libxcb1-dev
```

```bash
cargo run --release -p otoa-input-app
```

配布物を作る場合:

```bash
bash scripts/build-release.sh    # 共通
bash scripts/package-linux.sh    # Linux: AppImage
bash scripts/package-macos.sh    # macOS: 署名・公証済み DMG（鍵は環境変数で渡す）
```

別の機械でサーバーだけ動かす場合:

```bash
otoa-input --serve --asr-model-dir=<dir>   # 同じ実行ファイルでサーバーだけ
otoa-asr-server --asr-model-dir=<dir>      # サーバー単体のバイナリ
```

## どれくらいの機械で動くか

Intel Core Ultra 7 265K の **1 コアに CPU 帯域の制限をかけて実測**した。
7.68 秒の日本語発話 1 つを、話し終えてから確定が返るまでの待ち時間。

| 1 コアのうち使える割合 | 待ち時間 | モデル読み込み | 実用性 |
|---|---|---|---|
| 100% | **0.32 秒** | 6 秒 | 快適 |
| 50%  | 0.57 秒 | 6 秒 | 快適 |
| 25%  | 1.18 秒 | 11 秒 | 我慢できる |
| 12%  | 2.69 秒 | 21 秒 | 遅い |
| 6%   | 6.37 秒 | 46 秒 | 実用外 |

**認識結果はどの条件でも同一。** CPU を絞っても精度は落ちず、待ち時間だけが伸びる。

コア数を増やしても速くならない。20 コア 0.185 秒 → 4 コア 0.191 秒 →
2 コア 0.258 秒 → 1 コア 0.324 秒。**この認識器は並列化で速くならないので、
1 コアで足りる。**

- **クライアント側は無視してよい。** マイクを常時待ち受けしていて CPU 2%、
  メモリ 44 MB。VAD は 32 ms ごとに動くが軽い
- **効いてくるのはメモリ。** ASR サーバーが **1.3 GB** 使う。ほとんどが認識
  モデルで、CPU より先にこちらが下限を決める。**2 GB 以上を推奨**

体感の目安は「話し終えてから 1 秒以内に貼り付く」あたり。上の表では
1 コアの 25% 程度が境目になる。

配布物は AppImage 23 MB / DMG 28 MB / zip 24 MB。**実行ファイルは 1 つ**で、
共有ライブラリも追加ファイルも要らない（ONNX Runtime は静的リンク、
VAD モデルはバイナリへ埋め込み）。別途要るのは認識モデルだけ。

## 中身を読む人へ

実装上の約束事は [`docs/design.md`](docs/design.md) にまとめてある。
プロトコルの仕様は [`docs/otoa_asr_protocol.md`](docs/otoa_asr_protocol.md)。

## 主な設定

設定画面（トレイアイコン →「設定」）から変更できます。設定ファイルの場所は
Linux なら `~/.config/otoa-input-oss/settings.json`、認識モデルの既定の探索先は
`~/.local/share/otoa-input-oss/models/reazonspeech-k2-v2` です。


| 項目 | 既定 | 説明 |
|---|---|---|
| サーバー URL | `ws://127.0.0.1:8770/asr/v1` | 接続先 |
| 発話終了の判定 | `client` | 同梱サーバーは `client` のみ |
| VAD しきい値 | 0.5 | 上げると拾いにくく、下げると誤検知が増える |
| 無音判定 | 300 ms | これだけ黙ると区切る |
| プリロール | 500 ms | 検知が遅れた分をさかのぼって送る長さ |

オプションの一覧は `--help` で出ます。

```bash
cargo run --release -p otoa-asr-server -- --help
cargo run --release -p otoa-input-app -- --help
```

サーバーの `--dump-dir` を指定すると、認識へ渡した音声をそのまま WAV で
書き出します。「先頭が欠ける」ような不具合で、欠けているのが音声なのか
認識結果なのかを切り分けるためのものです。

## ライセンス

MIT。詳細は `LICENSE`、同梱物と別途取得するモデルの表示は `NOTICE` を
参照してください。

依存する第三者クレートの一覧とライセンス全文は `THIRD-PARTY-LICENSES.md`
（`cargo about` で自動生成、配布物へ同梱）にあります。**MPL-2.0 のクレートを
含みます。** いずれも改変せず使っており、配布した版に対応する対象ソース
コードの入手方法も同ファイルに記載しています。

コントリビュートは `CONTRIBUTING.md` を参照してください。DCO による署名を
使います。署名は特許条項への同意も兼ねます（寄与の実施に必要な自分の特許に
ついて通常実施権を許諾するか、関係する特許を通知する）。
