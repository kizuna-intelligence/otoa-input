---
name: "git-commit-and-pr-rules"
description: "git commit\u3001DCO \u7f72\u540d\u3001\u5c65\u6b74\u306e\u66f8\u304d\u63db\u3048\u3001\u30d7\u30eb\u30ea\u30af\u30a8\u30b9\u30c8\u306e\u4f5c\u6210\u3092\u884c\u3046\u3068\u304d\u3002\u3068\u304f\u306b\u65b0\u898f\u30ea\u30dd\u30b8\u30c8\u30ea\u3067\u6700\u521d\u306e\u30b3\u30df\u30c3\u30c8\u3092\u6253\u3064\u524d\u3068\u3001PR \u3092\u4f5c\u308d\u3046\u3068\u3057\u305f\u3068\u304d\u306b\u4f7f\u3046\u3002"
---

# コミットと PR の作法

## 1. コミットの著者情報

**コミットは必ずこの identity で打つ。**

```
user.name  = mera-chan
user.email = 269069712+mera-chan[bot]@users.noreply.github.com
```

DCO の署名行（`git commit -s`）も同じものにする。

### なぜ

**実メールアドレスをコミット履歴に入れてはいけない。** 一度 push すると
公開リポジトリの全コミットに永久に残り、消すには履歴の書き換えと
強制 push が要る。他人が clone した後では取り返しがつかない。

noreply アドレスを使うのは、まさにこれを避けるためである。

### 新しいリポジトリを作ったとき

`git init` した直後はリポジトリ設定が空で、グローバル設定が使われる。
**グローバル設定が正しいことを確認してから最初のコミットを打つ。**

```bash
git config user.name  "mera-chan"
git config user.email "269069712+mera-chan[bot]@users.noreply.github.com"
```

`git commit -c user.email=...` のような一時指定で別のアドレスを渡さないこと。
一時指定はグローバル設定を黙って上書きするので、気づかないまま実メールが入る。

### 打ってしまったとき

**push する前なら書き換えられる。** 未 push であることを先に確認する。

```bash
git log --all --format='%an|%ae|%cn|%ce|%b' | grep -i '<自分のドメイン>'
```

出てきたら書き換える。

```bash
FILTER_BRANCH_SQUELCH_WARNING=1 git filter-branch -f \
  --env-filter '
    export GIT_AUTHOR_NAME="mera-chan"
    export GIT_AUTHOR_EMAIL="269069712+mera-chan[bot]@users.noreply.github.com"
    export GIT_COMMITTER_NAME="mera-chan"
    export GIT_COMMITTER_EMAIL="269069712+mera-chan[bot]@users.noreply.github.com"
  ' \
  --msg-filter 'sed "s|Signed-off-by: .*|Signed-off-by: mera-chan <269069712+mera-chan[bot]@users.noreply.github.com>|"' \
  -- --all
```

**書き換えたあと、必ず後始末まで行う。** `filter-branch` は元の履歴を
`refs/original/` に残すので、これを消さないと古いコミットが到達可能なまま残り、
検査しても「まだ実メールがある」と出る。

```bash
git for-each-ref --format='%(refname)' refs/original | xargs -r -n1 git update-ref -d
git reflog expire --expire=now --all
git gc --prune=now
```

最後にもう一度検査して 0 件になったことを確かめる。

### 既に push してしまったとき

書き換えて強制 push しても、**すでに clone された分は戻せない。**
公開リポジトリなら、そのアドレスは漏れたものとして扱う。
履歴の書き換えは共同作業者全員に影響するので、独断で行わず先に相談する。

## 2. プルリクエストは必ず Cyborgy 経由で作る

**`gh pr create` や GitHub の画面から直接 PR を作らないこと。**
PR は必ず Cyborgy を通して作る。

### なぜ

直接作った PR は Cyborgy の Task と結び付かない。そうなると、

- どの依頼から出た変更なのかが追えなくなる
- レビューの依頼と結果が記録に残らない
- 作業の重複や取りこぼしが見えなくなる

Cyborgy 経由なら、Task・Session・PR が繋がった状態で残る。

### 手順

具体的な流れは Cyborgy の PR 関連スキルに従う。

- `base-pr-review-requester` — 対象の Task を解決または作成し、レビューの
  Session を立ち上げる
- `base-github-pr-reviewer` — レビュー側の作法
- `base-git-release-safety` — commit / push / PR / マージの扱い
- リポジトリ固有の決まりがあるスキル（例: `cyborgy-repository-pr-policy`）が
  あれば、そちらが優先する

**該当する Task が分からない場合でも、直接 PR を作って済ませない。**
既存の Task を探し、無ければ作ってから進める。
