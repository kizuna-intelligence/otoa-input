---
name: "cyborgy-skill-binding"
description: "Cyborgy skill: cyborgy-skill-binding"
---

# スキルの追加とスキルセットへの紐づけ

**先に読むこと: `base-api-usage`。**
「Skill で名前が出てくる機能が MCP ツールとして直接あるとは限らない。
`search_apis` で探してから `call_api` で呼べ」と書いてある。
MCP ツール一覧だけを見て「できない」と結論づけないこと。

## 1. スキルを作る

```
create_skill(name=..., summary=..., description=..., tags=[...], content=<SKILL.md 本文>)
```

`description` は**いつ使うか**を書く。何をするかではない。
呼び出す側はこれを見て使うかどうかを決める。

## 2. 組織スキルセットへ紐づける

**`update_role` は使えない。** これはユーザーロール用（`PUT /me/roles/{id}`）で、
組織スコープのロールには効かず 410 を返す
（`user Role creation and mutation were removed`）。

正しいのは `agent-settings` API である。

```
# 現状を読む（既存を消さないため必須）
call_api(api_id="agent-settings", endpoint_id="get")

# organization.skill_references を取り出し、末尾に足して丸ごと送り返す
call_api(api_id="agent-settings", endpoint_id="set_organization",
         inputs={"organization_skill_references": [
             ...既存全部..., {"skill_id": "<新しいID>", "provision": "required"}]})
```

**リストは置き換えである。** 既存を読まずに送ると、組織全体のスキルが消える。
必ず `get` してから足すこと。

**`mcp__plugin_cyborgy-workspace_cyborgy__call_api` を使うこと。**
古い方の `call_api` は `endpoint_id` を受け取れず、すべて 405 になる。

## 3. リポジトリのスキルセットへ紐づける

**リポジトリ内の `.cyborgy/settings.json` に書く。**

```json
{
  "apis": [],
  "skills": ["<スキルの slug>"]
}
```

ここに置くと、手順がリポジトリと一緒に版管理され、clone した時点で有効になる。
リポジトリ固有の手順（リリース手順、そのリポジトリだけの決まり）はこちらが適切。

`agent-settings` の `set_repository` エンドポイントもあるが、
**`github_repository`（text 型）が送られず `400 github_repository required` になる**
（2026-08-22 時点）。直るまでは `.cyborgy/settings.json` を使う。

現在の設定は `project_settings()` で読める。

## 4. 既存スキルの中身を直す

**`update_skill(content=...)` / `put_skill_files` / `add_skill_file` は
すべて 409 になる。** サーバーが `expected_head_sha` を要求するが、MCP が
これを送らない（2026-08-22 時点）。

- **メタデータ（name / summary / description / tags）は `update_skill` で直せる**
- **本文（SKILL.md）は直せない**

本文を変えたい場合は、まだ誰も使っていないなら `delete_skill` して作り直す。
既に運用中なら作り直すと紐づけが切れるので、Web UI から直す。

## 5. 反映

スキルセットを変えたら `sync_workspace()` で手元に materialize する。

## 落とし穴のまとめ（2026-08-22 時点）

| やりたいこと | 使うもの | 注意 |
|---|---|---|
| 組織スキルセットに追加 | `agent-settings` / `set_organization` | 既存を `get` して丸ごと送り返す |
| リポジトリに追加 | `.cyborgy/settings.json` | `set_repository` は text 入力が落ちる |
| スキル本文の修正 | （不可） | 作り直すか Web UI |
| `call_api` で `endpoint_id` | plugin 側の MCP | 古い方は 405 |
