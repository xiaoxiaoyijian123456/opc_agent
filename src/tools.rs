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
            name: "dispatch_task".into(),
            description: "Send a task to a role's inbox per handoff protocol".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "role": { "type": "string", "enum": ["pm", "dev", "ui", "tester", "admin"] },
                    "msg_type": { "type": "string", "description": "Message type from Decision Rules" },
                    "title": { "type": "string" },
                    "content": { "type": "string", "description": "Task description. Include [PRD excerpt] and/or [Design System excerpt] with relevant contract sections." },
                    "depends_on": { "type": "array", "items": { "type": "string" }, "description": "Upstream task IDs" },
                    "artifacts": { "type": "array", "items": { "type": "string" }, "description": "Referenced artifact file paths" }
                },
                "required": ["role", "msg_type", "title", "content"]
            }),
        },
        ToolDef {
            name: "read_workspace_file".into(),
            description: "Read a file from the workspace directory".into(),
            parameters: json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
        },
        ToolDef {
            name: "write_workspace_file".into(),
            description: "Write content to a file in the workspace directory".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            }),
        },
        ToolDef {
            name: "get_project_status".into(),
            description: "Get current status of all agents and tasks".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "execute_role_task".into(),
            description: "Have a role agent pick up and execute its next inbox task".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "role": { "type": "string", "enum": ["pm", "dev", "ui", "tester", "admin"] },
                    "task_id": { "type": "string" }
                },
                "required": ["role", "task_id"]
            }),
        },
        ToolDef {
            name: "run_bash".into(),
            description: "Run a shell command. User will be asked to approve before execution.".into(),
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
            name: "list_projects".into(),
            description: "List all existing projects in workspace. Call before create_project to check for related/duplicate projects.".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "create_project".into(),
            description: "Create a new project. Call list_projects first to check for related projects.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Project name (directory name, lowercase, no spaces)" },
                    "location": { "type": "string", "enum": ["workspace", "desktop"], "description": "Where to create code directory" },
                    "parent": { "type": "string", "description": "Name of existing parent project for sibling placement" },
                    "title": { "type": "string", "description": "Human-readable project title" },
                    "keywords": { "type": "array", "items": { "type": "string" }, "description": "Keywords for future project matching" }
                },
                "required": ["name", "location"]
            }),
        },
        ToolDef {
            name: "ask_user".into(),
            description: "Ask the user a question or request approval. Provide options for common choices.".into(),
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
