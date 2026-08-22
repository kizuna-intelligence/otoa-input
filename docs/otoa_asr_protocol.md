# Otoa ASR Protocol v1

otoa-input が音声認識サーバーと話す WebSocket プロトコルの仕様。

**この仕様を満たすサーバーを立てれば、otoa-input の接続先を差し替えて
自分の音声認識基盤で使える。** 設定画面の「サーバー URL」で切り替える。

---

## 1. 接続

```
wss://<host>/<path>
```

- **WebSocket**（`wss` を推奨。ローカル開発では `ws` でもよい）
- サブプロトコルの指定は無い
- クライアントは接続確立後、**最初に 1 つの Text フレーム**で設定 JSON を送る

### 認証
サーバーが認証を要求する場合、クライアントは
WebSocket ハンドシェイクのヘッダで渡す。

```
Authorization: Bearer <token>
```

- 認証しないサーバー（自分のローカル環境など）は、このヘッダを無視してよい
- 認証に失敗したら **WebSocket にアップグレードせず HTTP 401** を返すこと。
  アップグレードしてから閉じると、クライアントが原因を判別できない

---

## 2. 設定メッセージ（クライアント → サーバー、1 回目の Text フレーム）

```json
{
  "model": "stt-rt-v5",
  "audio_format": "pcm_s16le",
  "sample_rate": 16000,
  "num_channels": 1,
  "language_hints": ["ja"],
  "enable_endpoint_detection": true,
  "endpoint_mode": "client",
  "max_endpoint_delay_ms": 1500,
  "endpoint_sensitivity": 0.0,
  "endpoint_latency_adjustment_level": 0,
  "client_reference_id": "otoa-input"
}
```

| フィールド | 型 | 必須 | 意味 |
|---|---|---|---|
| `model` | string | 必須 | サーバーが解釈するモデル識別子。意味はサーバーが決める |
| `audio_format` | string | 必須 | 現在 `pcm_s16le` のみ |
| `sample_rate` | number | 必須 | 現在 `16000` のみ |
| `num_channels` | number | 必須 | 現在 `1` のみ |
| `language_hints` | array\<string\> | 任意 | ISO 言語コード。無指定は自動判定 |
| `enable_endpoint_detection` | bool | 必須 | `true` |
| `endpoint_mode` | string | 任意 | 発話の区切りを誰が決めるか。`client` / `server`。下記 |
| `max_endpoint_delay_ms` | number | 任意 | 発話終了後、区切りを返すまでの上限 |
| `endpoint_sensitivity` | number | 任意 | 大きいほど区切りを出しやすい |
| `endpoint_latency_adjustment_level` | number | 任意 | 大きいほど低遅延・積極的 |
| `api_key` | string | 任意 | 直接接続時のみ。サーバー経由では**送らない** |

**未知のフィールドは無視すること。** クライアントが将来増やしても壊れないため。

### `endpoint_mode` — 区切りの決定者は 1 か所だけ

| 値 | 意味 |
|---|---|
| `client` | クライアントの VAD が決める。サーバーは `finalize` を受けるまで音声を溜め、`<end>` を送らない |
| `server` | サーバーが決め、`<end>` を送る |

**両方に判定を持たせてはならない。** 片方が「発話は終わった」と見なした時点で
音声の蓄積が止まり、もう片方は空の結果を受け取る。実際にこれで
「最初の 1 回しか確定しない」不具合が起きた。

- 省略された場合の解釈は**サーバーが決める**
- **対応しない値を受け取ったサーバーは、黙って受理せずエラーを返すこと。**
  受理すると、クライアントは永久に来ない `<end>` を待ち続ける

---

## 3. 音声（クライアント → サーバー）

- **Binary フレーム**で生 PCM を送る。base64 にしない
- **16 kHz / モノラル / 符号付き 16bit リトルエンディアン**
- 1 フレーム 120 ms（1920 サンプル = 3840 バイト）を実時間で送る
- **発話していない間も送り続ける。** 送信を止めない
  （止めるとサーバーが発話終了を判定できない）

### 音声の終了
**空のフレーム**（長さ 0 の Text または Binary）を送る。
サーバーはこれを「音声はこれで終わり」と解釈する。

---

## 4. 制御メッセージ（クライアント → サーバー、設定後の Text フレーム）

| メッセージ | 意味 |
|---|---|
| `{"type":"finalize"}` | 現時点までを強制的に確定する |
| `{"type":"keepalive"}` | 接続維持。音声を送らない区間で使う |

