use serde_json::Value;
use std::fmt;

/// `ToolSchema` 包装工具参数 JSON Schema。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSchema {
    pub value: Value,
}

impl ToolSchema {
    /// `new` 创建工具参数 schema。
    pub fn new(value: Value) -> Self {
        Self { value }
    }
}

/// `ToolSchemaError` 描述工具参数不符合 JSON Schema 的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSchemaError {
    path: String,
    message: String,
}

impl ToolSchemaError {
    fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ToolSchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.path, self.message)
    }
}

impl std::error::Error for ToolSchemaError {}

pub(crate) fn validate_tool_arguments(
    schema: &Value,
    arguments: &Value,
) -> Result<(), ToolSchemaError> {
    validate_value(schema, arguments, "$")
}

fn validate_value(schema: &Value, value: &Value, path: &str) -> Result<(), ToolSchemaError> {
    if let Some(types) = schema_types(schema)
        && !types
            .iter()
            .any(|expected| value_matches_type(value, expected))
    {
        return Err(ToolSchemaError::new(
            path,
            format!("must match type {}", types.join(" or ")),
        ));
    }

    validate_object_keywords(schema, value, path)?;
    validate_array_keywords(schema, value, path)?;
    validate_numeric_keywords(schema, value, path)?;
    Ok(())
}

fn schema_types(schema: &Value) -> Option<Vec<String>> {
    match schema.get("type")? {
        Value::String(value) => Some(vec![value.clone()]),
        Value::Array(values) => Some(
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
        ),
        _ => None,
    }
}

fn value_matches_type(value: &Value, expected: &str) -> bool {
    match expected {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => true,
    }
}

fn validate_object_keywords(
    schema: &Value,
    value: &Value,
    path: &str,
) -> Result<(), ToolSchemaError> {
    let has_object_keywords = schema.get("properties").is_some()
        || schema.get("required").is_some()
        || schema.get("additionalProperties").is_some();
    if !has_object_keywords {
        return Ok(());
    }

    let Some(object) = value.as_object() else {
        return Ok(());
    };
    let properties = schema.get("properties").and_then(Value::as_object);

    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for required_key in required.iter().filter_map(Value::as_str) {
            if !object.contains_key(required_key) {
                return Err(ToolSchemaError::new(
                    child_path(path, required_key),
                    "is required",
                ));
            }
        }
    }

    if schema.get("additionalProperties").and_then(Value::as_bool) == Some(false) {
        for key in object.keys() {
            if properties.is_none_or(|properties| !properties.contains_key(key)) {
                return Err(ToolSchemaError::new(
                    child_path(path, key),
                    "is not allowed by additionalProperties",
                ));
            }
        }
    }

    if let Some(properties) = properties {
        for (key, property_schema) in properties {
            if let Some(property_value) = object.get(key) {
                validate_value(property_schema, property_value, &child_path(path, key))?;
            }
        }
    }

    Ok(())
}

fn validate_numeric_keywords(
    schema: &Value,
    value: &Value,
    path: &str,
) -> Result<(), ToolSchemaError> {
    let Some(number) = value.as_f64() else {
        return Ok(());
    };

    if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64)
        && number < minimum
    {
        return Err(ToolSchemaError::new(
            path,
            format!("must be greater than or equal to minimum {minimum}"),
        ));
    }

    if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64)
        && number > maximum
    {
        return Err(ToolSchemaError::new(
            path,
            format!("must be less than or equal to maximum {maximum}"),
        ));
    }

    Ok(())
}

fn validate_array_keywords(
    schema: &Value,
    value: &Value,
    path: &str,
) -> Result<(), ToolSchemaError> {
    let Some(array) = value.as_array() else {
        return Ok(());
    };

    let Some(item_schema) = schema.get("items") else {
        return Ok(());
    };

    for (index, item) in array.iter().enumerate() {
        validate_value(item_schema, item, &array_item_path(path, index))?;
    }

    Ok(())
}

fn child_path(parent: &str, key: &str) -> String {
    if parent == "$" {
        format!("$.{key}")
    } else {
        format!("{parent}.{key}")
    }
}

fn array_item_path(parent: &str, index: usize) -> String {
    format!("{parent}[{index}]")
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::validate_tool_arguments;

    fn edit_array_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "edits": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_string": { "type": "string" },
                            "new_string": { "type": "string" }
                        },
                        "required": ["old_string", "new_string"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["edits"],
            "additionalProperties": false
        })
    }

    #[test]
    fn validate_tool_arguments_recurses_into_array_items() {
        let schema = edit_array_schema();

        let error = validate_tool_arguments(
            &schema,
            &json!({
                "edits": [
                    {
                        "old_string": "old",
                        "new_string": "new",
                        "replace_all": true
                    }
                ]
            }),
        )
        .expect_err("unknown fields inside array items should be rejected");

        assert_eq!(
            error.to_string(),
            "$.edits[0].replace_all is not allowed by additionalProperties"
        );
    }

    #[test]
    fn validate_tool_arguments_reports_missing_required_array_item_field() {
        let schema = edit_array_schema();

        let error = validate_tool_arguments(
            &schema,
            &json!({
                "edits": [
                    {
                        "old_string": "old"
                    }
                ]
            }),
        )
        .expect_err("missing fields inside array items should be rejected");

        assert_eq!(error.to_string(), "$.edits[0].new_string is required");
    }

    #[test]
    fn validate_tool_arguments_reports_numbers_above_maximum() {
        let schema = json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "maximum": 10
                }
            },
            "additionalProperties": false
        });

        let error = validate_tool_arguments(&schema, &json!({ "limit": 11 }))
            .expect_err("maximum should reject values above the schema limit");

        assert_eq!(
            error.to_string(),
            "$.limit must be less than or equal to maximum 10"
        );
    }
}
