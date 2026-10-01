//! 解析器的 `info` 字典。
//!
//! ⚠️ 三种格式的形状**不一样**（与 Python 保持一致，不要"统一"）：
//! · RPE / PGR：`{"judge_lines": n, "bpm_count": n, "bpm_min": x, "bpm_max": y}`
//! · PEC       ：同上但 `judge_lines: null`，并多一个 `pec_offset_sec`
//! · PBC       ：`{"error": "binary"}`
//!
//! 「统一成一个形状」看似更整洁，但会让下游（`chart_api.analyze` 的
//! `info.get("judge_lines")`）的语义发生改变，而且黄金夹具就是按原形状生成的。

use serde_json::{json, Map, Value};

#[derive(Debug, Clone)]
pub struct Info {
    pub judge_lines: Option<usize>,
    pub bpm_count: usize,
    pub bpm_min: Option<f64>,
    pub bpm_max: Option<f64>,
    pub pec_offset_sec: Option<f64>,
    pub error: Option<&'static str>,
}

impl Info {
    pub fn to_json(&self) -> Value {
        if let Some(e) = self.error {
            let mut m = Map::new();
            m.insert("error".into(), Value::String(e.to_string()));
            return Value::Object(m);
        }
        let mut m = Map::new();
        m.insert(
            "judge_lines".into(),
            match self.judge_lines {
                Some(n) => json!(n),
                None => Value::Null,
            },
        );
        m.insert("bpm_count".into(), json!(self.bpm_count));
        m.insert(
            "bpm_min".into(),
            self.bpm_min.map(|v| crate::note::num(v)).unwrap_or(Value::Null),
        );
        m.insert(
            "bpm_max".into(),
            self.bpm_max.map(|v| crate::note::num(v)).unwrap_or(Value::Null),
        );
        if let Some(o) = self.pec_offset_sec {
            m.insert("pec_offset_sec".into(), crate::note::num(o));
        }
        Value::Object(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Info {
        Info {
            judge_lines: Some(3),
            bpm_count: 1,
            bpm_min: Some(120.0),
            bpm_max: Some(120.0),
            pec_offset_sec: None,
            error: None,
        }
    }

    #[test]
    fn shapes() {
        assert_eq!(sample().to_json()["judge_lines"], json!(3));
        let mut pec = sample();
        pec.judge_lines = None;
        pec.pec_offset_sec = Some(0.35);
        assert_eq!(pec.to_json()["judge_lines"], Value::Null);
        assert_eq!(pec.to_json()["pec_offset_sec"], json!(0.35));
        let pbc = Info {
            judge_lines: None, bpm_count: 0, bpm_min: None, bpm_max: None,
            pec_offset_sec: None, error: Some("binary"),
        };
        assert_eq!(pbc.to_json(), json!({"error": "binary"}));
    }
}
