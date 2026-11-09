use super::{LlmResponse, ToolCallMsg};
use serde_json::Value;

pub fn validate_tool_call(call: &ToolCallMsg) -> Result<(), String> {
    if call.id.trim().is_empty() {
        return Err("Tool call id cannot be empty".into());
    }
    if call.name.trim().is_empty() {
        return Err("Tool call name cannot be empty".into());
    }
    if !call.arguments.is_object() {
        return Err(format!(
            "Tool {} arguments must be a JSON object",
            call.name
        ));
    }
    Ok(())
}

pub fn parse_openai_response(body: &Value) -> Result<LlmResponse, String> {
    let choices = body
        .get("choices")
        .and_then(Value::as_array)
        .ok_or("Missing choices array")?;
    let message = choices
        .first()
        .and_then(|choice| choice.get("message"))
        .ok_or("Missing choices[0].message")?;
    let text = match message.get("content") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(_) => return Err("Message content must be a string or null".into()),
    };
    let raw_calls = match message.get("tool_calls") {
        None | Some(Value::Null) => &[],
        Some(Value::Array(calls)) => calls.as_slice(),
        Some(_) => return Err("Message tool_calls must be an array".into()),
    };
    let mut tool_calls = Vec::new();
    for call in raw_calls {
        let id = required_string(call, "id", "Tool call")?;
        let function = call
            .get("function")
            .ok_or("Tool call function is missing")?;
        let name = required_string(function, "name", "Tool function")?;
        let arguments = function
            .get("arguments")
            .and_then(Value::as_str)
            .ok_or("Tool function arguments must be a JSON string")?;
        let arguments: Value = serde_json::from_str(arguments)
            .map_err(|error| format!("Invalid arguments for tool {name}: {error}"))?;
        let call = ToolCallMsg {
            id: id.into(),
            name: name.into(),
            arguments,
        };
        validate_tool_call(&call)?;
        tool_calls.push(call);
    }
    non_empty_response(text, tool_calls)
}

pub fn parse_anthropic_response(body: &Value) -> Result<LlmResponse, String> {
    let content = body
        .get("content")
        .and_then(Value::as_array)
        .ok_or("Missing Anthropic content array")?;
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                let value = block
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or("Anthropic text block is missing text")?;
                text.push_str(value);
            }
            Some("tool_use") => {
                let call = ToolCallMsg {
                    id: required_string(block, "id", "Anthropic tool_use")?.into(),
                    name: required_string(block, "name", "Anthropic tool_use")?.into(),
                    arguments: block
                        .get("input")
                        .cloned()
                        .ok_or("Anthropic tool_use input is missing")?,
                };
                validate_tool_call(&call)?;
                tool_calls.push(call);
            }
            Some(_) => {}
            None => return Err("Anthropic content block is missing a type".into()),
        }
    }
    non_empty_response(text, tool_calls)
}

fn required_string<'a>(value: &'a Value, field: &str, context: &str) -> Result<&'a str, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{context} {field} cannot be empty"))
}

fn non_empty_response(text: String, tool_calls: Vec<ToolCallMsg>) -> Result<LlmResponse, String> {
    if text.trim().is_empty() && tool_calls.is_empty() {
        return Err("Response contains neither text nor tool calls".into());
    }
    Ok(LlmResponse { text, tool_calls })
}
