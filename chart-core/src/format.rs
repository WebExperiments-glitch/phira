//! 格式嗅探 —— **逐字复刻** `prpr-pbc/src/main.rs:69-81`（经 Python `official_format`）。
//!
//! ```python
//! def official_format(raw):
//!     try:    t = raw.decode("utf-8")
//!     except UnicodeDecodeError: return "pbc"
//!     if t.startswith("{"):
//!         return "rpe" if '"META"' in t else "pgr"
//!     return "pec"
//! ```
//!
//! ⚠️ 顺序不能反：必须**先**做严格 UTF-8 解码（失败 → pbc），
//! **再**判断 `{`。JS/Rust 里对应 `String::from_utf8`（严格），
//! 而后续解析用 `from_utf8_lossy`（errors="replace"）—— 两处语义不同，别混用。

pub fn official_format(raw: &[u8]) -> &'static str {
    let t = match std::str::from_utf8(raw) {
        Ok(t) => t,
        Err(_) => return "pbc",
    };
    if t.starts_with('{') {
        if t.contains("\"META\"") {
            "rpe"
        } else {
            "pgr"
        }
    } else {
        "pec"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff() {
        assert_eq!(official_format(b"{\"META\":{}}"), "rpe");
        assert_eq!(official_format(b"{\"bpm\":120}"), "pgr");
        assert_eq!(official_format(b"# comment\ncp 0 0 1024 700"), "pec");
        assert_eq!(official_format(b"\xff\xfe\x00"), "pbc");   // 非 UTF-8
        assert_eq!(official_format(b""), "pec");               // 空文件 → 不是 { → pec
    }
}
