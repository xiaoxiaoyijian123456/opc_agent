# AGENTS.md — MYOPC Team Operating Protocol

> 本文件是所有 Agent 的初始化必读文档。Brain 和每个角色 Agent 在启动时自动加载此文件作为基础协议。

---

## 1. 角色一览

| ID | 角色 | 定位 | 核心产出 |
|----|------|------|----------|
| `boss` | Boss | 人类决策者，最终审批权 | 需求指令、审批决策 |
| `pm` | 产品经理 | 需求分析与定义，deepthink 追问直到需求无歧义 | PRD、用户故事、竞品分析 |
| `dev` | 研发工程师 | 架构设计与代码实现 | 技术方案、代码、接口文档 |
| `ui` | UI/UX 设计师 | 视觉与交互设计 | 设计规范、组件 spec、交互流程 |
| `tester` | 测试工程师 | 质量保障 | 测试计划、用例、BUG 报告 |
| `admin` | 行政 BP | 流程与协调 | 周报、会议纪要、流程文档 |

**权限层级**: `boss > pm = dev = ui = tester = admin`

Boss 的指令对所有角色具有最高优先级。角色之间为平级协作关系，不能互相指挥；标准任务流由 orchestrator 统一推进。

---

## 2. 消息系统 (BUS)

### 2.1 消息流转机制

所有角色之间**不直接对话**。任务、依赖、执行尝试和状态以 `.myopc_agent/myopc.db` 中的 SQLite 记录为准。

**发送流程**:
```
Brain → start_workflow(request) → orchestrator 创建 PM 任务并持久化状态
```

**接收与决策流程**:
```
orchestrator 读取当前任务
  → 创建独立执行尝试
  → 成功后按冻结模板创建唯一下游任务
  → 失败则停止下游，等待 Boss 重试、修改或终止
```

**任务生命周期**:
```
pending → active → completed
               └→ failed → pending（显式重试，保留历史尝试）

PRD 完成后：waiting_approval → active / cancelled
```

### 2.2 Decision Rules — 信箱决策规则

每个角色只接受与其职责匹配的任务。orchestrator 根据确定性模板和 PRD 审批结果推进：

#### PM 信箱
| 消息类型 | 触发条件 | 处理方式 |
|----------|----------|----------|
| `requirement` | 用户提出功能需求 | deepthink 追问 → 产出 PRD |
| `research` | 需要调研分析 | 竞品/用户/市场分析报告 |
| `review_request` | 其他角色请求产品确认 | 审核并回复决策 |

#### Dev 信箱
| 消息类型 | 触发条件 | 处理方式 |
|----------|----------|----------|
| `implement` | PRD 已完成，需要开发 | 技术方案 → 代码实现 |
| `bugfix` | BUG 报告需要修复 | 定位 → 修复 → 输出补丁 |
| `tech_review` | 技术方案评审 | 输出评审意见 |

#### UI 信箱
| 消息类型 | 触发条件 | 处理方式 |
|----------|----------|----------|
| `design` | 需要视觉/交互设计 | 设计规范 → 组件 spec |
| `design_review` | 设计稿评审 | 输出修改建议 |

#### Tester 信箱
| 消息类型 | 触发条件 | 处理方式 |
|----------|----------|----------|
| `test_plan` | 功能开发完成 | 编写测试计划和用例 |
| `bug_verify` | BUG 修复后验证 | 回归测试 → 通过/不通过 |

#### Admin 信箱
| 消息类型 | 触发条件 | 处理方式 |
|----------|----------|----------|
| `report` | 需要周报/汇总 | 汇总信息 → 输出报告 |
| `coordinate` | 跨角色协调 | 梳理依赖 → 同步各方 |

### 2.3 消息路由实现

Brain 是唯一的路由中心，路由逻辑如下：

```
用户输入
  │
  ├─ 普通聊天 → Brain 直接回复
  │
  └─ 执行意图 → Brain 分析任务类型（所有任务必须经过 PM → Dev）
       │
       ├─ 需求/规划类 → dispatch to PM (type: requirement)
       ├─ 开发/代码类 → 必须先 dispatch to PM (type: requirement) 产出 PRD，
       │                 PRD 完成后 dispatch to Dev (type: implement, 依赖 PM 任务)
       ├─ 设计/视觉类 → 依赖 PM PRD 完成后 dispatch to UI (type: design)
       ├─ 测试/质量类 → 依赖 Dev 实现完成后 dispatch to Tester (type: test_plan)
       ├─ 流程/报告类 → dispatch to Admin (type: report)
       │
       └─ 复合任务 → Brain 拆解为多个子任务，按依赖顺序串行分发
            例: "做一个登录功能"
            → Step 1: PM 写 PRD (requirement)
            → Step 2: UI 出设计（仅 requires_ui=true）— 依赖 Step 1
            → Step 3: Dev 写技术方案与代码 — 依赖前一阶段
            → Step 4: Tester 验证，Admin 总结
```

**强制流水线**：任何执行类任务都不得直接派发给 Dev/UI/Tester/Admin，必须先经过 PM 产出 PRD，再按依赖顺序进入后续角色。

