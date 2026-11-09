use crate::agent_runner::ToolExecutor;
use crate::config::Config;
use crate::identity::IdentityEngine;
use crate::llm::{LlmBackend, LlmResponse, Msg};
use crate::tools::ToolDef;
use std::sync::Arc;

const BRAIN_RULES: &str = "
You are MYOPC Brain, the central coordinator. Follow the AGENTS.md protocol and Handoff Protocols above.

Execution rules:
- Chat queries: answer directly, no tools.
- For every new feature or code change, call start_workflow exactly once with the complete original request. Never simulate role results yourself.
- The orchestrator, not you, owns PM → optional UI → Dev → Tester → Admin, PRD approval, dependencies, retries, and failure handling. Do not attempt to dispatch or execute individual role tasks.
- Wait for start_workflow to return and report its persisted status accurately. Never claim a failed or waiting workflow is complete.
- Use run_bash for shell commands. Always include a description. Commands run automatically — no confirmation needed.
- Installed skills (skill_*) are available. Use when relevant.
- PRD approval is mandatory and is handled by the orchestrator. Do not call ask_user to approve your own PRD.
- Artifacts: each role's deliverable (PRD, design spec, test report, code summary) is written automatically by the system to `.myopc_agent/.artifacts/{task_id}_{role}.md` after the role finishes. Do NOT write artifact files yourself — never create or move files under `.myopc_agent/.artifacts/` or `workspace/`. Just report results in your reply.
- Be concise.";

pub struct Brain {
    config: Config,
    llm: Arc<dyn LlmBackend>,
    identity: Option<IdentityEngine>,
    pub tools: Vec<ToolDef>,
    messages: Vec<Msg>,
    system_prompt_cache: Option<String>,
}

impl Brain {
    pub fn new(
        config: Config,
        llm: Arc<dyn LlmBackend>,
        identity: Option<IdentityEngine>,
        tools: Vec<ToolDef>,
    ) -> Brain {
        Brain {
            config,
            llm,
            identity,
            tools,
            messages: Vec::new(),
            system_prompt_cache: None,
        }
    }

    pub fn system_prompt(&mut self) -> String {
        if self.system_prompt_cache.is_none() {
            let mut parts: Vec<String> = Vec::new();
            if let Some(identity) = &self.identity {
                let agents_md = identity.load_agents_md();
                if !agents_md.is_empty() {
                    parts.push(agents_md);
                }
                let handoff = identity.load_handoff();
                if !handoff.is_empty() {
                    parts.push(handoff);
                }
                let boss = identity.load_boss_identity();
                if !boss.is_empty() {
                    parts.push(boss);
                }
            }
            parts.push(BRAIN_RULES.to_string());
            parts.push(format!(
                "Working directory is the project code root (code_root: {}). Dispatch role tasks directly; every write_file path is relative to this directory.",
                self.config.code_root.clone().unwrap_or_else(|| ".".into())
            ));
            self.system_prompt_cache = Some(parts.join("\n"));
        }
        self.system_prompt_cache.clone().unwrap_or_default()
    }

    pub async fn chat(
        &mut self,
        user_input: &str,
        executor: Arc<dyn ToolExecutor>,
    ) -> Result<String, String> {
        self.messages.push(Msg::User(user_input.to_string()));
        if self.config.provider == "anthropic" {
            return self.chat_anthropic(executor).await;
        }
        self.chat_openai(executor).await
    }

    async fn chat_openai(&mut self, executor: Arc<dyn ToolExecutor>) -> Result<String, String> {
        loop {
            let resp: LlmResponse = self
                .llm
                .complete(&self.messages, &self.tools, &self.config)
                .await?;
            if resp.tool_calls.is_empty() {
                self.messages.push(Msg::Assistant(resp.text.clone()));
                return Ok(resp.text);
            }
            self.messages
                .push(Msg::AssistantToolCalls(resp.tool_calls.clone()));
            for tc in resp.tool_calls {
                let result = executor.execute(&tc.name, &tc.arguments).await;
                self.messages.push(Msg::ToolResult {
                    id: tc.id,
                    content: result,
                });
            }
        }
    }

    async fn chat_anthropic(&mut self, executor: Arc<dyn ToolExecutor>) -> Result<String, String> {
        // Mirror TS behavior: tool call history is kept in a local list, only
        // user/assistant text is persisted across turns.
        let mut api_messages: Vec<Msg> = self
            .messages
            .iter()
            .filter(|m| matches!(m, Msg::User(_) | Msg::Assistant(_)))
            .cloned()
            .collect();
        loop {
            let resp: LlmResponse = self
                .llm
                .complete(&api_messages, &self.tools, &self.config)
                .await?;
            if resp.tool_calls.is_empty() {
                self.messages.push(Msg::Assistant(resp.text.clone()));
                return Ok(resp.text);
            }
            api_messages.push(Msg::AssistantToolCalls(resp.tool_calls.clone()));
            for tc in resp.tool_calls {
                let result = executor.execute(&tc.name, &tc.arguments).await;
                api_messages.push(Msg::ToolResult {
                    id: tc.id,
                    content: result,
                });
            }
        }
    }

    pub fn reset(&mut self) {
        self.messages.clear();
    }

    pub fn set_tools(&mut self, tools: Vec<ToolDef>) {
        self.tools = tools;
    }
}
