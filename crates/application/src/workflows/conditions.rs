//! No-code conditions and `{{ }}` templates evaluated over JSON values.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConditionMatch {
    /// Every rule must hold.
    #[default]
    All,
    /// At least one rule must hold.
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionOperator {
    Equals,
    NotEquals,
    Contains,
    NotContains,
    StartsWith,
    EndsWith,
    GreaterThan,
    LessThan,
    IsEmpty,
    IsNotEmpty,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConditionRule {
    /// Dotted path into the data, for example `year` or `screening.final`.
    pub field: String,
    pub operator: ConditionOperator,
    #[serde(default)]
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Condition {
    #[serde(rename = "match", default)]
    pub match_mode: ConditionMatch,
    #[serde(default)]
    pub rules: Vec<ConditionRule>,
}

/// Look up a dotted path (`a.b.0.c`) in a JSON value.
pub fn json_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.').filter(|segment| !segment.is_empty()) {
        current = match current {
            Value::Object(map) => map.get(segment)?,
            Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

fn as_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn as_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn is_empty(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::String(text)) => text.trim().is_empty(),
        Some(Value::Array(items)) => items.is_empty(),
        Some(Value::Object(map)) => map.is_empty(),
        Some(_) => false,
    }
}

fn values_equal(left: &Value, right: &Value) -> bool {
    if let (Some(a), Some(b)) = (as_number(left), as_number(right))
        && !matches!((left, right), (Value::String(_), Value::String(_)))
    {
        return (a - b).abs() < f64::EPSILON;
    }
    if let (Value::Bool(a), Value::Bool(b)) = (left, right) {
        return a == b;
    }
    as_text(left).trim().to_lowercase() == as_text(right).trim().to_lowercase()
}

fn contains(haystack: Option<&Value>, needle: &Value) -> bool {
    match haystack {
        Some(Value::Array(items)) => items.iter().any(|item| values_equal(item, needle)),
        Some(value) => as_text(value)
            .to_lowercase()
            .contains(&as_text(needle).to_lowercase()),
        None => false,
    }
}

fn rule_holds(rule: &ConditionRule, data: &Value) -> bool {
    let actual = json_path(data, &rule.field);
    match rule.operator {
        ConditionOperator::IsEmpty => is_empty(actual),
        ConditionOperator::IsNotEmpty => !is_empty(actual),
        ConditionOperator::Equals => actual.is_some_and(|value| values_equal(value, &rule.value)),
        ConditionOperator::NotEquals => {
            !actual.is_some_and(|value| values_equal(value, &rule.value))
        }
        ConditionOperator::Contains => contains(actual, &rule.value),
        ConditionOperator::NotContains => !contains(actual, &rule.value),
        ConditionOperator::StartsWith => actual.is_some_and(|value| {
            as_text(value)
                .to_lowercase()
                .starts_with(&as_text(&rule.value).to_lowercase())
        }),
        ConditionOperator::EndsWith => actual.is_some_and(|value| {
            as_text(value)
                .to_lowercase()
                .ends_with(&as_text(&rule.value).to_lowercase())
        }),
        ConditionOperator::GreaterThan => actual
            .and_then(as_number)
            .zip(as_number(&rule.value))
            .is_some_and(|(a, b)| a > b),
        ConditionOperator::LessThan => actual
            .and_then(as_number)
            .zip(as_number(&rule.value))
            .is_some_and(|(a, b)| a < b),
    }
}

/// Evaluate a condition. A condition without rules always holds.
pub fn evaluate_condition(condition: &Condition, data: &Value) -> bool {
    if condition.rules.is_empty() {
        return true;
    }
    match condition.match_mode {
        ConditionMatch::All => condition.rules.iter().all(|rule| rule_holds(rule, data)),
        ConditionMatch::Any => condition.rules.iter().any(|rule| rule_holds(rule, data)),
    }
}

/// The `{{path}}` tokens of a template, in order. `unclosed` is set when a `{{`
/// has no `}}` after it; that text is left as it is when the template renders.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TemplateTokens {
    pub paths: Vec<String>,
    pub unclosed: bool,
}

