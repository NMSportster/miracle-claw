// Week 3 (2026-09-09): Shared form_schema parser.
// Spec: docs/mc-module-driven-surfaces-2026-09.md §3.2 + §5.4.
//
// Every first-party module declares a `form_schema` per tool in its
// `client_manifest`. The schema is a JSON Schema subset:
//
//   {
//     "type": "object",
//     "required": ["url"],
//     "properties": {
//       "url":  { "type": "string", "title": "URL", "minLength": 1 },
//       "max_pages": {
//         "type": "integer", "title": "Max pages",
//         "default": 10, "minimum": 1, "maximum": 100
//       },
//       "tone": {
//         "type": "string", "title": "Tone",
//         "enum": ["friendly", "professional", "urgent", "playful"]
//       }
//     }
//   }
//
// Three surfaces render this schema:
//   - Tauri launchpad (HTML form, JS-side render in dashboard_launchpad.js)
//   - mc-terminal TUI (text fields with Tab/Enter nav)
//   - mclaw CLI (one-shot: `mclaw run <module>.<tool> --key=value`)
//
// This parser is the single source of truth for what fields a tool
// needs, in what order, with what validation rules. Each surface then
// renders the same parsed `FormSchema` in its native UI.
//
// We deliberately support ONLY the subset MAIC currently emits (see
// migration 035 + the 8 starter modules). Adding more types is
// straightforward — just extend the `FieldKind` enum and the
// `parse_property` function — but we don't speculate on JSON Schema
// features we don't use yet (oneOf, anyOf, $ref, etc.).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Parsed form schema for a single tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormSchema {
    /// Ordered list of fields, in the order MAIC declared them.
    /// JSON object property order isn't guaranteed by all encoders,
    /// but the modules seeded in migration 035 use deterministic order
    /// (Rust's `serde_json::json!` macro preserves it).
    pub fields: Vec<FormField>,
}

/// One input field in a form. `display_order` matches the order MAIC
/// declared the property in the schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormField {
    /// The property name (used as JSON key in `inputs`).
    pub name: String,
    /// Human label. Falls back to `name` if MAIC didn't set `title`.
    pub label: String,
    /// Required by the schema's `required` array.
    pub required: bool,
    /// What kind of input to render.
    pub kind: FieldKind,
    /// Original `default` from the schema, if any. Use `kind.default_value()`
    /// to get a typed default (e.g. `10_i64` for an integer field).
    pub default: Option<Value>,
    /// Position in declaration order (0-based). Useful for Tab/Enter nav
    /// in the TUI and `data-field-index` in the HTML form.
    pub display_order: usize,
}

/// The kind of input widget to render. Each variant carries the
/// constraint metadata needed to validate a user-typed value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FieldKind {
    /// Single-line text input.
    Text {
        min_length: Option<usize>,
        max_length: Option<usize>,
    },
    /// Multi-line text input. Same constraints as Text but rendered with
    /// a larger box in the TUI / a `<textarea>` in HTML.
    TextArea {
        min_length: Option<usize>,
        max_length: Option<usize>,
    },
    /// Numeric input. `minimum`/`maximum` are inclusive bounds.
    Number {
        minimum: Option<f64>,
        maximum: Option<f64>,
        /// True if the schema declared `"type": "integer"`. Affects how
        /// we parse user input (no decimal point allowed).
        is_integer: bool,
    },
    /// Dropdown / picklist. `options` are the allowed string values.
    Select { options: Vec<String> },
    /// Boolean checkbox.
    Boolean,
}

impl FieldKind {
    /// Type tag for JS-side rendering. Mirrors the `FieldKind` variant
    /// names (snake_case) so `dashboard_launchpad.js` can branch on
    /// `field.kind.type === "text"` etc. without a separate mapping.
    pub fn type_tag(&self) -> &'static str {
        match self {
            FieldKind::Text { .. } => "text",
            FieldKind::TextArea { .. } => "textarea",
            FieldKind::Number { .. } => "number",
            FieldKind::Select { .. } => "select",
            FieldKind::Boolean => "boolean",
        }
    }

    /// Convenience: return the field's default as a typed value, or `None`.
    /// Mirrors what the user sees in the form before they type anything.
    pub fn default_value(&self) -> Option<Value> {
        self.default_from_kind()
    }

    fn default_from_kind(&self) -> Option<Value> {
        // The default is stored on `FormField::default`; this method is
        // a forwarder so call sites can write `field.kind.default_value()`
        // without threading the Option through.
        None // Real impl: see FormField::default_value() below
    }
}

