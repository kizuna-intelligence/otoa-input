---
name: "verify-before-push"
description: "Cyborgy skill: verify-before-push"
---

# push する前に確かめる

**push した時点で自分の手を離れる。** private リポジトリでも、組織の全員と
将来の連携先が読める。公開してからでは取り返しがつかない。

## 見る場所を間違えない

**作業ツリーだけ見ても意味がない。** 消したつもりのものは履歴に残っている。

```bash
# 現在のファイル（これだけでは足りない）
git grep -nE '<パターン>' HEAD

# **全コミットの中身**（誰でも git log -p で読める）
git rev-list --all | while read c; do git grep -nE '<パターン>' "$c" 2>/dev/null; done

# **到達可能な全 blob**（消したファイルもここに残る）
git cat-file --batch-all-objects --batch-check='%(objectname) %(objecttype)' \
  | awk '$2=="blob"{print $1}' \
  | while read o; do git cat-file blob "$o" | grep -q '<パターン>' && echo "残留 $o"; done
```

**`git rev-list --all` はタグとリモート追跡参照も辿る。** ブランチを作り直しても、
古いタグが古い履歴を指していれば到達可能なまま残る。実際にこれで見落とした。

```bash
git for-each-ref | grep -v refs/heads   # タグと remotes を確認する
```

## 何を探すか

| | 例 |
|---|---|
| パスワード | キーチェーン、DB、テスト用アカウント。**「一時的だから」も出さない** |
| トークン・鍵 | API キー、`.p12` / `.pem` / `.p8`、`BEGIN ... PRIVATE KEY` |
| 実メールアドレス | コミットの著者、`Signed-off-by`、設定ファイル |
| 社内のホスト | `192.168.*`、社内 DNS 名、踏み台の情報 |
| 取引先・上流の名前 | 契約で出せないベンダ名、内部の URL |
| 第三者の配布物 | ビルドツールのバイナリなど。**再配布になる** |

```bash
for pat in 'BEGIN.*PRIVATE' 'password' 'passwd' 'token' 'api[_-]?key' \
           '192\.168\.' '<自社ドメイン>' '<ベンダ名>'; do
  echo "--- $pat"
  git rev-list --all | while read c; do git grep -l -iE "$pat" "$c" 2>/dev/null; done | sort -u
done
```

**当たったものを目で見る。** 生成式（`KC_PASS="$(head -c 32 /dev/urandom | ...)"`）と
固定値では意味がまったく違う。パターンに当たっただけで慌てない。

## 出してしまったとき

### まだ push していない

履歴を作り直す。**`refs/original/` の後始末とタグの張り直しまでやる。**
やらないと古いコミットが到達可能なまま残り、検査しても消えたように見えない。

```bash
git checkout --orphan clean && git add -A && git commit   # 全部畳む場合
git tag -d <古いタグ>... && git tag -a <タグ> -m "..."
git for-each-ref --format='%(refname)' refs/original | xargs -r -n1 git update-ref -d
git reflog expire --expire=now --all && git gc --prune=now
```

### 既に push した

**force push では消えない。** GitHub は到達不能になったコミットも、しばらくは
SHA を指定すれば取得できる。fork や clone があればそちらにも残る。

- **公開予定のリポジトリなら、リポジトリごと作り直す。** これが確実。
  リリースの成果物は再アップロードすればよい
- 既に公開済み、または他人が clone している可能性があるなら、**その秘密は
  漏れたものとして扱う。** 鍵やトークンは失効させて作り直す。履歴の書き換えは
  共同作業者全員に影響するので、独断で行わず先に相談する

## 秘密が「実物」として残っていないかも見る

リポジトリを綺麗にしても、**その秘密で守られていた実物が残っていれば意味がない。**
ビルド機の `/tmp` に署名鍵入りのキーチェーンが残っていた例がある。

- 一時ファイルを作るスクリプトは、**`trap ... EXIT` で必ず消す**
- 秘密を扱うディレクトリと、配布物に入れるディレクトリを分ける。
  配布物を作る直前に `.p12` / `.pem` / `.p8` が混ざっていないか検査する

## 関連

コミットの著者情報と PR の作り方は `git-commit-and-pr-rules` に従う。
