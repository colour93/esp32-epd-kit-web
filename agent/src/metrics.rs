use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_EXPRESSION_BYTES: usize = 1024;
pub const MAX_TITLE_CHARS: usize = 32;
pub const MAX_LABEL_CHARS: usize = 24;
pub const MAX_DATA_CHARS: usize = 48;
pub const MAX_DESCRIPTION_CHARS: usize = 96;
pub const MAX_ITEMS: usize = 4;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricFormat {
    #[default]
    Text,
    Percent,
    Countdown,
    CompactNumber,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct MetricItemConfig {
    pub label: String,
    pub data_expression: String,
    pub description_expression: String,
    pub progress_expression: String,
    #[serde(default)]
    pub format: MetricFormat,
}

#[derive(Clone, Debug, Serialize)]
pub struct MetricPreviewItem {
    pub label: String,
    pub data: Value,
    pub description: Option<String>,
    pub progress: Option<f64>,
    pub format: MetricFormat,
}

#[derive(Clone, Debug, Serialize)]
pub struct MetricPreview {
    pub source_status: &'static str,
    pub title: String,
    pub items: Vec<MetricPreviewItem>,
    pub elapsed_ms: u128,
    pub output_bytes: usize,
}

pub fn validate_metric_config(
    title: &str,
    items: &[MetricItemConfig],
    require_complete: bool,
) -> Result<()> {
    if title.chars().count() > MAX_TITLE_CHARS {
        bail!("标题最多 {MAX_TITLE_CHARS} 个字符");
    }
    if items.len() > MAX_ITEMS {
        bail!("最多配置 {MAX_ITEMS} 个数据项");
    }
    for (index, item) in items.iter().enumerate() {
        if item.label.chars().count() > MAX_LABEL_CHARS {
            bail!(
                "数据项 {} 的 label 最多 {MAX_LABEL_CHARS} 个字符",
                index + 1
            );
        }
        for expression in [
            &item.data_expression,
            &item.description_expression,
            &item.progress_expression,
        ] {
            if expression.len() > MAX_EXPRESSION_BYTES {
                bail!("JMESPath 表达式不能超过 {MAX_EXPRESSION_BYTES} bytes");
            }
        }
    }
    if require_complete {
        if title.trim().is_empty() {
            bail!("标题不能为空");
        }
        if items.is_empty() {
            bail!("至少需要一个数据项");
        }
        for (index, item) in items.iter().enumerate() {
            if item.label.trim().is_empty() {
                bail!("数据项 {} 的 label 不能为空", index + 1);
            }
            compile_expression("data", &item.data_expression)?;
            if !item.description_expression.trim().is_empty() {
                compile_expression("description", &item.description_expression)?;
            }
            if !item.progress_expression.trim().is_empty() {
                compile_expression("progress", &item.progress_expression)?;
            }
        }
    }
    Ok(())
}

pub fn project_metrics(
    title: &str,
    items: &[MetricItemConfig],
    input: &Value,
    elapsed_ms: u128,
    output_bytes: usize,
) -> Result<MetricPreview> {
    let mut preview_items = Vec::with_capacity(items.len());
    for item in items {
        let mut data = evaluate_value(input, &item.data_expression, MAX_DATA_CHARS)?;
        if matches!(&item.format, MetricFormat::CompactNumber) {
            data = Value::String(compact_number(data)?);
        }
        let description = if item.description_expression.trim().is_empty() {
            None
        } else {
            let value = evaluate_value(input, &item.description_expression, MAX_DESCRIPTION_CHARS)?;
            display_text(value).filter(|value| !value.is_empty() && value != "--")
        };
        let progress = if item.progress_expression.trim().is_empty() {
            None
        } else {
            Some(evaluate_number(input, &item.progress_expression)?.clamp(0.0, 100.0))
        };
        preview_items.push(MetricPreviewItem {
            label: item.label.trim().to_owned(),
            data,
            description,
            progress,
            format: item.format.clone(),
        });
    }
    Ok(MetricPreview {
        source_status: "ok",
        title: title.trim().to_owned(),
        items: preview_items,
        elapsed_ms,
        output_bytes,
    })
}

fn compile_expression(label: &str, expression: &str) -> Result<()> {
    if expression.trim().is_empty() {
        bail!("{label} 表达式不能为空");
    }
    jmespath::compile(expression)
        .map(|_| ())
        .with_context(|| format!("{label} JMESPath 无效"))
}

fn evaluate_value(input: &Value, expression: &str, max_chars: usize) -> Result<Value> {
    let compiled =
        jmespath::compile(expression).with_context(|| format!("JMESPath 无效: {expression}"))?;
    let data = jmespath::Variable::from_json(&serde_json::to_string(input)?)
        .map_err(|error| anyhow!("无法构造 JMESPath 输入: {error}"))?;
    let projected = compiled
        .search(data)
        .with_context(|| format!("JMESPath 执行失败: {expression}"))?;
    let value: Value =
        serde_json::from_str(&projected.to_string()).context("无法编码 JMESPath 结果")?;
    Ok(match value {
        Value::Null => Value::String("--".into()),
        Value::String(value) => Value::String(truncate_text(value.trim(), max_chars)),
        Value::Bool(_) | Value::Number(_) => value,
        value => Value::String(truncate_text(&serde_json::to_string(&value)?, max_chars)),
    })
}

fn evaluate_number(input: &Value, expression: &str) -> Result<f64> {
    let value = evaluate_value(input, expression, MAX_DATA_CHARS)?;
    match value {
        Value::Number(value) => value
            .as_f64()
            .ok_or_else(|| anyhow!("progress 不是有限数字")),
        Value::String(value) => value
            .parse::<f64>()
            .with_context(|| format!("progress 不是数字: {value}")),
        _ => bail!("progress 必须投影为数字"),
    }
}

fn display_text(value: Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        Value::Null => None,
        value => serde_json::to_string(&value).ok(),
    }
}

