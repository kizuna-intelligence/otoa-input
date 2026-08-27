---
name: "kizuna-wasabi-data"
description: "Cyborgy skill: kizuna-wasabi-data"
---

# Wasabi のデータとチェックポイント

**ML 系のデータとモデルは基本的に Wasabi にある。リポジトリに実体を置かない。**

（titan-platform の製品データは別。そちらは S3 / GCS を使う。）

## 認証

環境変数 `WASABI_ACCESS_KEY` / `WASABI_SECRET_KEY`、
または `~/.aws/credentials` の `[wasabi]` プロファイル。

```
endpoint: https://s3.ap-northeast-1.wasabisys.com
```

`WASABI_ACCESS_KEY_TITAN` / `_SECRET_KEY_TITAN` は**別アカウント**。混同しないこと。

## バケットの使い分け

| バケット | 中身 |
|---|---|
| `kizuna-intelligence-checkpoint` | 学習済みモデル |
| `kizuna-intelligence-dataset` | データセット、評価データ |
| `asr-dataset-raw` / `processed-asr-dataset` | ASR コーパスの生 / 加工済み |
| `tver-archive` | TVer 収集物 |

## パスの規約

```
<バケット>/conversation/<領域>/<プロジェクト>/<版>/
<バケット>/evaluation/<タスク>/<データセットID>/
```

実例:

```
kizuna-intelligence-checkpoint/conversation/asr/granite-target-speaker/v7b-step8000/
kizuna-intelligence-dataset/evaluation/conversational_asr/realmic-20260822/
kizuna-intelligence-dataset/evaluation/conversational_asr/granite_spk_sim/traces-20260823/
```

版はディレクトリ名に入れる。**上書きしない。**

## 取得

`kizuna-ai` からはラッパを使う（キャッシュと SHA256 検証つき）。

```python
from kizuna.core.io.wasabi import fetch, verify, WasabiUnavailable

d = fetch("kizuna-intelligence-dataset", "evaluation/conversational_asr/realmic-20260822")
assert not verify(d)          # SHA256SUMS.txt と突き合わせる
audio = d / "segments" / "segment_0002.wav"
```

boto3 があればそれを使い、無ければ `aws` CLI に落ちる。
CLI だけに依存すると、Python は動くのに PATH に aws が無いホストで取得できなくなる（実際になった）。

キャッシュは `~/.cache/kizuna`（`KIZUNA_CACHE` で変更可）。

テストから使うときは、**取れなければ skip、取れたが SHA256 が合わなければ fail**。
黙って通さない。

CLI で直接触るとき:

```bash
export AWS_PROFILE=wasabi
EP=https://s3.ap-northeast-1.wasabisys.com
aws --endpoint-url $EP s3 ls s3://kizuna-intelligence-dataset/
aws --endpoint-url $EP s3 sync <src> s3://<bucket>/<prefix>/ --no-progress
```

## 上げるときに必ずやること

1. **README.md を同梱する** — 何のデータか、**どれが正しいか**（同名で複数の版が
   あるとき、どれを使うべきでどれが壊れているか）、既知の限界
2. **SHA256SUMS.txt を同梱する** — 取得後に検証できるように
3. **上げた後に取り直して検証する** — `s3 sync` して `sha256sum -c`
4. **生成元を README に書く** — リポジトリ名 / コミットハッシュ / スクリプト名(下記)

## 生成元を README に記録する

データを上げるときは、**それを作ったコードを特定できる形で README に書く**。
コードの実体を Wasabi に置くのではなく、次の3つ組で指す。

```
リポジトリ名 / コミットハッシュ / スクリプト名
```

例:

```markdown
## 生成元

| 段 | リポジトリ | コミット | スクリプト |
|---|---|---|---|
| 強制整列 | kizuna-data-pipeline | b612f86 | tts/irodori_tts/prepare_narabas_alignment_manifest.py |
| prefix-duration 化 | kizuna-data-pipeline | b612f86 | tts/irodori_tts/prepare_narabas_prefix_duration_manifest.py |

実行時の引数:
`--chunk-mode openjtalk --assign-leading-to-first-phone`
```

**コードの実体を同梱しない。** リポジトリが正本であり、コピーを置くと
二重管理になって、コードが更新されたとき古い版が残る。
「リポジトリにデータの実体を置かない」の裏返しで、
**データバケットにコードの実体を置かない**。

コミットハッシュまで書くのは、スクリプト名だけでは再現できないため。
同じファイル名でも中身は変わる。**どの時点のコードで作ったか**が要る。

未コミットのコードで作ったデータは、**先にコミットしてから**上げる。
それができない事情があるなら、README にその旨と差分を明記する。

## 派生データを置くときの注意

モデル出力を凍らせたもの（トレース、埋め込み、特徴量）は、**どのチェックポイントで
採ったかを README に書く**。学習し直したら古くなる。

古いまま使い続けると「動いているように見えるが、測りたいものを測っていない」状態になる。
リポジトリに派生データを置くと、これが黙って起きる。だから Wasabi に置き、
どの版のものかを明示する。

## 秘密情報

`WASABI_SECRET_KEY` をコマンドラインに直接書かない（`ps` から見える）。
環境変数かプロファイルを使う。リポジトリに push する前に、鍵が混入していないか走査する。