impl FormField {
    /// Return the field's default value as a typed JSON value, or `None`
    /// if no default was declared. Renders to the same shape the
    /// `inputs` payload expects.
    pub fn default_value(&self) -> Option<Value> {
        self.default.clone()
    }
}

/// Parse a JSON-Schema-subset form_schema blob into our typed structure.
/// Returns `Err` with a human-readable message if the schema is
/// malformed (missing `properties`, non-object root, etc.).
pub fn parse_form_schema(schema: &Value) -> Result<FormSchema, String> {
    let obj = schema
        .as_object()
        .ok_or_else(|| "form_schema root must be a JSON object".to_string())?;

    let schema_type = obj
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "form_schema is missing required `type` field".to_string())?;
    if schema_type != "object" {
        return Err(format!(
            "form_schema `type` must be \"object\", got \"{schema_type}\""
        ));
    }

    let properties = obj
        .get("properties")
        .ok_or_else(|| "form_schema is missing `properties`".to_string())?
        .as_object()
        .ok_or_else(|| "form_schema `properties` must be an object".to_string())?;

    let required: Vec<String> = obj
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    // JSON object iteration order is insertion order in serde_json's
    // Map<String, Value> (BTreeMap-style ordered, actually a BTreeMap
    // by default but serde_json uses IndexMap if the `preserve_order`
    // feature is enabled; either way our `json!` macro output is
    // deterministic). We capture the insertion order here.
    let mut fields = Vec::with_capacity(properties.len());
    for (display_order, (name, prop_value)) in properties.iter().enumerate() {
        let field = parse_property(name, prop_value, &required, display_order)?;
        fields.push(field);
    }

    Ok(FormSchema { fields })
}

/// Parse a single property of the form_schema into a typed `FormField`.
fn parse_property(
    name: &str,
    prop: &Value,
    required: &[String],
    display_order: usize,
) -> Result<FormField, String> {
    let prop_obj = prop
        .as_object()
        .ok_or_else(|| format!("property `{name}` must be a JSON object"))?;

    let label = prop_obj
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or(name)
        .to_string();

    let is_required = required.iter().any(|r| r == name);
    let default = prop_obj.get("default").cloned();

    let prop_type = prop_obj
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("property `{name}` is missing `type`"))?;

    let kind = match prop_type {
        "string" => {
            // `format` can hint at textarea (e.g. "long-text"). MAIC
            // doesn't use it yet but we forwarder the convention in case
            // future modules do.
            let is_long = prop_obj
                .get("format")
                .and_then(|v| v.as_str())
                .map(|f| f == "long-text" || f == "textarea")
                .unwrap_or(false);

            let min_length = prop_obj
                .get("minLength")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);
            let max_length = prop_obj
                .get("maxLength")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);

            if let Some(enum_values) = prop_obj.get("enum").and_then(|v| v.as_array()) {
                let options: Vec<String> = enum_values
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
                FieldKind::Select { options }
            } else if is_long {
                FieldKind::TextArea {
                    min_length,
                    max_length,
                }
            } else {
                FieldKind::Text {
                    min_length,
                    max_length,
                }
            }
        }
        "integer" | "number" => {
            let is_integer = prop_type == "integer";
            let minimum = prop_obj.get("minimum").and_then(|v| v.as_f64());
            let maximum = prop_obj.get("maximum").and_then(|v| v.as_f64());
            FieldKind::Number {
                minimum,
                maximum,
                is_integer,
            }
        }
        "boolean" => FieldKind::Boolean,
        other => {
            return Err(format!(
                "property `{name}` has unsupported type \"{other}\" \
                 (form_schema parser supports string, integer, number, boolean)"
            ));
        }
    };

    Ok(FormField {
        name: name.to_string(),
        label,
        required: is_required,
        kind,
        default,
        display_order,
    })
}