**上記 2 つと空フレーム以外の Text を受け取ったら、サーバーは無視してよい。**

---

## 5. 認識結果（サーバー → クライアント、Text フレーム）

```json
{
  "tokens": [
    { "text": "こんにちは", "is_final": true, "start_ms": 600, "end_ms": 1200, "confidence": 0.97 },
    { "text": "<end>", "is_final": true }
  ],
  "final_audio_proc_ms": 1200,
  "total_audio_proc_ms": 1400
}
```

### token
| フィールド | 型 | 必須 | 意味 |
|---|---|---|---|
| `text` | string | 必須 | 文字列。特殊トークンを含む（下記） |
| `is_final` | bool | 必須 | 確定済みか |
| `start_ms` / `end_ms` | number | 任意 | 音声中の位置 |
| `confidence` | number | 任意 | 0.0–1.0 |
| `speaker` / `language` | string | 任意 | 使わなくてよい |

### partial は任意

`is_final: false` の token（途中経過）を**送らないサーバーがあってよい。**
非ストリーミングの認識器では原理的に出せない。同梱の `otoa-asr-server` は
ReazonSpeech k2-v2 が非ストリーミングのため partial を返さない。

貼り付けは区切りの確定時なので、partial が無くても動作は成立する。
**クライアントは partial が一度も来ないことを前提に作ること。**

### 蓄積の規則（クライアントの実装）
- `is_final: true` の token は**追記**される。同じ token を再送しないこと
- `is_final: false` の token は、**そのレスポンス限りの暫定値**として扱われる。
  クライアントは受信のたびに前回の暫定分を捨てて置き換える
- したがってサーバーは、暫定分を**毎回すべて**送ること

### 特殊トークン
| text | 意味 |
|---|---|
| `<end>` | **発話の区切り**。これを受けた時点までの確定文字列が 1 つの発話として確定する |
| `<fin>` | `finalize` の完了通知 |

**`<end>` と `<fin>` は本文に連結しない。**

`<end>` は **`is_final: true`** で送ること。
`<end>` を送る前に、その発話の token をすべて `is_final: true` にしておくこと。

### 終了
```json
{ "tokens": [], "finished": true }
```
空フレームを受け取ったら、残りを送り切ってから `finished: true` を返し、
**その後に WebSocket を正常クローズ（コード 1000）する。**

クローズフレームを送ったら、相手の応答を待ってから TCP を閉じること。
待たずに閉じると、クライアントには異常切断として観測される。

---

## 6. エラー（サーバー → クライアント）

```json
{
  "tokens": [],
  "error_code": 401,
  "error_type": "unauthenticated",
  "error_message": "…",
  "request_id": "…"
}
```

- `error_type` は**安定した機械可読の識別子**にすること。
  クライアントはこれで分岐し、`error_message` の文言では分岐しない
- エラーを 1 つ送ったら接続を閉じてよい

---

## 7. クライアントの前提（サーバー実装者向けの注意）

otoa-input は次のように振る舞う。サーバーはこれを前提にしてよい。

1. **発話の開始は端末側の VAD で判断する。** 検知した時点で接続し、
   直前 500 ms 分の音声（プリロール）を先に送ってから実時間送出に移る
2. **発話の終了は判断しない。** `<end>` を受けるまで音声を送り続ける
3. `<end>` を受けたら、その発話を確定してクリップボードへ貼り付ける
4. 最後の `<end>` から一定時間（既定 15 秒）で空フレームを送り接続を閉じる
5. `finished` を受け取る前にクライアントから切断しない

**したがって「いつ発話が終わったか」を決めるのはサーバーである。**
これがこのプロトコルの中心的な役割分担である。

---

## 8. 最小のサーバー実装の流れ

```
1. WebSocket を受け付ける（必要なら Authorization を検証、失敗は HTTP 401）
2. 最初の Text フレームを設定として読む
3. Binary フレームを PCM として受け取り、認識器へ流す
4. 暫定結果を is_final:false の token として随時返す
5. 発話の区切りを検出したら、確定 token を is_final:true で返し、
   最後に {"text":"<end>","is_final":true} を 1 回返す
6. {"type":"finalize"} を受けたら強制確定し、{"text":"<fin>","is_final":true} を返す
7. 空フレームを受けたら残りを返し、{"finished":true} を返してから
   コード 1000 でクローズする
```

---

## 9. 互換性

- 本仕様は **v1**。破壊的変更を行う場合は設定に版数を追加する
- クライアントは**未知のフィールドを無視する**。サーバーも同様にすること
