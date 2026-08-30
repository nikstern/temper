pub(super) fn to_pascal_case(name: &str) -> String {
    let mut result = String::with_capacity(name.len());
    let mut uppercase = true;
    for character in name.chars() {
        if character == '_' {
            uppercase = true;
        } else if uppercase {
            result.extend(character.to_uppercase());
            uppercase = false;
        } else {
            result.push(character);
        }
    }
    result
}

pub(super) fn deterministic_param_value(param_type: &str) -> serde_json::Value {
    match param_type.to_ascii_lowercase().as_str() {
        "bool" | "boolean" | "edm.boolean" => serde_json::Value::Bool(true),
        "int" | "integer" | "i32" | "i64" | "edm.byte" | "edm.sbyte" | "edm.int16"
        | "edm.int32" | "edm.int64" | "float" | "double" | "decimal" | "edm.decimal"
        | "edm.double" | "edm.single" => serde_json::json!(1),
        name if name.starts_with("list") || name.starts_with("collection") => {
            serde_json::json!(["value"])
        }
        _ => serde_json::Value::String("value".to_string()),
    }
}