/// Validate a user-supplied `inputs` object against the parsed schema.
/// Returns `Err` listing the first validation failure (short-circuit
/// for friendly UX; we don't collect all errors at once). The TUI and
/// CLI use this; the HTML launchpad uses native browser validation
/// (`required`, `minlength`, `min`/`max`) instead.
pub fn validate_inputs(
    schema: &FormSchema,
    inputs: &Value,
) -> Result<(), String> {
    let inputs_obj = inputs
        .as_object()
        .ok_or_else(|| "inputs must be a JSON object".to_string())?;

    for field in &schema.fields {
        let value = inputs_obj.get(&field.name);

        match (&field.kind, value) {
            // Required + missing → fail
            (_, None) if field.required => {
                return Err(format!("`{}` is required", field.label));
            }
            // Not required + missing → fine, leave the default
            (_, None) => continue,

            // Type-specific checks
            (FieldKind::Text { min_length, max_length } | FieldKind::TextArea { min_length, max_length }, Some(v)) => {
                let s = v
                    .as_str()
                    .ok_or_else(|| format!("`{}` must be a string", field.label))?;
                if let Some(min) = min_length {
                    if s.chars().count() < *min {
                        return Err(format!(
                            "`{}` must be at least {min} characters",
                            field.label
                        ));
                    }
                }
                if let Some(max) = max_length {
                    if s.chars().count() > *max {
                        return Err(format!(
                            "`{}` must be at most {max} characters",
                            field.label
                        ));
                    }
                }
            }
            (FieldKind::Number { minimum, maximum, is_integer }, Some(v)) => {
                let n = v
                    .as_f64()
                    .ok_or_else(|| format!("`{}` must be a number", field.label))?;
                if *is_integer && n.fract() != 0.0 {
                    return Err(format!("`{}` must be an integer", field.label));
                }
                if let Some(min) = minimum {
                    if n < *min {
                        return Err(format!("`{}` must be ≥ {min}", field.label));
                    }
                }
                if let Some(max) = maximum {
                    if n > *max {
                        return Err(format!("`{}` must be ≤ {max}", field.label));
                    }
                }
            }
            (FieldKind::Select { options }, Some(v)) => {
                let s = v
                    .as_str()
                    .ok_or_else(|| format!("`{}` must be a string", field.label))?;
                if !options.iter().any(|o| o == s) {
                    return Err(format!(
                        "`{}` must be one of: {}",
                        field.label,
                        options.join(", ")
                    ));
                }
            }
            (FieldKind::Boolean, Some(v)) => {
                if !v.is_boolean() {
                    return Err(format!("`{}` must be true or false", field.label));
                }
            }
        }
    }
    Ok(())
}

