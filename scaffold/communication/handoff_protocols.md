# Handoff Protocols — 角色交接规范

> 本文件定义角色之间如何交接任务与产出物。Brain 与所有角色 Agent 启动时自动加载。
> 角色之间不直接对话，所有交接通过 BUS 消息 + `workspace/artifacts/` 文件完成。

---

## 1. 交接总原则

1. **单一来源**：上游结论只写进 artifact 文件，下游只读文件，不依赖对话记忆。
2. **最小上下文**：Brain 分发任务时只摘录下游需要的章节，并标注来源。
3. **单向推进**：任务按依赖顺序串行分发，下游不得在依赖未完成时开工。
4. **不改他人产出**：角色只能写自己的 artifact 与代码文件，发现上游问题走 `review_request` 回传。
5. **有据可查**：每个结论必须能追溯到 PRD 条款或验收标准编号。

---

## 2. 标准交接链

```
Boss 指令
  │
  ▼
[PM] requirement      → workspace/artifacts/{id}_pm.md       （PRD）
  │  审批门：PRD Review
  ├──────────────▶ [Dev] implement   → {id}_dev.md + code/    （技术方案 + 代码）
  └──────────────▶ [UI]  design      → {id}_ui.md + code/     （设计规范 / design_system.md）
                          │
                          ▼
              [Tester] test_plan     → {id}_tester.md         （测试计划 + 用例 + 结论）
                          │
                          ▼
              [Admin]  report        → {id}_admin.md          （项目总结归档）
```

依赖关系：`Dev`、`UI` 依赖 `PM`；`Tester` 依赖 `Dev`（若 UI 已交付则同时参考）；`Admin` 依赖全部完成。

---

## 3. 各环节交接契约

### 3.1 PM → Dev / UI

必须包含：
- PRD 全文路径：`workspace/artifacts/{pm_task_id}_pm.md`
- 本次任务的验收标准编号（如 AC-01, AC-03）
- 范围边界（明确 out-of-scope）
- 技术或设计约束（若有）

Dev/UI **不得**自行扩大范围；缺信息时通过 Brain 向 PM 发 `review_request`。

### 3.2 PM → UI

额外要求：设计规范必须落到 `workspace/code/design_system.md`（或项目 code 目录同名文件），包含：
- Color / Typography / Spacing / Radius / Motion token
- 组件级 spec（状态、尺寸、交互）
- 可访问性（对比度、焦点态、触控热区）

### 3.3 Dev → Tester

必须包含：
- 代码根目录路径
- 实现清单（写过哪些文件、入口文件是哪个）
- 与 PRD 的差异点（未实现 / 降级实现要显式声明）
- 本地启动方式

Tester **不读代码猜测行为**，以 PRD 验收标准 + Dev 声明的差异点为准。

### 3.4 Tester → Dev（BUG 回路）

每个 BUG 必须结构化：
```
BUG-{序号}
- 关联验收标准: AC-xx
- 严重级别: Blocker / Major / Minor / Cosmetic
- 复现步骤: 1) ... 2) ...
- 期望结果 / 实际结果
- 建议修复方向
```

### 3.5 任意角色 → Admin

Admin 只做汇总，不重新判断技术结论。上游需提供：
- 各阶段 artifact 路径
- 遗留问题与风险
- 关键决策记录

---

## 4. 消息（dispatch）字段约定

| 字段 | 约定 |
|------|------|
| `role` | `pm` / `dev` / `ui` / `tester` / `admin` |
| `msg_type` | 见 AGENTS.md 2.2 Decision Rules，如 `requirement` / `implement` / `design` / `test_plan` / `bugfix` / `bug_verify` / `report` |
| `title` | 动词开头，一句话说明交付目标 |
| `content` | 任务描述 + `[PRD excerpt]` / `[Design System excerpt]` 摘录，禁止整篇粘贴 |
| `depends_on` | 上游 task id 列表，必须真实存在 |
| `artifacts` | 下游需要读取的 artifact 相对路径 |

---

## 5. 质量门禁

| 门禁 | 负责 | 通过条件 |
|------|------|----------|
| PRD Review | Boss | 验收标准可测、范围清晰、无待定项 |
| Tech Review | Boss | 技术选型与架构影响已确认 |
| Design Review | Boss | 设计规范覆盖 PRD 全部界面需求 |
| Test Gate | Tester | 无 Blocker / Major 未闭环；否则退回 Dev `bugfix` |
| Launch Readiness | Boss | 全部 artifact 齐备，遗留问题已记录 |

未通过门禁不得流转到下一环节；Brain 必须在门禁处调用 `ask_user` 等待 Boss 决策。

---

## 6. 失败与重试

- Agent 执行失败：任务标记完成并写入 `ERROR:` 摘要，同时在 `memory_center/archive/{date}/` 生成 `{task_id}_{role}_reflection.json`。
- 重试策略：同一任务最多重试 2 次；仍失败则上报 Boss，附失败原因与已产出部分。
- 下游发现上游产出不可用：退回上游而不是自行补齐。

---

## 7. 产出物命名与存放

| 类型 | 路径 |
|------|------|
| 角色文档 | `workspace/artifacts/{task_id}_{role}.md` |
| 多项目模式 | `workspace/{project}/artifacts/{task_id}_{role}.md` |
| 代码 | 项目 `code/` 目录（由 `create_project` 决定，默认 `workspace/{project}/code/`） |
| 归档 | `memory_center/archive/{YYYY-MM-DD}/{task_id}.json` |

命名一经写入不得重命名或覆盖他人文件；同一文件的多版本以新 task id 区分。
