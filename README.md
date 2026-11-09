<p align="center">
  <h1 align="center">MYOPC</h1>
  <p align="center"><strong>One Person Company</strong></p>
  <p align="center">
    Your AI-powered virtual team. Five agents, one command line, zero hiring.
  </p>
  <p align="center">
    你的 AI 虚拟团队 —— 一个人，也能开公司。
  </p>
</p>

<p align="center">
  <a href="https://github.com/CroTuyuzhe/opc_agent/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-Apache%202.0-green" alt="license"></a>
  <img src="https://img.shields.io/badge/LLM-OpenAI%20%7C%20Anthropic-purple" alt="LLM">
</p>

---

MYOPC 是一个运行在终端里的 **AI 多智能体协作框架**。你只需要一句话描述需求，MYOPC 会自动拆解任务，分配给 5 个专业角色 Agent，像一支真实团队一样协作完成：需求分析、技术实现、UI 设计、质量测试、项目管理。

> *"我想做一个网页贪吃蛇小游戏"*
>
> PM 追问你 5 轮需求细节 → 产出 PRD → Dev 写出可运行代码 → UI 输出设计规范 → Tester 生成测试用例 → Admin 归档总结 —— 全自动，你只需要做决策。

---

## Why MYOPC

**🏢 一个人就是一家公司** — 不用招人，不用开会。5 个 AI 角色各司其职，你是 Boss，做决策就好。

**🧠 Brain 智能协调** — 中枢大脑自动理解意图、拆解任务、编排依赖、串联流水线，复杂项目也能一键启动。

**🔁 真实团队协作模式** — 角色之间通过消息总线异步通信，有交接规范、有审批节点、有质量门禁，不是简单的 prompt chain。

**🔌 可扩展插件系统** — 从 GitHub 一键安装 Skill 插件，支持 Python / Bash，让你的团队能力无限延伸。

---

## 架构总览

```
                            ┌─────────────────────────┐
                            │       👤 You (Boss)      │
                            │   "做一个贪吃蛇小游戏"    │
                            └────────────┬────────────┘
                                         │
                                         ▼
                          ┌──────────────────────────────┐
                          │         🧠 Brain              │
                          │   意图识别 · 任务拆解 · 依赖编排  │
                          │   审批节点 · 知识注入 · Skill 调度 │
                          └──────────────┬───────────────┘
                                         │ dispatch
                                         ▼
               ┌─────────────────────────────────────────────────┐
               │   📬 Bus  ·  .myopc_agent/communication/           │
               │                                                  │
               │   inbox/ (待处理) → active/ (执行中) → archive/   │
               └──┬──────────┬──────────┬──────────┬──────────┬──┘
                  │          │          │          │          │
                  ▼          ▼          ▼          ▼          ▼
            ┌─────────┐┌─────────┐┌─────────┐┌─────────┐┌─────────┐
            │ 📋 PM   ││ 💻 Dev  ││ 🎨 UI   ││ 🧪 Test ││ 📊 Admin│
            │         ││         ││         ││         ││         │
            │ 需求分析 ││ 代码实现 ││ 设计规范 ││ 测试验证 ││ 项目管理 │
            │ Deepthink││write_file││write_file││ 报告   ││ 归档   │
            │ ask_user ││         ││         ││         ││         │
            └────┬────┘└────┬────┘└────┬────┘└────┬────┘└────┬────┘
                 │          │          │          │          │
                 ▼          ▼          ▼          ▼          ▼
            ┌─────────────────────────────────────────────────────┐
            │      📁 .myopc_agent/.artifacts/                       │
            │                                                     │
            │   {id}_pm.md    PRD 需求文档                         │
            │   {id}_dev.md   技术方案 + 代码文件                    │
            │   {id}_ui.md    设计规范                              │
            │   {id}_tester.md 测试报告                             │
            │   {id}_admin.md  项目总结                             │
            └─────────────────────────────────────────────────────┘
```

