---
name: "cyborgy-api-without-mcp"
description: "Cyborgy \u306e MCP \u30c4\u30fc\u30eb\u304c\u4f7f\u3048\u306a\u3044\u30fb\u30c4\u30fc\u30eb\u4e00\u89a7\u306b\u76ee\u7684\u306e\u30c4\u30fc\u30eb\u304c\u7121\u3044\u3068\u304d\u3002MCP \u304c\u6b63\u5e38\u306a\u3089\u4f7f\u308f\u306a\u3044\u3002"
---

# MCP が落ちていても Cyborgy は使える

**⚠️ これは一時的な回避策である。** 2026-08-22 時点で Cyborgy の MCP が
セッション中に切断される事象が起きている。**MCP が正常なら、いつもどおり
MCP のツール（`create_task` など）を使うこと。** この文書は、MCP が
使えないときにだけ開く。

## 結論を先に

**「MCP のツール一覧に無い」は「機能が無い」ではない。**
実体は Python パッケージ `cyborgy.api.Api` にあり、CLI で保存された
トークンでそのまま認証が通る。

```python
import sys
sys.path.insert(0, "/home/yusuke/.pyenv/versions/3.10.14/lib/python3.10/site-packages")
from cyborgy.api import Api

a = Api()                      # 引数なしで CLI のトークンを使う
a.list_task_projects()
```

パスは環境で変わる。`python -c "import cyborgy, os; print(os.path.dirname(cyborgy.__file__))"`
で確かめる。**`cyborgy` CLI が動く Python を使うこと**
（`head -1 $(command -v cyborgy)` で分かる）。

## よく使うもの

```python
# タスク
a.list_task_projects()
a.list_tasks(project_id)
a.create_task({"project_id": pid, "name": "...", "summary": "...", "status": "done"})
a.update_task(task_id, {"status": "done"})

# ファイルの添付（25MiB まで。base64 にせず、そのまま送られる）
a.upload_task_file(task_id, "/path/to/file.md", "file.md", "text/markdown")
a.list_task_files(task_id)

# タスク同士の紐づけ
a.set_related_tasks(task_id, [other_id, ...])
a.set_task_dependencies(task_id, [depends_on_id, ...])
a.update_task_parent(task_id, parent_id)

# スキルとスキルセット
a.search_skills(q="...")
a.get_skill(skill_id)
a.get_skill_files(skill_id)          # -> {"files": [{"path":..., "content":...}]}
a.create_skill({"name":..., "summary":..., "description":..., "tags":[...], "visibility":"private"})
a.put_skill_files(skill_id, [{"path": "SKILL.md", "content": "..."}])   # **list である。dict を渡すと 400 bad json**
a.get_agent_settings()
a.set_agent_settings(organization_skill_ids=[...])   # **全置き換え。今の一覧に足してから渡す**
```

**何ができるかは `api.py` を見れば分かる。**

```bash
P=$(python -c "import cyborgy,os;print(os.path.dirname(cyborgy.__file__))")
grep -n "def .*task" $P/api.py
grep -n "^    def " $P/api.py | head -80
```

## CLI で足りることもある

```bash
cyborgy whoami
cyborgy agent-settings show                  # 全設定を JSON で
cyborgy agent-settings set-organization ...  # Organization Skill Set を置き換え
cyborgy skills                               # スキルの閲覧・導入
```

**ただし CLI に task のサブコマンドは無い。** タスクは上の Python 経由。

## 踏んだ穴

- `put_skill_files` の `files` は **list**。`{"SKILL.md": "..."}` を渡すと
  `400 bad json` になる。正しくは `[{"path": "SKILL.md", "content": "..."}]`
- **`set_agent_settings` は全置き換え。しかも読み取りのキーが入れ子である。**
  `get_agent_settings()` が返すのは `{base, organization, repositories}` で、
  一覧は `s["organization"]["skill_ids"]` にある。
  **`s["organization_skill_ids"]` はトップレベルには無い。** ここを読み違えると
  空リストが返り、それに足して書き込んで**既存を全部消す**。

  ```python
  s = a.get_agent_settings()
  cur = s["organization"]["skill_ids"]          # ← ここ
  assert cur, "空なら読み取りを疑う。いきなり書かない"
  a.set_agent_settings(organization_skill_ids=cur + [new_id])
  ```

  2026-08-22 に**実際に 28 個を 1 個に消した。** 直前の調査で 28 個を
  列挙していたので順序ごと戻せたが、控えが無ければ復旧できなかった。
  **書く前に必ず今の一覧を出力しておくこと。**

- `put_skill_files` は 2 回目以降 `expected_head_sha` が要る（無いと 409）。
  `get_skill_files()` が返す `head_sha` をそのまま渡す
- MCP サーバーを直接起動して `tools/list` を見ても**当てにならない**。
  `cyborgy.mcp_server` は 21 ツールしか出さないのに、セッションでは同じ
  コマンドで 73 ツール見えていた。**無いように見えても api.py にはある**

## 動いていないなら、コマンドで起動する

**「Worker がオンラインでないのでできません」「MCP が使えないのでできません」で
止まらないこと。** どちらもコマンドで起動できる。止まる前に起動する。

### Worker が1台もオンラインでないとき

`online-workers` が全部 `accepts_new_sessions=false` なら、単にこの PC で
Worker Manager が動いていない。起動する。

```bash
cyborgy worker start        # 既存が stale なら置き換えて起動する
cyborgy worker restart      # 応答しない Manager を入れ替える
```

起動後 `online-workers` を取り直してから Session を立てる。**起動せずに
「レビュアーを起動できない」と報告して終わらない。**

**`cyborgy` が壊れていることがある。** 共有の `/home/yusuke/tools/bin/cyborgy`
は `ModuleNotFoundError: No module named 'cyborgy'` になることがある
（venv に本体が入っていない）。その場合は動く checkout の venv を使う。

```bash
find /home/yusuke -maxdepth 6 -path '*/bin/cyborgy' -type f 2>/dev/null
<checkout>/.venv/bin/cyborgy whoami      # 通るものを選ぶ
```

### dev QA の Worker も同じ

dev QA の被試験 Worker は**ローカル PC の Worker ではなく Docker コンテナ**で、
これもコマンドから起動する。手順は `docs/qa/dev-deployment-qa.md`。

```bash
./scripts/test-dev-docker-worker.sh
./scripts/start-dev-docker-worker.sh --run-id <一意なRun ID> \
  --worker-name <一意なWorker名> --receipt <private receipt path> \
  --expected-sha "$(git rev-parse HEAD)" --expected-tree "$(git rev-parse HEAD^{tree})"
```

Codex の枠が切れているときは `--codex-home <別の account home>` で別アカウントを
割り当てて起動し直す。**枠切れを理由に QA を省略しない。**

dev デプロイ自体（`scripts/deploy-release.sh dev release-manifest.json`）は
Worker を必要としない。Worker が要るのは独立レビューと QA だけなので、
そこを混同して全体を止めない。

## やってはいけないこと

- **「MCP が落ちているのでできません」で止まること。** api.py を見る前に
  結論を出さない（2026-08-22 に実際にやって叱られた）
- **「Worker がオンラインでないのでできません」で止まること。**
  `cyborgy worker start` で起動してから判断する（2026-08-24）
- トークンを読み出してログや出力へ書くこと。`Api()` に任せる
- **MCP が生きているのにこの方法を使うこと。** MCP が正常ならそちらが正