**角色间协作消息**: 角色不得自行创建下游任务；需要变更流程时报告 orchestrator 并等待 Boss 决策：
```
Dev 完成代码 → orchestrator 创建 Tester 任务
Tester 发现阻断问题 → 当前任务失败并暂停，不自动创建 bugfix 循环
```

---

## 3. 核心工作流

### 3.1 标准需求流转

```
Boss: "做一个用户登录功能"
  │
  ▼
[PM] requirement — deepthink 追问
  │  Q: 目标用户是谁？
  │  Q: 登录方式有哪些？
  │  Q: 登录态有效期？
  │  ...追问到信息充足
  │  → 产出: PRD (.artifacts/{id}_pm.md)
  │
  ▼
[Dev] implement — 基于 PRD 写技术方案和代码
  │  读取 PM 的 PRD
  │  → 产出: 技术方案 + 代码 (.artifacts/{id}_dev.md)
  │
  ├──▶ [UI] design — 并行，基于 PRD 出设计
  │      → 产出: 设计规范 (.artifacts/{id}_ui.md)
  │
  ▼
[Tester] test_plan — Dev 完成后
  │  读取 PRD + 技术方案
  │  → 产出: 测试计划 + 用例 (.artifacts/{id}_tester.md)
  │
  ▼
[Admin] report — 全部完成后
  │  → 产出: 项目总结 (.artifacts/{id}_admin.md)
  │
  ▼
Boss 审阅所有产出
```

### 3.2 BUG 修复流转

```
Boss: "登录页验证码倒计时有 BUG"
  │
  ▼
[Dev] bugfix
  │  定位问题 → 输出修复方案 + 代码补丁
  │  → 产出: bugfix patch (.artifacts/{id}_dev.md)
  │
  ▼
[Tester] bug_verify
  │  验证修复是否生效 + 回归测试
  │  → 产出: 验证报告 (.artifacts/{id}_tester.md)
  │
  ├─ 通过 → 归档完成
  └─ 不通过 → 重新 dispatch to Dev (type: bugfix)
```

---

## 4. 文件约定

### 4.1 目录结构

```
MYOPC_Team/
├── AGENTS.md                          ← 本文件，初始化必读
├── BOUNDARY.md                        ← 安全红线，所有角色强制遵守
│                                       （配置使用全局 ~/.myopc/myopc.json，init 不生成）
│
├── myopc.db                           ← SQLite 任务与协作状态
├── myopc.lock                         ← 单项目执行实例锁
│
├── .artifacts/                        ← 所有角色的产出物
│   └── {task_id}_{role}.md
│
├── workspace/                         ← 项目工作区（注册表、临时文件等）
│
├── staff/                             ← 角色身份定义
│   └── {role}/{role}_identity.md
│
├── knowledge_base/                    ← 静态知识库
│
├── memory_center/                     ← 持久记忆
│   ├── archive/{date}/                ← 已完成任务归档
│   ├── user.md                        ← Boss 偏好记录
│   └── reflection.log                 ← Agent 自省日志
│
└── skills/                            ← 插件技能目录
    └── {skill_name}/manifest.json
```

**代码位置**：所有代码/文件写入当前目录（code root）。写文件路径必须直接相对于当前目录（如 `cmd/server/main.go`），**不要创建项目名子目录**（如 `gin-scaffold/`），也不要加 `workspace/` 前缀。

### 4.2 任务 JSON 格式

```json
{
  "id": "a1b2c3d4",
  "from": "boss",
  "to": "pm",
  "type": "requirement",
  "title": "用户登录功能",
  "content": "详细需求描述...",
  "status": "pending",
  "ts": "2026-04-23T10:00:00Z"
}
```

**type 字段枚举**: `requirement`, `research`, `review_request`, `implement`, `bugfix`, `tech_review`, `design`, `design_review`, `test_plan`, `bug_verify`, `report`, `coordinate`

### 4.3 产出物命名

所有角色产出写入 `.artifacts/`，命名规则: `{task_id}_{role}.md`

---

## 5. 扩展机制

### 5.1 Skill 插件系统

在 `skills/` 下创建目录，包含 `manifest.json` + `run.sh`，模型可以发现但不得自行安装、更新或删除。
只有用户可通过 `/skill install <repo_url>` 主动安装；安装不自动执行入口或依赖。每次 Skill 调用和每条 Shell 命令均须由用户当次明确输入 `y`/`yes`，回车默认拒绝，不得提供会话级豁免。

### 5.2 记忆进化

`memory_center/reflection.log` 记录 Agent 的自省日志。每次任务完成后，Brain 可选择性追加一行反思（什么做对了、什么可以改进），用于长期优化决策质量。

### 5.3 知识库扩展

`knowledge_base/` 放入技术文档、设计规范、公司调性等静态知识，角色 Agent 执行任务时可按需读取，避免每次重复描述上下文。

### 5.4 协作链自动化

Brain 只调用一次 `start_workflow`。orchestrator 独占 PM、可选 UI、Dev、Tester、Admin 的依赖推进、PRD 审批、失败暂停和重试；角色与 Brain 均不得绕过。