pub fn template_tokens(template: &str) -> TemplateTokens {
    let mut tokens = TemplateTokens::default();
    let mut rest = template;
    while let Some((_, after)) = rest.split_once("{{") {
        match after.split_once("}}") {
            Some((path, tail)) => {
                tokens.paths.push(path.trim().to_owned());
                rest = tail;
            }
            None => {
                tokens.unclosed = true;
                break;
            }
        }
    }
    tokens
}

/// One detail as text. A list has one detail of its own, `count`: its size.
fn detail_text(data: &Value, path: &str) -> Option<String> {
    if let Value::Array(items) = data
        && path == "count"
    {
        return Some(items.len().to_string());
    }
    json_path(data, path).map(as_text)
}

/// Replace `{{path}}` tokens with values from `data`. Unknown paths render as
/// an empty string so a missing detail never aborts a run; validation reports
/// them before a flow is published.
pub fn render_template(template: &str, data: &Value) -> String {
    let mut output = String::with_capacity(template.len());
    let mut rest = template;
    while let Some((before, after)) = rest.split_once("{{") {
        output.push_str(before);
        match after.split_once("}}") {
            Some((path, tail)) => {
                if let Some(text) = detail_text(data, path.trim()) {
                    output.push_str(&text);
                }
                rest = tail;
            }
            None => {
                output.push_str("{{");
                output.push_str(after);
                return output;
            }
        }
    }
    output.push_str(rest);
    output
}

/// HTTP templates may substitute path/query values, never scheme or authority.
pub fn fixed_url_origin(template: &str) -> Result<String, &'static str> {
    let (_, authority_and_path) = template.split_once("://").ok_or("URL must have a fixed origin")?;
    let authority = authority_and_path.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains("{{") {
        return Err("URL must have a fixed origin");
    }
    let literal = template.split("{{").next().unwrap_or(template);
    let url = url::Url::parse(literal).map_err(|_| "URL must have a fixed origin")?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none()
        || !url.username().is_empty() || url.password().is_some()
    {
        return Err("URL must have an HTTP origin without credentials");
    }
    Ok(url.origin().ascii_serialization())
}

