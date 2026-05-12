use genai::chat::{
    Tool as GenAiTool, ToolCall as GenAiToolCall, ToolResponse as GenAiToolResponse,
};

use crate::runtime::tools::{
    RuntimeToolCall, RuntimeToolDefinition, RuntimeToolRegistry, RuntimeToolResult,
};

pub(crate) fn genai_tools_for_registry(registry: &RuntimeToolRegistry) -> Vec<GenAiTool> {
    registry
        .definitions()
        .map(genai_tool_for_definition)
        .collect()
}

pub(crate) fn runtime_tool_call_from_genai(tool_call: GenAiToolCall) -> RuntimeToolCall {
    RuntimeToolCall::new(tool_call.call_id, tool_call.fn_name, tool_call.fn_arguments)
}

pub(crate) fn genai_tool_response_from_runtime(result: &RuntimeToolResult) -> GenAiToolResponse {
    GenAiToolResponse::new(result.call_id.clone(), result.content.clone())
}

fn genai_tool_for_definition(definition: &RuntimeToolDefinition) -> GenAiTool {
    let mut tool = GenAiTool::new(definition.name.clone());
    if let Some(description) = definition.description.as_ref() {
        tool = tool.with_description(description.clone());
    }
    if let Some(schema) = definition.input_schema.as_ref() {
        tool = tool.with_schema(schema.clone()).with_strict(true);
    }
    tool
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::tools::{RuntimeToolDefinition, RuntimeToolRegistry};

    #[test]
    fn maps_runtime_tools_to_genai_tools_in_stable_order() {
        let mut registry = RuntimeToolRegistry::new();
        registry
            .insert(RuntimeToolDefinition::new("write_file").with_description("Write a text file"));
        registry.insert(
            RuntimeToolDefinition::new("read_file")
                .with_description("Read a text file")
                .with_input_schema(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" }
                    },
                    "required": ["path"]
                })),
        );

        let tools = genai_tools_for_registry(&registry);

        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name.as_str(), "read_file");
        assert_eq!(tools[0].description.as_deref(), Some("Read a text file"));
        assert_eq!(
            tools[0]
                .schema
                .as_ref()
                .and_then(|schema| schema.get("type")),
            Some(&serde_json::json!("object"))
        );
        assert_eq!(tools[0].strict, Some(true));
        assert_eq!(tools[1].name.as_str(), "write_file");
        assert_eq!(tools[1].strict, None);
    }

    #[test]
    fn maps_genai_tool_call_to_runtime_tool_call() {
        let call = GenAiToolCall {
            call_id: "call-1".to_string(),
            fn_name: "read_file".to_string(),
            fn_arguments: serde_json::json!({ "path": "Cargo.toml" }),
            thought_signatures: Some(vec!["sig".to_string()]),
        };

        let runtime_call = runtime_tool_call_from_genai(call);

        assert_eq!(runtime_call.call_id, "call-1");
        assert_eq!(runtime_call.name, "read_file");
        assert_eq!(
            runtime_call.arguments,
            serde_json::json!({ "path": "Cargo.toml" })
        );
    }

    #[test]
    fn maps_runtime_tool_result_to_genai_tool_response() {
        let result = RuntimeToolResult::success("call-1", "file content");

        let response = genai_tool_response_from_runtime(&result);

        assert_eq!(response.call_id, "call-1");
        assert_eq!(response.content, "file content");
    }
}
