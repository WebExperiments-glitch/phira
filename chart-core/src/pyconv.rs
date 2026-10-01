//! Python 语义的数值/字符串转换。
//!
//! 为什么要专门做一个模块：Python 的 `float()` / `int()` / `or` 都有隐式语义，
//! 直接用 Rust 的 `as_f64()` 会在边角上悄悄走样。移植的原则是
//! **先做到行为一致，再谈"更合理"**。

use serde_json::Value;

/// 等价于 Python 的 `float(v)`（对 JSON 值）。
///
/// · 数字 → f64
/// · 字符串 → 解析（Python `float("1.5")` 合法，还能带前后空白）
/// · true/false → 1.0 / 0.0（Python `float(True) == 1.0`）
/// · null / 数组 / 对象 → None（Python 会抛 TypeError）
pub fn py_float(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

/// 等价于 Python 的 `int(s)`（**只对字符串**）。
///
/// ⚠️ Python 的 `int()` 只接受「可选正负号 + 数字 + 可选前后空白」；
/// `int("3.0")` 会抛 ValueError —— 这与 Rust 的 `str::parse::<i64>()` 不同，
/// 也与"先转 f64 再截断"不同。PEC 的行号解析（`int(cs[1])`）走的是这条，
/// 所以 `cp 1.5 0 1024 700` 在 Python 里是 `cp_value` 问题，这里也必须是。
pub fn py_int_str(s: &str) -> Option<i64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let (sign, digits) = match t.as_bytes()[0] {
        b'+' => (1i64, &t[1..]),
        b'-' => (-1i64, &t[1..]),
        _ => (1i64, t),
    };
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse::<i64>().ok().map(|v| sign * v)
}

/// 等价于 Python 的真值判断（用于 `x or default`）。
pub fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// 等价于 Python 的 `round(x, ndigits)` —— **银行家舍入**（.5 取偶）。
///
/// ⚠️ 移植大坑：JS 的 `Math.round` 是"四舍五入（.5 取大）"，
/// Rust 的 `f64::round` 也是。Python 的 `round` 是"取偶"。
/// 实现：走**正确舍入的十进制转换**（Rust 的 `{:.*}` 格式化就是），
/// 再解析回 f64 —— 与 CPython 的 `round()` 在实用精度上一致。
pub fn py_round_to(x: f64, ndigits: i32) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let n = ndigits.max(0) as usize;
    let s = format!("{:.*}", n, x);
    s.parse::<f64>().unwrap_or(x)
}

/// 把 JSON 值格式化成 Python `f"{v}"` 的样子（用于 `?{t}` 这种兜底类型名）。
///
/// · 整数值的浮点 → 不带小数点（Python 里 `f"{1.0}"` 是 `"1.0"`，但
///   `f"{1}"` 是 `"1"`；这里对齐**整数 JSON 数字**输出整数、浮点输出浮点）
/// · 字符串原样；null → "None"；true/false → "True"/"False"
pub fn py_display(v: &Value) -> String {
    match v {
        Value::Null => "None".to_string(),
        Value::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(u) = n.as_u64() {
                u.to_string()
            } else {
                n.as_f64().map(py_float_display).unwrap_or_default()
            }
        }
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 对齐 Python 的 `str(float)`：`1.0` → `"1.0"`、`0.5` → `"0.5"`、`1e20` → `"1e+20"`。
/// （这只影响 `?{t}` 这种兜底字符串，正常谱面不会走到。）
pub fn py_float_display(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let s = format!("{}", f);
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{}.0", s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn float_basic() {
        assert_eq!(py_float(&json!(1)), Some(1.0));
        assert_eq!(py_float(&json!(1.5)), Some(1.5));
        assert_eq!(py_float(&json!("2.5")), Some(2.5));
        assert_eq!(py_float(&json!(" 3 ")), Some(3.0));
        assert_eq!(py_float(&json!(true)), Some(1.0));
        assert_eq!(py_float(&json!(null)), None);
    }

    #[test]
    fn int_str_strict() {
        assert_eq!(py_int_str("3"), Some(3));
        assert_eq!(py_int_str(" -4 "), Some(-4));
        assert_eq!(py_int_str("3.0"), None);      // Python int("3.0") 抛异常
        assert_eq!(py_int_str("abc"), None);
        assert_eq!(py_int_str(""), None);
    }

    #[test]
    fn truthy_matches_python() {
        assert!(!py_truthy(&json!(0)));
        assert!(py_truthy(&json!(0.5)));
        assert!(!py_truthy(&json!(null)));
        assert!(!py_truthy(&json!("")));
        assert!(py_truthy(&json!("0")));          // 非空字符串为真
    }

    #[test]
    fn display() {
        assert_eq!(py_display(&json!(7)), "7");
        assert_eq!(py_display(&json!(7.5)), "7.5");
        assert_eq!(py_display(&json!(null)), "None");
    }

    #[test]
    fn banker_rounding() {
        // Python: round(0.5)=0, round(1.5)=2, round(2.5)=2
        assert_eq!(py_round_to(0.5, 0), 0.0);
        assert_eq!(py_round_to(1.5, 0), 2.0);
        assert_eq!(py_round_to(2.5, 0), 2.0);
        assert_eq!(py_round_to(0.125, 2), 0.12);  // round(0.125,2)=0.12（取偶）
        assert_eq!(py_round_to(1020.5153, 3), 1020.515);
    }
}
