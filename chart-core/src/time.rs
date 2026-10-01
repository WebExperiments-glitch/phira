//! 时间换算。**单位：拍 → 秒。**
//!
//! ⚠️ 移植最容易错的一块。RPE / PEC 的原始事件时间是**拍**，
//! 早期 Python 版直接把拍当秒采样，导致速度单位随 BPM 变化、跨谱不可比。

use serde_json::Value;

use crate::pyconv::{py_display, py_float};

/// RPE 的时间三元组 `[a, b, c]` → `a + b/c` **拍**（`core.rs:98-100`）。
///
/// · 也接受单个数字（`triple(1.5) == 1.5`）
/// · `c` 为 0 / null / 缺失时按 1.0 处理（Python：`d = v[2] or 1.0`）
/// · None 抛错（Python：`raise ValueError("None triple")`）
pub fn triple(v: &Value) -> Result<f64, String> {
    if v.is_null() {
        return Err("None triple".to_string());
    }
    if let Some(f) = py_float(v) {
        return Ok(f);
    }
    if let Some(a) = v.as_array() {
        if a.len() >= 3 {
            let d = if py_truthy_local(&a[2]) {
                py_float(&a[2]).unwrap_or(1.0)
            } else {
                1.0
            };
            let b0 = py_float(&a[0]).ok_or_else(|| format!("bad triple {}", a[0]))?;
            let b1 = py_float(&a[1]).unwrap_or(0.0);
            return Ok(b0 + b1 / d);
        }
    }
    // Python 的兜底是 float(v)，对字符串有效
    py_float(v).ok_or_else(|| format!("bad triple {}", py_display(v)))
}

fn py_truthy_local(v: &Value) -> bool {
    // 与 pyconv::py_truthy 相同语义（单独写是为了让 triple 的对照一眼可见）
    match v {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        serde_json::Value::String(s) => !s.is_empty(),
        serde_json::Value::Array(a) => !a.is_empty(),
        serde_json::Value::Object(o) => !o.is_empty(),
    }
}

/// 拍 → 秒 的换算器（分段 BPM 积分）。
#[derive(Debug, Clone)]
pub struct ToSec {
    bl: Vec<(f64, f64)>,
}

impl ToSec {
    pub fn new(bl: Vec<(f64, f64)>) -> Self {
        let mut bl = bl;
        bl.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        bl.retain(|(_, bpm)| *bpm > 0.0);
        ToSec { bl }
    }

    pub fn at(&self, b: f64) -> f64 {
        beats_to_sec(b, &self.bl)
    }

    pub fn is_empty(&self) -> bool {
        self.bl.is_empty()
    }
}

/// 便捷构造。
pub fn make_to_sec(bl: Vec<(f64, f64)>) -> ToSec {
    ToSec::new(bl)
}

/// 分段 BPM 下 beats → 秒。`bl = [(start_beat, bpm)]` **升序**。
///
/// 逐字对照 Python：
/// ```python
/// def beats_to_sec(b, bl):
///     if not bl: return 0.0
///     sec = 0.0
///     for i, (bs, bpm) in enumerate(bl):
///         if b <= bs: return sec
///         nxt = bl[i+1][0] if i+1 < len(bl) else None
///         end = b if nxt is None else min(b, nxt)
///         if bpm and bpm > 0: sec += (end - bs) * 60.0 / bpm
///         if nxt is None or b <= nxt: return sec
///     return sec
/// ```
pub fn beats_to_sec(b: f64, bl: &[(f64, f64)]) -> f64 {
    if bl.is_empty() {
        return 0.0;
    }
    let mut sec = 0.0f64;
    for (i, (bs, bpm)) in bl.iter().enumerate() {
        if b <= *bs {
            return sec;
        }
        let nxt = bl.get(i + 1).map(|x| x.0);
        let end = match nxt {
            None => b,
            Some(n) => b.min(n),
        };
        if *bpm != 0.0 && *bpm > 0.0 {
            sec += (end - bs) * 60.0 / bpm;
        }
        match nxt {
            None => return sec,
            Some(n) => {
                if b <= n {
                    return sec;
                }
            }
        }
    }
    sec
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn triple_forms() {
        assert_eq!(triple(&json!(1.5)).unwrap(), 1.5);
        assert_eq!(triple(&json!([1, 0, 1])).unwrap(), 1.0);
        assert_eq!(triple(&json!([2, 1, 2])).unwrap(), 2.5);
        // c=0 在 Python 里是 falsy → `d = v[2] or 1.0` → 按 1 处理 → 2 + 1/1 = 3.0
        assert_eq!(triple(&json!([2, 1, 0])).unwrap(), 3.0);
        assert!(triple(&json!(null)).is_err());
    }

    #[test]
    fn beats_single_bpm() {
        let bl = vec![(0.0, 120.0)];
        assert_eq!(beats_to_sec(0.0, &bl), 0.0);
        assert_eq!(beats_to_sec(2.0, &bl), 1.0);   // 2 拍 @120 = 1 秒
        assert_eq!(beats_to_sec(4.0, &bl), 2.0);
    }

    #[test]
    fn beats_multi_bpm_matches_python() {
        // (0,120) (4,240)：前 4 拍每拍 0.5s，之后每拍 0.25s
        let bl = vec![(0.0, 120.0), (4.0, 240.0)];
        assert_eq!(beats_to_sec(2.0, &bl), 1.0);
        assert_eq!(beats_to_sec(4.0, &bl), 2.0);
        assert_eq!(beats_to_sec(6.0, &bl), 2.5);
        assert_eq!(beats_to_sec(4.0, &vec![]), 0.0);
    }
}