pub fn render_url_template(template: &str, data: &Value) -> Result<String, &'static str> {
    let origin = fixed_url_origin(template)?;
    let mut output = String::with_capacity(template.len());
    let mut rest = template;
    while let Some((before, after)) = rest.split_once("{{") {
        output.push_str(before);
        let (path, tail) = after.split_once("}}").ok_or("Unclosed URL template token")?;
        if let Some(text) = detail_text(data, path.trim()) {
            use std::fmt::Write;
            for byte in text.bytes() {
                if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                    output.push(char::from(byte));
                } else {
                    write!(output, "%{byte:02X}").map_err(|_| "Could not encode URL")?;
                }
            }
        }
        rest = tail;
    }
    output.push_str(rest);
    if fixed_url_origin(&output)? != origin {
        return Err("URL substitution changed origin");
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn rule(field: &str, operator: ConditionOperator, value: Value) -> ConditionRule {
        ConditionRule {
            field: field.to_owned(),
            operator,
            value,
        }
    }

    #[test]
    fn text_and_number_operators() {
        let item =
            json!({"title": "Statins for Heart Failure", "year": 2021, "authors": ["Li", "Sousa"]});
        assert!(rule_holds(
            &rule("title", ConditionOperator::Contains, json!("heart")),
            &item
        ));
        assert!(rule_holds(
            &rule("title", ConditionOperator::StartsWith, json!("statins")),
            &item
        ));
        assert!(rule_holds(
            &rule("year", ConditionOperator::GreaterThan, json!("2020")),
            &item
        ));
        assert!(!rule_holds(
            &rule("year", ConditionOperator::LessThan, json!(2020)),
            &item
        ));
        assert!(rule_holds(
            &rule("year", ConditionOperator::Equals, json!("2021")),
            &item
        ));
        assert!(rule_holds(
            &rule("authors", ConditionOperator::Contains, json!("sousa")),
            &item
        ));
        assert!(rule_holds(
            &rule("abstract", ConditionOperator::IsEmpty, Value::Null),
            &item
        ));
        assert!(rule_holds(
            &rule("title", ConditionOperator::IsNotEmpty, Value::Null),
            &item
        ));
        assert!(rule_holds(
            &rule("title", ConditionOperator::NotContains, json!("cancer")),
            &item
        ));
    }

    #[test]
    fn all_any_and_empty_conditions() {
        let item = json!({"year": 2019, "title": "x"});
        let all = Condition {
            match_mode: ConditionMatch::All,
            rules: vec![
                rule("year", ConditionOperator::GreaterThan, json!(2018)),
                rule("title", ConditionOperator::Equals, json!("y")),
            ],
        };
        assert!(!evaluate_condition(&all, &item));
        let any = Condition {
            match_mode: ConditionMatch::Any,
            ..all.clone()
        };
        assert!(evaluate_condition(&any, &item));
        assert!(evaluate_condition(&Condition::default(), &item));
    }

    #[test]
    fn missing_fields_never_satisfy_comparisons() {
        let item = json!({});
        assert!(!rule_holds(
            &rule("year", ConditionOperator::GreaterThan, json!(1)),
            &item
        ));
        assert!(!rule_holds(
            &rule("year", ConditionOperator::Equals, json!(1)),
            &item
        ));
        assert!(rule_holds(
            &rule("year", ConditionOperator::NotEquals, json!(1)),
            &item
        ));
    }

    #[test]
    fn templates_render_paths_and_ignore_unknowns() {
        let data = json!({"title": "A", "screening": {"final": "include"}, "list": [{"n": 3}]});
        assert_eq!(
            render_template("{{title}} is {{ screening.final }}{{nope}}!", &data),
            "A is include!"
        );
        assert_eq!(
            render_template("n={{list.0.n}} {{open", &data),
            "n=3 {{open"
        );
    }

    #[test]
    fn a_list_renders_its_count_and_tokens_are_listed() {
        let records = json!([{"title": "A"}, {"title": "B"}]);
        assert_eq!(
            render_template("Found {{count}} new", &records),
            "Found 2 new"
        );
        assert_eq!(render_template("{{title}}", &records), "");
        let tokens = template_tokens(" {{ title }} and {{count}} {{}} ");
        assert_eq!(tokens.paths, vec!["title", "count", ""]);
        assert!(!tokens.unclosed);
        let open = template_tokens("x {{title");
        assert!(open.unclosed);
        assert!(open.paths.is_empty());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod url_tests {
    use super::{fixed_url_origin, render_url_template};
    use serde_json::json;

    #[test]
    fn http_templates_require_a_fixed_origin() {
        for url in ["{{url}}", "{{scheme}}://example.com/x", "https://{{host}}/x",
            "https://example.com{{suffix}}/x", "https://user:password@example.com/x"] {
            assert!(fixed_url_origin(url).is_err(), "{url}");
        }
        assert_eq!(fixed_url_origin("https://EXAMPLE.com:443/path/{{id}}").unwrap(), "https://example.com");
    }

    #[test]
    fn url_values_are_encoded_as_components_not_url_syntax() {
        let url = render_url_template("https://example.com/p/{{id}}?q={{query}}",
            &json!({"id":"//evil.example/a?b#c","query":"a&admin=true ü"})).unwrap();
        assert_eq!(url, "https://example.com/p/%2F%2Fevil.example%2Fa%3Fb%23c?q=a%26admin%3Dtrue%20%C3%BC");
        assert_eq!(fixed_url_origin(&url).unwrap(), "https://example.com");
    }
}
