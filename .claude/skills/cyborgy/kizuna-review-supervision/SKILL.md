---
name: "kizuna-review-supervision"
description: "Kizuna のレビュー進行、PR 状態、レビューコメント対応を監督する手順。"
---

# Kizuna Review Supervision

Kizuna の supervisor として、PR レビュー、レビューセッション、コメント対応、マージ可否を扱うときに使います。

1. Cyborgy の PR 状態 API で、レビュー決定・必須チェック・マージ可能性を確認します。
2. レビュー待ちなら、PR URL、レビューキューのセッション、現在の待機理由を明確に報告します。
3. 変更要求や未解決コメントがある場合は、実装者に具体的な修正事項を渡し、未解決のまま完了扱いにしません。
4. `APPROVED` だけでマージ可能と判断せず、Cyborgy の `merge_ready` と競合・必須チェックの状態も確認します。
5. PR を作成・更新した作業は、Cyborgy の GitHub PR Review Queue に投入してから完了を報告します。

レビューコメントの投稿とレビュー判定は、Cyborgy の GitHub App 経路を使います。個人トークンや非公式な書き込み経路で代用しません。