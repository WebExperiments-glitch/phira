//! 音符与数值序列化。
//!
//! ⚠️ 这是**移植的显性契约**（对应黄金夹具里的具名字段，不要用数组位置）。

use serde_json::{json, Map, Value};

/// 音符。
///
/// | 字段 | 含义 | 单位 |
/// |---|---|---|
/// | `t` | 音符时刻 | **秒** |
/// | `kind` | tap/drag/hold/flick 或 `?N` | — |
/// | `x_rel` | 相对判定线的 x | 屏幕半宽 = 1.0 |
/// | `hold_sec` | hold 时长，非 hold 为 0 | 秒 |
/// | `beat` | 原始拍值 | 拍 |
/// | `x_abs_lo/hi` | 判定线位移区间端点 + x_rel | 屏幕半宽 |
/// | `is_fake` | 1 = 假音符（不计入统计） | — |
///
/// ⚠️ `x_abs_lo/hi` **不是音符的位置**，只是判定线位移区间的端点。
/// 真正的屏幕位置要沿 father 链逐音符算（`lines` 模块，待移植）。
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub t: f64,
    pub kind: String,
    pub x_rel: f64,
    pub hold_sec: f64,
    pub beat: f64,
    pub x_abs_lo: Option<f64>,
    pub x_abs_hi: Option<f64>,
    pub is_fake: i64,
}

impl Note {
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("t".into(), num(self.t));
        m.insert("type".into(), Value::String(self.kind.clone()));
        m.insert("x_rel".into(), num(self.x_rel));
        m.insert("hold_sec".into(), num(self.hold_sec));
        m.insert("beat".into(), num(self.beat));
        m.insert("x_abs_lo".into(), opt_num(self.x_abs_lo));
        m.insert("x_abs_hi".into(), opt_num(self.x_abs_hi));
        m.insert("is_fake".into(), json!(self.is_fake));
        Value::Object(m)
    }
}

/// 把 f64 序列化成 JSON number。
///
/// ⚠️ serde_json 对 `NaN`/`Infinity` 输出 `null`（Python 的 `json.dumps` 默认输出
/// `NaN`/`Infinity` 这种非法 JSON）。正常谱面不会出现；**如果**出现了，
/// 夹具比对会以 `null` 呈现 —— 这正好让差异暴露出来，而不是被悄悄吞掉。
pub fn num(f: f64) -> Value {
    serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null)
}

pub fn opt_num(f: Option<f64>) -> Value {
    match f {
        Some(v) => num(v),
        None => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_json_fields() {
        let n = Note {
            t: 1.0, kind: "tap".into(), x_rel: 0.5, hold_sec: 0.0, beat: 2.0,
            x_abs_lo: Some(0.5), x_abs_hi: Some(0.5), is_fake: 0,
        };
        let j = n.to_json();
        assert_eq!(j["type"], json!("tap"));
        assert_eq!(j["is_fake"], json!(0));
        assert_eq!(j["x_abs_lo"], json!(0.5));
    }

    #[test]
    fn nan_becomes_null() {
        assert_eq!(num(f64::NAN), Value::Null);
    }
}
