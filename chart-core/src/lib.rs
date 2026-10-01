//! # chart_core
//!
//! Phira / RPE 谱面解析与难度分析核心。
//!
//! ## 来源与验收
//! 本 crate 是 **Python 参考实现的逐字移植**（不是重写）：
//! - `tools/audit_dataset.py`  → `parse` 模块（S1）
//! - `server/lines.py`         → `lines` 模块（S5，待移植）
//! - `server/pro_metrics.py`   → `metrics` 模块（S3，待移植）
//! - `server/audit.py`         → `audit` 模块（S4，待移植）
//!
//! 验收方式：[`crates/golden_check`]（仓库内 `server/port/golden/*.json` 黄金夹具，
//! 由 Python 版生成，Rust 版必须逐字段复现，数值相对误差 ≤ 1e-9）。
//!
//! ## 为什么是 Rust
//! 面板要打包成 iOS App，iOS 起不了 Python 子进程；Rust 同时覆盖
//! Windows（cdylib/staticlib）与 iOS（staticlib → XCFramework），
//! 而且与 Phira 上游（prpr，Rust）同语言，将来可以直接互调。
//!
//! ## 移植纪律（见 server/port/PORT-SPEC.md）
//! 1. **不要凭理解重写**，要逐函数对照 Python；
//! 2. **不要改运算顺序** —— Python float 与 Rust f64 都是 IEEE754 double，
//!    同样的顺序应得到逐位相同的结果；
//! 3. 遇到 Python 的隐式语义（`0 or -1`、`int("3")`、`float("1.5")`、银行家舍入）
//!    在 [`pyconv`] 里显式建模，并写注释说明。

pub mod format;
pub mod info;
pub mod lines;
pub mod note;
pub mod parse;
pub mod pyconv;
pub mod time;

pub use note::Note;
pub use parse::{parse_chart_file, Parsed};

/// 仓库根（用于 golden_check 找夹具对应的谱面源文件）。
/// 用编译期环境变量注入，避免硬编码盘符。
pub const AIROOT_ENV: &str = "CHART_AIROOT";
