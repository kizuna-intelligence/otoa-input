---
name: external-surface-hides-internals
description: Use when designing, implementing, reviewing, or documenting anything a user or customer can observe — public API field names, URL paths, enum and event values, error codes and error message bodies, response headers, health-check payloads, SDK and package names, and customer-facing documents or PDFs — so that vendor names, model names, hosting platforms, regions, project IDs, and internal component names do not leak. Also use when putting a third-party service behind your own API, when letting a client connect directly to a third party, and when migrating a surface that already leaks. Not for interfaces used only between your own services, and not for deciding whether a change counts as a specification change.
---

# 外に見える面に内部実装を出さない

## 原則

外から観測できるものは、意図しなくても契約になる。内部実装の固有名を出すと 2 つ損をする。

1. **差し替えられなくなる。** ベンダーやモデルを替えると顧客のコードが壊れる。
2. **何を使っているかが分かる。** 顧客にも競合にも見える。

**種類は出してよい。固有名を出さない。** 「音声認識」「生成」「音声合成」は機能の区分であって
実装ではない。`asr_error` や `tts_error` のような分類は残してよい。

## 出してはいけないもの

- 第三者サービスの名前・製品名（音声認識・生成・音声合成・発話検出などの実装）
- モデル名とバージョン
- ホスティング基盤、リージョン、プロジェクト ID、サービスアカウント、コンテナ名
- 社内でだけ通じるコンポーネント名

## 監査する面

設計時とレビュー時に、次を全部見る。**4 と 7 が見落とされやすい。**

1. フィールド名（リクエスト・レスポンス・イベント）
2. URL のパスとクエリ
3. 列挙値・イベントの値（例: 発話開始イベントの `source` に検出器の実装名が入る）
4. エラーコードと**メッセージ本文**。本文は顧客の画面やログに届く
5. レスポンスヘッダ、ヘルスチェックの応答、`component` のような自己申告
6. SDK・パッケージ・設定ファイルの名前、顧客向け文書と配布 PDF
7. **クライアントが直接つなぐ先**。第三者へ直接つながせた時点で、鍵の取り方も
   パラメータもプロトコルの癖も全部見える。名前を伏せても意味がない

## 実装の指針

- **第三者へは自分のサーバがつなぐ。** 顧客に直接つながせない。直結は、地理的な往復を
  減らすなど明確な理由があるときだけにし、理由が消えたら畳む
- **中立な名前を最初に決める。** `asr_*`、`speech_*` のように、実装が替わっても保つ名前にする。
  後から改名すると互換の仕事が増える
- 分類のエラーコードは残してよいが、**本文に固有名を書かない**

## すでに漏れている面を直すとき

- **互換のために旧フィールドを既定で返し続けない。** 既定で返すと、新しい顧客の目にも入り、
  隠した意味が消える
- 旧経路は**明示的に要求されたときだけ**有効にする（`audio_uplink: "client"` のような opt-in）
- 自社のクライアントは同じ変更のなかで直す。次のリリースで旧経路ごと落とす

## 確認

**目視で済ませない。** 実際に外へ出るもの（応答、生成した文書、PDF の抽出テキスト）を
テキストとして取り出し、固有名で検索して残っていないことを示す。