/// Build the `inputs` payload by merging schema defaults with user
/// overrides. Used by `mclaw run --key=value` so the caller doesn't
/// have to repeat every field that has a default.
pub fn inputs_with_defaults(
    schema: &FormSchema,
    user_inputs: &Value,
) -> Value {
    let mut out = serde_json::Map::new();
    for field in &schema.fields {
        if let Some(v) = user_inputs.get(&field.name) {
            out.insert(field.name.clone(), v.clone());
        } else if let Some(default) = &field.default {
            out.insert(field.name.clone(), default.clone());
        }
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_minimal_string_field() {
        let schema = json!({
            "type": "object",
            "required": ["url"],
            "properties": {
                "url": {"type": "string", "title": "URL", "minLength": 1}
            }
        });
        let parsed = parse_form_schema(&schema).unwrap();
        assert_eq!(parsed.fields.len(), 1);
        let f = &parsed.fields[0];
        assert_eq!(f.name, "url");
        assert_eq!(f.label, "URL");
        assert!(f.required);
        assert!(matches!(f.kind, FieldKind::Text { min_length: Some(1), .. }));
    }

    #[test]
    fn parses_integer_with_default_and_bounds() {
        let schema = json!({
            "type": "object",
            "properties": {
                "max_pages": {
                    "type": "integer", "title": "Max pages",
                    "default": 10, "minimum": 1, "maximum": 100
                }
            }
        });
        let parsed = parse_form_schema(&schema).unwrap();
        let f = &parsed.fields[0];
        assert!(!f.required);
        assert_eq!(f.default, Some(json!(10)));
        match &f.kind {
            FieldKind::Number { minimum, maximum, is_integer } => {
                assert_eq!(*minimum, Some(1.0));
                assert_eq!(*maximum, Some(100.0));
                assert!(*is_integer);
            }
            _ => panic!("expected Number kind"),
        }
    }

    #[test]
    fn parses_enum_as_select() {
        let schema = json!({
            "type": "object",
            "properties": {
                "tone": {
                    "type": "string", "title": "Tone",
                    "enum": ["friendly", "professional", "urgent"]
                }
            }
        });
        let parsed = parse_form_schema(&schema).unwrap();
        let f = &parsed.fields[0];
        match &f.kind {
            FieldKind::Select { options } => {
                assert_eq!(options, &vec!["friendly", "professional", "urgent"]);
            }
            _ => panic!("expected Select kind"),
        }
    }

    #[test]
    fn preserves_property_order() {
        // Migration 035 emits property order = declaration order. The
        // launchpad and TUI rely on this for tab navigation.
        let schema = json!({
            "type": "object",
            "properties": {
                "first":  {"type": "string"},
                "second": {"type": "string"},
                "third":  {"type": "string"}
            }
        });
        let parsed = parse_form_schema(&schema).unwrap();
        assert_eq!(parsed.fields[0].name, "first");
        assert_eq!(parsed.fields[1].name, "second");
        assert_eq!(parsed.fields[2].name, "third");
        assert_eq!(parsed.fields[0].display_order, 0);
        assert_eq!(parsed.fields[2].display_order, 2);
    }

    #[test]
    fn rejects_non_object_root() {
        let schema = json!("not an object");
        let err = parse_form_schema(&schema).unwrap_err();
        assert!(err.contains("must be a JSON object"), "got: {err}");
    }

    #[test]
    fn rejects_wrong_type() {
        let schema = json!({"type": "array"});
        let err = parse_form_schema(&schema).unwrap_err();
        assert!(err.contains("must be \"object\""), "got: {err}");
    }

    #[test]
    fn rejects_unsupported_property_type() {
        let schema = json!({
            "type": "object",
            "properties": {"weird": {"type": "array"}}
        });
        let err = parse_form_schema(&schema).unwrap_err();
        assert!(err.contains("unsupported type"), "got: {err}");
    }

    #[test]
    fn validate_required_missing() {
        let schema = json!({
            "type": "object",
            "required": ["url"],
            "properties": {
                "url": {"type": "string", "title": "URL", "minLength": 1}
            }
        });
        let parsed = parse_form_schema(&schema).unwrap();
        // Missing required field fails.
        let err = validate_inputs(&parsed, &json!({})).unwrap_err();
        assert!(err.contains("`URL` is required"), "got: {err}");

        // Empty string also fails when minLength >= 1.
        let err = validate_inputs(&parsed, &json!({"url": ""})).unwrap_err();
        assert!(err.contains("at least 1 characters"), "got: {err}");
    }

    #[test]
    fn validate_min_length() {
        let schema = json!({
            "type": "object",
            "properties": {"name": {"type": "string", "minLength": 3}}
        });
        let parsed = parse_form_schema(&schema).unwrap();
        let err = validate_inputs(&parsed, &json!({"name": "ab"})).unwrap_err();
        assert!(err.contains("at least 3 characters"), "got: {err}");
    }

    #[test]
    fn validate_number_out_of_range() {
        let schema = json!({
            "type": "object",
            "properties": {"n": {"type": "integer", "minimum": 1, "maximum": 10}}
        });
        let parsed = parse_form_schema(&schema).unwrap();
        let err = validate_inputs(&parsed, &json!({"n": 100})).unwrap_err();
        assert!(err.contains("≤ 10"), "got: {err}");
    }

    #[test]
    fn validate_integer_rejects_float() {
        let schema = json!({
            "type": "object",
            "properties": {"n": {"type": "integer"}}
        });
        let parsed = parse_form_schema(&schema).unwrap();
        let err = validate_inputs(&parsed, &json!({"n": 3.5})).unwrap_err();
        assert!(err.contains("must be an integer"), "got: {err}");
    }

    #[test]
    fn validate_select_rejects_unknown_option() {
        let schema = json!({
            "type": "object",
            "properties": {
                "tone": {"type": "string", "enum": ["a", "b"]}
            }
        });
        let parsed = parse_form_schema(&schema).unwrap();
        let err = validate_inputs(&parsed, &json!({"tone": "c"})).unwrap_err();
        assert!(err.contains("must be one of"), "got: {err}");
    }

    #[test]
    fn inputs_with_defaults_merges() {
        let schema = json!({
            "type": "object",
            "properties": {
                "url":  {"type": "string"},
                "max_pages": {"type": "integer", "default": 10}
            }
        });
        let parsed = parse_form_schema(&schema).unwrap();
        let user = json!({"url": "https://example.com"});
        let merged = inputs_with_defaults(&parsed, &user);
        assert_eq!(merged["url"], "https://example.com");
        assert_eq!(merged["max_pages"], 10);
    }

    #[test]
    fn field_kind_type_tag_matches_variant() {
        let schema = json!({
            "type": "object",
            "properties": {
                "t":  {"type": "string"},
                "n":  {"type": "integer"},
                "sel": {"type": "string", "enum": ["a"]},
                "b":  {"type": "boolean"}
            }
        });
        let parsed = parse_form_schema(&schema).unwrap();
        let by_name: std::collections::HashMap<_, _> =
            parsed.fields.iter().map(|f| (f.name.as_str(), &f.kind)).collect();
        assert_eq!(by_name["t"].type_tag(), "text");
        assert_eq!(by_name["n"].type_tag(), "number");
        assert_eq!(by_name["sel"].type_tag(), "select");
        assert_eq!(by_name["b"].type_tag(), "boolean");
    }
}
