# デモ GIF の作り方

`docs/demo.gif` を作り直す手順。**画面を人が操作せずに撮れる**ようにしてある。

## 仕組み

Docker のクリーンな Ubuntu で、

1. Xvfb で仮想画面を出す
2. PulseAudio の null sink を作り、その monitor を**既定の入力**にする
   （これが仮想マイクになる）
3. テキストエディタ（mousepad）を開いて前面にする
4. ホストでビルドした ELF バイナリを起動する。**ASR サーバーは自分で立ち上がる**
5. 用意した音声を `paplay` で仮想マイクへ流す
6. `ffmpeg -f x11grab` で画面を録る

## 音声を用意する

**実際の録音を使わない。** 開発中の音声には出せない内容が混ざる。
読み上げを合成して使う。

`record.sh` が Open JTalk（日本語ボイスが使えない場合は `espeak-ng`）で日本語を
合成し、`sox` で 16 kHz mono の WAV に変換する。実録音は使わない。Open JTalk では
漢字を含む説明文をそのまま読ませ、eSpeak-ng のフォールバックでは発音用テキストを
ひらがなにする。

既定の読み上げは「音声入力のデモです」「話した内容がそのままカーソル位置に貼り付けられます」。
2 文目は Open JTalk が「カーソルの位置」と発音する音声から助詞の区間だけを除き、
認識結果が元の文になるようにしている。

## 撮る

```bash
cd /home/yusuke/gitrepos/otoa-input
cargo build --release -p otoa-input-app
mkdir -p target/demo-recording
chmod 777 target/demo-recording  # コンテナの demo ユーザーが書き戻せるようにする
docker build -t otoa-demo -f test-e2e/demo/Dockerfile test-e2e/demo
MODEL_DIR=$(readlink -f "$HOME/.local/share/otoa-input-oss/models/reazonspeech-k2-v2")
MODEL_HUB=$(readlink -f "$MODEL_DIR/../..")
docker run --rm --user demo \
  -v "$PWD/target/release/otoa-input:/home/demo/otoa-input:ro" \
  -v "$MODEL_DIR:/home/demo/.local/share/otoa-input-oss/models/reazonspeech-k2-v2:ro" \
  -v "$MODEL_HUB/blobs:/home/demo/.local/share/otoa-input-oss/blobs:ro" \
  -v "$PWD/test-e2e/demo:/host-demo:ro" \
  -v "$PWD/target/demo-recording:/host" \
  --entrypoint /bin/bash otoa-demo -c \
  'cp /host-demo/record.sh /home/demo/record.sh && \
   DEMO_COMPOSITOR=0 bash /home/demo/record.sh && \
   cp /home/demo/out.mp4 /home/demo/final.png /home/demo/first.txt /home/demo/mousepad.txt /home/demo/app.log /host/'
```

バイナリはホストでビルドするが、**UI と ASR サーバーの起動は Docker 内だけ**で行う。
`DEMO_COMPOSITOR=0` は不透過フォールバック、`DEMO_COMPOSITOR=1` は `xcompmgr` を
使う透過表示である。Xvfb では透過ウィンドウが Mousepad の上に描画されないことが
あるため、成果物の録画には不透過フォールバックを使う。

## GIF にする

```bash
# Mousepad の窓（20,60 に置いた 960x480）だけを残す
ffmpeg -i out.mp4 -vf "crop=960:480:20:60" cropped.mp4
ffmpeg -i cropped.mp4 -vf "fps=10,scale=720:-1:flags=lanczos,palettegen=stats_mode=diff" palette.png
ffmpeg -i cropped.mp4 -i palette.png \
  -lavfi "fps=10,scale=720:-1:flags=lanczos[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=3" \
  ../../docs/demo.gif
```

## 詰まったところ

- **GUI は `--help` が通るだけでは起動しない。** `libxkbcommon-x11-0` と
  `libayatana-appindicator3-1` が無いと、画面が出る前に落ちる
- **モデルの読み込み完了はポートで待つ。** ログの文字列を待つと、
  ログの書式が変わったときに黙って進んでしまう
- コンテナから成果物を書き戻すので、ホスト側のディレクトリに書き込み権限が要る