> 图中 `communication/`、`workspace/`、`staff/`、`memory_center/`、`skills/`、`.artifacts/` 等内部路径均位于项目根目录的 `.myopc_agent/` 下。

### 核心设计

| 模块 | 说明 |
|------|------|
| **Brain** | 意图识别与问答入口。开发任务只调用一次 `start_workflow`，不直接控制角色顺序 |
| **Orchestrator + SQLite** | 确定性管理 PM、可选 UI、Dev、Tester、Admin，持久化依赖、审批、执行尝试与失败恢复 |
| **Identity Engine** | 每个角色有独立的身份定义、能力边界、输出规范，确保各司其职不越界 |
| **Tool Loop** | 角色通过 LLM tool-calling 循环执行任务：调用工具 → 获取结果 → 继续推理，直到产出最终交付物 |
| **Execution Safety** | Shell/Skill 逐次默认拒绝审批，共享超时、进程组和日志管理；角色写入仅限代码根目录 |
| **Skill System** | 用户主动安装插件，Brain 自动发现；安装不执行依赖或入口脚本，调用前逐次审批 |

---

## Quick Start

### 安装

需要安装 stable Rust 工具链（包含 `cargo`）。在项目仓库根目录执行：

```bash
cargo install --path .

mkdir -p ~/.myopc
chmod 700 ~/.myopc
cp -n scaffold/myopc.json.example ~/.myopc/myopc.json
chmod 600 ~/.myopc/myopc.json
```

`cargo install` 会将 `myopc` 安装到 Cargo 的二进制目录（通常为 `~/.cargo/bin`）。请确保该目录已加入 `PATH`。scaffold 已嵌入二进制，安装后不依赖本仓库目录。

### 初始化项目

```bash
mkdir my-startup && cd my-startup
myopc init
```

初始化后，项目根目录下只会多出一个隐藏目录 `.myopc_agent/`，所有内部文件（团队协议、角色人设、消息总线、工作区、记忆库、配置）都收纳在里面：

```
my-startup/
└── .myopc_agent/          ← 所有 MYOPC 内部文件
    ├── AGENTS.md
    ├── staff/
    ├── communication/
    ├── workspace/
    └── memory_center/
```

`myopc init` 只生成团队目录，**不生成项目内配置文件**——配置统一使用全局 `~/.myopc/myopc.json`（init 不覆盖已有文件）。

### 配置 API Key

编辑全局配置 `~/.myopc/myopc.json`：

```json
{
    "provider": "openai",
    "api_key": "sk-your-api-key",
    "base_url": "https://api.openai.com/v1",
    "default_model": "gpt-4o",
    "team_root": ".myopc_agent",
    "max_tokens": 8192,
    "temperature": 0.7
}
```

> 兼容所有 OpenAI API 格式的服务（DeepSeek、智谱、Moonshot 等），只需修改 `base_url` 和 `default_model`。
> 推荐使用 `MYOPC_API_KEY` 环境变量。若配置文件包含 API Key，文件权限必须为 `0600`，所在目录必须为 `0700`，否则启动会停止并提示修复。

### 启动

```bash
myopc
```

问答和角色工作流在后台执行。运行期间仍可使用 `/status`、`/tasks`、`/help` 和 `/exit`；完成或失败通知会排队，在命令提示符安全输出，不会插入正在进行的审批输入。退出会终止受管子进程，未完成任务不会被标记为成功，遗留活动任务在下次启动时按中断处理。

```
┌──────────────────────────────────────┐
│ MYOPC Team Agent v0.1                  │
│                                      │
│   Provider   : openai                │
│   Model      : gpt-4o               │
│   Max Tokens : 8192                  │
│   Temperature: 0.7                   │
│   Config     : OK                    │
│                                      │
│   /help for commands, /exit to quit  │
└──────────────────────────────────────┘

myopc > 做一个网页贪吃蛇小游戏
```

### 交互示例

