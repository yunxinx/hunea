use serde_json::Value;
use std::fmt;

/// `RuntimeToolSchema` 包装工具参数 JSON Schema。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeToolSchema {
    pub value: Value,
}

impl RuntimeToolSchema {
    /// `new` 创建工具参数 schema。
    pub fn new(value: Value) -> Self {
        Self { value }
    }
}

/// `RuntimeToolSchemaError` 描述工具参数不符合 JSON Schema 的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeToolSchemaError {
    path: String,
    message: String,
}

impl RuntimeToolSchemaError {
    fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for RuntimeToolSchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.path, self.message)
    }
}

impl std::error::Error for RuntimeToolSchemaError {}

pub(crate) fn validate_tool_arguments(
    schema: &Value,
    arguments: &Value,
) -> Result<(), RuntimeToolSchemaError> {
    validate_value(schema, arguments, "$")
}

fn validate_value(schema: &Value, value: &Value, path: &str) -> Result<(), RuntimeToolSchemaError> {
    if let Some(types) = schema_types(schema)
        && !types
            .iter()
            .any(|expected| value_matches_type(value, expected))
    {
        return Err(RuntimeToolSchemaError::new(
            path,
            format!("must match type {}", types.join(" or ")),
        ));
    }

    validate_object_keywords(schema, value, path)?;
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
) -> Result<(), RuntimeToolSchemaError> {
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
                return Err(RuntimeToolSchemaError::new(
                    child_path(path, required_key),
                    "is required",
                ));
            }
        }
    }

    if schema.get("additionalProperties").and_then(Value::as_bool) == Some(false) {
        for key in object.keys() {
            if properties.is_none_or(|properties| !properties.contains_key(key)) {
                return Err(RuntimeToolSchemaError::new(
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
) -> Result<(), RuntimeToolSchemaError> {
    let Some(number) = value.as_f64() else {
        return Ok(());
    };

    if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64)
        && number < minimum
    {
        return Err(RuntimeToolSchemaError::new(
            path,
            format!("must be greater than or equal to minimum {minimum}"),
        ));
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