fn compact_number(value: Value) -> Result<String> {
    let number = match value {
        Value::Number(value) => value
            .as_f64()
            .ok_or_else(|| anyhow!("紧凑数字不是有限数字"))?,
        Value::String(value) => value
            .parse::<f64>()
            .with_context(|| format!("紧凑数字不是数字: {value}"))?,
        _ => bail!("紧凑数字必须投影为数字"),
    };
    if !number.is_finite() {
        bail!("紧凑数字不是有限数字");
    }

    const UNITS: [&str; 5] = ["", "K", "M", "B", "T"];
    let mut scaled = number;
    let mut unit = 0;
    while scaled.abs() >= 1000.0 && unit < UNITS.len() - 1 {
        scaled /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        return Ok(number.to_string());
    }

    let precision = if scaled.abs() >= 100.0 {
        0
    } else if scaled.abs() >= 10.0 {
        1
    } else {
        2
    };
    let mut value = format!("{scaled:.precision$}");
    if value.contains('.') {
        value = value.trim_end_matches('0').trim_end_matches('.').to_owned();
    }
    Ok(format!("{value}{}", UNITS[unit]))
}

pub fn truncate_text(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{MetricFormat, MetricItemConfig, project_metrics, validate_metric_config};

    #[test]
    fn projection_normalizes_values_and_clamps_progress() {
        let items = vec![MetricItemConfig {
            label: "Balance".into(),
            data_expression: "account.balance".into(),
            description_expression: "account.currency".into(),
            progress_expression: "account.percent".into(),
            format: MetricFormat::Percent,
        }];
        validate_metric_config("Plan", &items, true).unwrap();
        let preview = project_metrics(
            "Plan",
            &items,
            &json!({"account": {"balance": "12.5", "currency": "CNY", "percent": 120}}),
            7,
            64,
        )
        .unwrap();
        assert_eq!(preview.items[0].data, json!("12.5"));
        assert_eq!(preview.items[0].description.as_deref(), Some("CNY"));
        assert_eq!(preview.items[0].progress, Some(100.0));
        assert_eq!(preview.elapsed_ms, 7);
        assert_eq!(preview.output_bytes, 64);
    }

    #[test]
    fn projection_rejects_non_numeric_progress() {
        let items = vec![MetricItemConfig {
            label: "Balance".into(),
            data_expression: "balance".into(),
            progress_expression: "state".into(),
            ..Default::default()
        }];
        let error = project_metrics("Plan", &items, &json!({"balance": 1, "state": true}), 0, 0)
            .unwrap_err();
        assert!(error.to_string().contains("progress"));
    }
}