```
🧠 Brain: 识别为新功能需求，分发给 PM 进行需求分析...

📋 PM 开始执行任务 [a1b2c3d4]

  ? PM asks: 这个贪吃蛇游戏的目标平台是什么？
  → 移动端H5，手机浏览器直接玩

  ? PM asks: 游戏视觉风格偏好？
    (1) 像素复古风
    (2) 科技霓虹风  ← 你选了这个
    (3) 简约扁平风
    (4) 可爱卡通风

  ? PM asks: 操控方式需要支持哪些？
    (1) 键盘方向键
    (2) 触屏滑动
    (3) 键盘 + 触屏都要  ← 你选了这个

  ✅ PM 产出 PRD → .myopc_agent/.artifacts/a1b2c3d4_pm.md

💻 Dev 开始执行任务 [b2c3d4e5]
  📝 写入 code/index.html
  📝 写入 code/game.js
  📝 写入 code/style.css
  ✅ Dev 完成 → .myopc_agent/.artifacts/b2c3d4e5_dev.md

🎨 UI 产出设计规范 → .myopc_agent/.artifacts/c3d4e5f6_ui.md
🧪 Tester 产出测试报告 → .myopc_agent/.artifacts/d4e5f6g7_tester.md
📊 Admin 产出项目总结 → .myopc_agent/.artifacts/e5f6g7h8_admin.md

✅ 所有任务完成，产出物在 .myopc_agent/.artifacts/ 目录下
```

---

## 角色分工

| 角色 | 代号 | 职责 | 工具 | 核心产出 |
|------|------|------|------|----------|
| **Boss** | `boss` | 你本人。最终决策者，审批关键节点 | — | 需求指令、审批决策 |
| **PM** | `pm` | 需求分析。Deepthink 追问直到需求无歧义 | `ask_user` | PRD、用户故事 |
| **Dev** | `dev` | 技术实现。写真实可运行的代码 | `write_file` | 代码文件、技术方案 |
| **UI** | `ui` | 设计规范。输出完整的 Design System | `write_file` | 设计规范、组件 spec |
| **Tester** | `tester` | 质量保障。基于 PRD 编写测试用例 | — | 测试报告、BUG 报告 |
| **Admin** | `admin` | 项目管理。流程跟踪、归档、总结 | — | 周报、项目总结 |

### 权限层级

```
Boss (你)  ← 最高决策权，所有角色听从指令
  │
  ├── PM     ── 平级协作 ── Dev
  ├── UI     ── 平级协作 ── Tester
  └── Admin
```

角色之间**不能互相指挥**，只能通过消息总线请求协作，由 Brain 统一调度。

---

## 工作流

### 标准需求流转

```
你: "做一个用户登录功能"
 │
 ▼
[PM] 追问需求 (Deepthink)
 │  Q: 目标用户？ → Q: 登录方式？ → Q: 登录态有效期？ → ...
 │  全部确认后 → 产出 PRD
 │
 ├──▶ [Dev] 读取 PRD → 技术方案 → 写代码
 │
 ├──▶ [UI] 读取 PRD → 设计规范 → Design System
 │
 ▼
[Tester] 读取 PRD + 技术方案 → 测试计划 + 用例
 │
 ▼
[Admin] 全部完成 → 项目总结归档
 │
 ▼
你审阅所有产出 ✅
```

### Deepthink 追问协议

PM 不会凭空猜测需求。收到任务后进入 Deepthink 模式：

1. 分析请求，识别**所有缺失信息**
2. 通过 `ask_user` 工具，**逐个追问**（不会一次性抛出 10 个问题）
3. 第一个问题必问：**交付形态**（H5 / App / 小程序 / Web / CLI / API？）
4. 继续追问：目标用户、核心场景、验收标准、边界条件、技术约束...
5. 全部确认后才产出 PRD

### 人工审批节点

系统在关键节点会暂停，等你确认：

