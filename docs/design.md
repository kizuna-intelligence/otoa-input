# 実装者向けの設計メモ

コードを読む前に知っておくと迷わない点をまとめる。
利用者向けの説明は README にある。

## 発話の区切りを決めるのはクライアント

クライアントの VAD が無音を検知した時点で `finalize` を送り、サーバーは
それまでに受け取った音声を認識する。**このサーバーは終話を判定しない。**

**決定者を 2 か所に持たせてはいけない。** 片方が「発話は終わった」と見なした
時点で音声の蓄積が止まり、もう片方は空の結果を受け取る。実機では
「最初の 1 回しか貼り付かない」という形で現れた。

決定者は設定 JSON の `endpoint_mode` で宣言する。このサーバーは `client`
以外を 400 で拒否する。黙って受理すると、クライアントは永久に来ない `<end>`
を待ち続ける。

サーバー側で終話を判定する構成自体は仕様上ありうる（`endpoint_mode=server`）。
このサーバーが実装していないだけである。

## マイクが動いている間、音声を捨てない

接続を張り直している最中に届いた音声も、プリロール用のリングバッファに積む。
捨てると次の発話の先頭が欠ける。音声を捨ててよいのはマイクを止めている
`Disabled` のときだけ。

## 途中経過テキスト（partial）は出さない

ReazonSpeech k2-v2 は非ストリーミングで、一度に扱えるのは約 30 秒まで。
このサーバーは partial を返さない。**Otoa ASR Protocol では partial は任意**
なので仕様違反ではない。貼り付けは区切りの確定時なので動作は成立し、
オーバーレイに途中経過が流れないだけである。

## 接続先の既定値は各実装が持つ

共通設定の `server_url` は空が既定で、空なら `ConnectionProvider` の実装が
自分の既定値を使う。**共通側に既定値を置くと、別の接続先を使うビルドが
設定ファイル無しで自分のローカルへ繋いでしまう。**

## 拡張ポイントは 1 つだけ

`otoa_input_app::run(Deps)` の `Deps` が持つのは
`provider: Arc<dyn ConnectionProvider>` のみ。**増やすたびに、公開側が永久に
維持する API が増える。**

接続先ごとの設定は `Settings::product`（不透明な JSON）を通して各実装が
自分で解釈する。ここに型を持たせると、接続先を差し替えるたびに公開側の型が
変わる。

ログイン関係の UI は `ConnectionProvider` の `prepare` / `authenticate` /
`account` / `readiness` だけで動く。`prepare()` が `None` を返す実装では
UI 自体が出ない。

## 配布物はバイナリだけで動く

ONNX Runtime は静的リンクし、Silero VAD モデルはバイナリへ埋め込んである。
共有ライブラリも `resources/` も要らない。

例外は音声認識モデル（ReazonSpeech k2-v2）で、数百 MB あるため同梱しない。
`--asr-model-dir` で場所を渡す。

## 調べ方

`otoa-asr-server --dump-dir=<dir>` を指定すると、認識へ渡した音声をそのまま
WAV で書き出す。「先頭が欠ける」ような不具合で、欠けているのが届いた音声なのか
認識結果だけなのかを切り分けられる。

`otoa-input --paste-test [文字列]` は本文を CLIPBOARD と PRIMARY に置き、既定の
`Shift+Insert` を送って状態をログへ出す診断である。貼り付けは OS ごとに実装が違い、
外部コマンド（Linux の `xdotool` / `wtype`）や権限（macOS のアクセシビリティ）に
依存するので、そこだけ切り離して確認できる。

## 貼り付け経路

Linux では宛先のウィンドウやプロセスを調べず、X11 と Wayland のどちらでも
CLIPBOARD と PRIMARY に同じ本文を置いて `Shift+Insert` を送る。X11 では貼り付け前の
PRIMARY を控え、既定では 150ms 後に元へ戻す。`paste_shortcut` に `ctrl-v`、
`ctrl-shift-v`、`shift-insert` を指定すると、`auto` の既定送出を手動で上書きできる。
Wayland の貼り付け動作は未実測であり、問題がある場合は `paste_shortcut=ctrl-v` を
逃げ道として使う。

UI の検証はホストで起動せず、`bash test-e2e/ui-preview.sh <state...>` で Docker から実行する。
設定面は `settings:general` など、透過を含めるときは `--compositor` を付ける。
出力先は `--out DIR` で指定し、`summary.txt` と PNG を確認する。

## UI の約束事

- 入力バーの `overlay_position=center` は作業領域ではなく画面全体の中央に置く。
  `bottom` と `top` は X11 の作業領域を基準にする。Wayland ではウィンドウ位置の
  指定が効かないことがある。
- `overlay_transparent=auto` は X11 の `_NET_WM_CM_S0` にコンポジタの所有者が
  いるかで透過を判定する。透過を使えない環境では不透過へフォールバックし、角丸の
  外側を背景色で塗る。`on` は透過を要求し、`off` は常に不透過にする。
- 見た目だけを確認するときは `--preview-overlay=<state>`、設定画面は
  `--preview-settings=<general|mic|asr|advanced|account|about>` を使う。ホストで
  起動せず、Docker の `bash test-e2e/ui-preview.sh` から撮影する。
- トレイの待受状態・ツールチップ・メニュー文言は、Linux では自前ループから更新し、
  Windows / macOS ではメインスレッドの UI 更新経路から更新する。コード上の更新経路は
  共通だが、実機での表示確認は OS ごとに行う。

## プラットフォーム差

- Windows のコンソールは日本語環境の既定が Shift-JIS なので、起動時に出力
  コードページを UTF-8 へ切り替えている

## macOS の配布

`.tar.gz` の中の実行ファイルには署名が無く、ダウンロードすると Gatekeeper に
弾かれる。**macOS 向けには `scripts/package-macos.sh` で `.dmg` を作る。**
Developer ID で署名し、Apple の公証を通して staple する。

このスクリプトで踏みやすいのは次の 2 つ。

- **DMG を、DMG の元にするフォルダの中へ書かない。** 自分を取り込みながら
  膨らみ、空き容量を食い潰して失敗する
- **元にするフォルダへ鍵や中間ファイルを置かない。** そこに入れたものは
  そのまま利用者へ渡る。スクリプトは配布直前に `.p12` / `.pem` / `.p8` が
  混ざっていないか確かめる

SSH 越しの非対話セッションでは秘密鍵の使用許可ダイアログを出せず、`codesign`
が失敗するかハングする。専用キーチェーンを作り、`set-key-partition-list` を
設定して既定キーチェーンにするのが回避策である。**この設定の終了コードを
握り潰すと、署名が必ずハングする。**
