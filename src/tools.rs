use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

pub fn tool_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "start_workflow".into(),
            description: "Start one deterministic software workflow. The system owns PM, optional UI, Dev, Tester, Admin, PRD approval, dependencies, retries, and failure handling.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "request": { "type": "string", "description": "Complete original user request and relevant context" }
                },
                "required": ["request"]
            }),
        },
        ToolDef {
            name: "get_project_status".into(),
            description: "Get current status of all agents and tasks".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "run_bash".into(),
            description: "Run a shell command. Executes automatically without confirmation; always include a description.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to run" },
                    "description": { "type": "string", "description": "Brief explanation of what this command does" }
                },
                "required": ["command"]
            }),
        },
        ToolDef {
            name: "ask_user".into(),
            description: "Ask the user a question. Provide 2-4 options with the RECOMMENDED choice FIRST (it becomes the default when the user presses Enter).".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "question": { "type": "string", "description": "Question or approval request" },
                    "options": {
                        "type": "array",
                        "description": "Selectable choices. Each: {label, value}. Omit for free-text.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "label": { "type": "string" },
                                "value": { "type": "string" }
                            }
                        }
                    }
                },
                "required": ["question"]
            }),
        },
    ]
}

pub fn skill_tool(name: &str, description: &str, parameters: Value) -> ToolDef {
    ToolDef {
        name: format!("skill_{}", name),
        description: description.to_string(),
        parameters,
    }
}

pub fn merge_tools(base: Vec<ToolDef>, extra: Vec<ToolDef>) -> Vec<ToolDef> {
    let mut out = base;
    out.extend(extra);
    out
}

pub fn to_openai_tools(tools: &[ToolDef]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters
                }
            })
        })
        .collect()
}

pub fn to_anthropic_tools(tools: &[ToolDef]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.parameters
            })
        })
        .collect()
}