| 节点 | 触发条件 |
|------|----------|
| **PRD 审批** | PM 产出 PRD 后，必须你确认才能分发给 Dev/UI |
| **Shell 命令** | 每次展示完整命令、工作目录、用途和超时；仅输入 `y`/`yes` 执行，回车默认拒绝 |
| **Skill 调用** | 每次展示名称、来源、入口、参数、工作目录和超时；逐次批准，不继承 `MYOPC_API_KEY` |
| **范围变更** | 任何涉及范围扩大或成本增加的决策 |

---

## 项目结构

`myopc init` 后生成的目录（全部收纳在项目根目录的 `.myopc_agent/` 下）：

```
my-startup/
└── .myopc_agent/
    ├── AGENTS.md                     ← 团队协议（所有角色自动加载）
    ├── boundary.md                   ← 安全红线
    │
    ├── communication/
    │   ├── bus/{role}/inbox/         ← 待处理任务
    │   ├── bus/{role}/active/        ← 执行中任务
    │   └── handoff_protocols.md      ← 角色交接规范
    │
    ├── .artifacts/                   ← 角色产出物（PRD、设计、测试报告）
    │
    ├── workspace/                    ← 工作区（运行时目录）
    │
    ├── staff/
    │   ├── boss_decide/              ← Boss 身份 + 干预开关
    │   ├── pm_office/                ← PM 身份 + 规则
    │   ├── dev_forge/                ← Dev 身份 + 前后端规范
    │   ├── ui_ux_studio/             ← UI 身份 + 设计模板
    │   ├── tester/                   ← Tester 身份 + 测试规范
    │   └── admin_bp/                 ← Admin 身份 + 报告模板
    │
    ├── knowledge_base/               ← 静态知识库（设计规范、技术文档、公司调性）
    │
    ├── memory_center/
    │   ├── archive/{date}/           ← 已完成任务归档
    │   ├── user.md                   ← Boss 偏好记录
    │   └── reflection.log            ← Agent 自省日志
    │
    └── skills/                       ← 插件目录
        └── hello_world/              ← 示例插件
```

> 代码统一写入当前目录（`code_root: "."`），产出物写到 `.artifacts/`。

---

## 配置

### myopc.json

全局配置位于 `~/.myopc/myopc.json`（`myopc init` 不生成项目内配置）：

```json
{
    "provider": "openai",
    "api_key": "sk-your-api-key",
    "base_url": "https://api.openai.com/v1",
    "default_model": "gpt-4o",
    "team_root": ".myopc_agent",
    "max_tokens": 8192,
    "temperature": 0.7
}
```

| 字段 | 说明 | 默认值 |
|------|------|--------|
| `provider` | LLM 提供商 (`openai` / `anthropic`) | `openai` |
| `api_key` | API 密钥 | — |
| `base_url` | API 端点（兼容 OpenAI 格式的服务都能用） | `https://api.openai.com/v1` |
| `default_model` | 模型名称 | `gpt-4o` |
| `team_root` | 团队根目录（相对启动目录） | `.myopc_agent` |
| `code_root` | 代码根目录；`"."` 表示当前目录就是代码目录（默认值） | `.` |
| `max_tokens` | 单次回复最大 token 数 | `8192` |
| `temperature` | 生成温度 (0-2) | `0.7` |
| `llm_connect_timeout_seconds` | LLM 连接超时（秒） | `10` |
| `llm_request_timeout_seconds` | 每次 LLM 请求总超时（秒） | `180` |
| `llm_max_attempts` | 网络错误、429、5xx 的最大尝试次数（1–10） | `3` |

### 环境变量

```bash
export MYOPC_API_KEY="sk-your-api-key"
export MYOPC_PROVIDER="openai"
export MYOPC_MODEL="gpt-4o"
export MYOPC_BASE_URL="https://api.openai.com/v1"
export MYOPC_TEAM_ROOT=".myopc_agent"
export MYOPC_CODE_ROOT="."
export MYOPC_MAX_TOKENS="8192"
export MYOPC_TEMPERATURE="0.7"
export MYOPC_LLM_CONNECT_TIMEOUT_SECONDS="10"
export MYOPC_LLM_REQUEST_TIMEOUT_SECONDS="180"
export MYOPC_LLM_MAX_ATTEMPTS="3"
```

