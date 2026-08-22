# デモ GIF の作り方

`docs/demo.gif` を作り直す手順。**画面を人が操作せずに撮れる**ようにしてある。

## 仕組み

Docker のクリーンな Ubuntu で、

1. Xvfb で仮想画面を出す
2. PulseAudio の null sink を作り、その monitor を**既定の入力**にする
   （これが仮想マイクになる）
3. テキストエディタ（mousepad）を開いて前面にする
4. AppImage を起動する。**ASR サーバーは自分で立ち上がる**
5. 用意した音声を `paplay` で仮想マイクへ流す
6. `ffmpeg -f x11grab` で画面を録る

## 音声を用意する

**実際の録音を使わない。** 開発中の音声には出せない内容が混ざる。
読み上げを合成して使う。

```bash
# 例: Cyborgy の tts API で作る
ffmpeg -i 読み上げ.mp3 -ar 16000 -ac 1 -c:a pcm_s16le demo_speech.wav
```

## 撮る

```bash
cd test-e2e/demo
cp ../../dist/*.AppImage otoa-input.AppImage
cp <用意した音声> demo_speech.wav
docker build -t otoa-demo .
chmod 777 .        # コンテナ側の利用者が書き戻せるように
docker run --rm \
  -v <HF キャッシュの models--reazon-research--reazonspeech-k2-v2>:/models-root:ro \
  -v "$PWD":/host --entrypoint /bin/bash otoa-demo -c '
    cp /host/otoa-input.AppImage /host/demo_speech.wav /host/record.sh /home/demo/
    mkdir -p /home/demo/models
    ln -sfn /models-root/snapshots/<sha> /home/demo/models/reazonspeech-k2-v2
    bash /home/demo/record.sh && cp /home/demo/out.mp4 /host/'
```

## GIF にする

```bash
# 黒帯を落とす（数値は画面サイズに合わせる）
ffmpeg -i out.mp4 -vf "crop=642:505:179:55" cropped.mp4
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
