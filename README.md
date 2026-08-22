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

## 動かす

> **macOS の人へ**: リリースページの `.dmg` を使ってください。署名と Apple の
> 公証を通してあるので、ダブルクリックで開けます。`.tar.gz` の中身は署名が
> 無いため Gatekeeper に弾かれます。

### 1. 必要なもの

- Rust（stable）
- 認識モデル（下記）
- ビルド時にネットワーク。`sherpa-onnx` が ONNX Runtime を取得する

Linux では次も要る。

```bash
sudo apt install libgtk-3-dev libasound2-dev libxcb1-dev
# 貼り付けに使う。X11 なら xdotool、Wayland なら wtype
sudo apt install xdotool          # X11
sudo apt install wtype            # Wayland
```

`xdotool`（Wayland では `wtype`）は**実行時**に要る。入っていないと、
認識はできるのに貼り付けだけが失敗する。

### 2. 認識モデルを取る

ReazonSpeech k2-v2 の ONNX 一式を落とします。リポジトリには同梱していません。

```bash
pip install -U "huggingface_hub[cli]"
hf download reazon-research/reazonspeech-k2-v2 --local-dir models/reazonspeech-k2-v2
```

次の 4 つがあれば動きます。

```
encoder-epoch-99-avg-1.onnx
decoder-epoch-99-avg-1.onnx
joiner-epoch-99-avg-1.onnx
tokens.txt
```

### 3. 起動する

```bash
cargo run --release -p otoa-input-app
```

**これだけです。** 接続先が自分の機械で、まだ誰も待ち受けていなければ、
ASR サーバーは自分で立ち上がります。別のプロセスを起動する必要はありません。

リリースの配布物なら、`.AppImage` / `.app` / `.exe` をそのまま起動するだけです。

別の機械でサーバーだけ動かしたい場合は次のどちらかを使います。

```bash
otoa-input --serve --asr-model-dir=<dir>   # 同じ実行ファイルでサーバーだけ
otoa-asr-server --asr-model-dir=<dir>      # サーバー単体のバイナリ
```
話し終えて少し黙ると、認識結果がカーソル位置へ貼り付きます。

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

コントリビュートは `CONTRIBUTING.md` を参照してください。DCO による署名を
使います。署名は特許条項への同意も兼ねます（寄与の実施に必要な自分の特許に
ついて通常実施権を許諾するか、関係する特許を通知する）。