优先级：**环境变量** > 当前目录 `myopc.json` > `.myopc_agent/myopc.json` > 全局 `~/.myopc/myopc.json`（默认使用全局配置）

包含 API Key 的任一配置文件都必须使用 `0600` 权限，且其所在目录必须使用 `0700` 权限。

LLM 请求仅对网络错误、HTTP 429 和 5xx 进行指数退避重试，并遵循最长 30 秒的 `Retry-After`；其他 4xx 立即失败。缺失 `choices`、畸形工具参数、空工具标识及空成功响应都会返回明确错误。Brain 和每个角色最多执行 30 轮工具调用，不会静默截断对话历史。

### 使用其他 LLM

<details>
<summary><strong>Anthropic Claude</strong></summary>

```json
{
    "provider": "anthropic",
    "api_key": "sk-ant-your-key",
    "default_model": "claude-sonnet-4-20250514"
}
```

</details>

<details>
<summary><strong>DeepSeek</strong></summary>

```json
{
    "provider": "openai",
    "api_key": "your-deepseek-key",
    "base_url": "https://api.deepseek.com/v1",
    "default_model": "deepseek-chat"
}
```

</details>

<details>
<summary><strong>其他 OpenAI 兼容服务</strong></summary>

任何兼容 OpenAI API 格式的服务都可以使用，只需修改 `base_url` 和 `default_model`：

- 智谱 GLM、Moonshot Kimi、百川、通义千问...
- 本地部署的 Ollama、vLLM、LocalAI...

</details>

---

## 命令参考

| 命令 | 说明 |
|------|------|
| `myopc init` | 初始化团队项目目录 |
| `myopc` | 启动交互式终端 |
| `/help` | 显示帮助信息 |
| `/status` | 查看各角色状态（idle / working / queued） |
| `/tasks` | 列出所有任务 |
| `/inbox <role>` | 查看角色收件箱 |
| `/dispatch <role> <desc>` | 手动分发任务到指定角色 |
| `/resume <workflow_id>` | 继续等待中的工作流 |
| `/retry <task_id>` | 重试失败任务并保留历史尝试 |
| `/cancel <workflow_id>` | 终止工作流 |
| `/skills` | 列出已安装的技能插件 |
| `/skill install <url>` | 从 GitHub 安装 Skill 插件 |
| `/skill remove <name>` | 卸载插件 |
| `/exit` | 退出 |

---

## Skill 插件

### 安装

```
myopc > /skill install https://github.com/user/my-skill
```

安装命令会展示下载来源、目标目录和超时并要求确认。安装只下载及校验 `manifest.json`，不会自动运行入口脚本、`pip` 或其他依赖安装命令；需要依赖时请单独批准 Shell 命令。

### 开发自定义 Skill

在 `skills/` 下创建目录：

```
skills/my_tool/
├── manifest.json      ← 工具描述 + 参数定义
└── run.sh             ← 入口脚本（支持 Python / Bash）
```

`manifest.json` 示例：

```json
{
    "name": "my_tool",
    "description": "A custom tool that does something useful",
    "version": "0.1.0",
    "type": "bash",
    "entry": "run.sh",
    "timeout_seconds": 60,
    "parameters": {
        "type": "object",
        "properties": {
            "input": { "type": "string", "description": "Input data" }
        },
        "required": ["input"]
    }
}
```

Brain 会自动发现已安装的 Skill。每次调用都会展示完整参数和执行位置，回车或其他非 `y`/`yes` 输入都会拒绝执行；`timeout_seconds` 可配置为 1–600 秒，默认 60 秒。完整输出保存在 `.myopc_agent/logs/`，终端和模型只接收明确标记的限长摘要。

---

## License

[Apache License 2.0](LICENSE)

---

<p align="center">
  <strong>MYOPC — My One Person Company</strong><br>
  <em>一个人的公司，AI 驱动的团队。</em>
</p>
