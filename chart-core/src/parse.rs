//! S1 解析：格式嗅探 + RPE / PEC / PGR → 音符列表。
//!
//! **逐字对照 `tools/audit_dataset.py`**（`official_format` / `parse_rpe` /
//! `parse_pec` / `parse_pgr` / `parse_chart_file`）。
//! 每一段都标注了对应的 Python 行为；发现行为差异请先改 Python 再改这里，
//! 然后重新生成黄金夹具。

use serde_json::Value;

use crate::format::official_format;
use crate::info::Info;
use crate::note::{num, Note};
use crate::pyconv::{py_display, py_float, py_int_str, py_truthy};
use crate::time::{beats_to_sec, triple};

/// 官方常数（来源见注释）。
pub const RPE_WIDTH: f64 = 1350.0;        // prpr/src/parse/rpe.rs:22
pub const RPE_X_DIV: f64 = RPE_WIDTH / 2.0; // rpe.rs:588  x/(WIDTH/2)
pub const PEC_X_DIV: f64 = 1024.0;        // pec.rs:253
pub const PEC_OFFSET_BIAS: f64 = 0.15;    // pec.rs:221  offset = ms/1000 - 0.15
pub const PGR_X_MUL: f64 = 2.0 * 9.0 / 160.0; // pgr.rs:207  x*(2*9/160)

/// 解析结果。
#[derive(Debug, Clone)]
pub struct Parsed {
    pub format: &'static str,
    pub notes: Vec<Note>,
    pub bpm_list: Vec<(f64, f64)>,
    pub info: Info,
    pub problems: Vec<String>,
}

/// ⚠️ 音符类型映射**三种格式不一样**，不要"统一"它们。
/// 来源 `rpe.rs:540-550` / `pec.rs:244-250` / `pgr.rs:186-196`。
///
/// | 数字 | RPE / PEC | PGR |
/// |---|---|---|
/// | 1 | tap | tap |
/// | 2 | hold | **drag** |
/// | 3 | flick | **hold** |
/// | 4 | drag | flick |
///
/// 兜底名对齐 Python 的 `DICT.get(t, f"?{t}")`：`t` 缺失 → `?None`。
fn rpe_note_name(t: Option<&Value>) -> String {
    match t.and_then(py_float) {
        Some(x) if x == 1.0 => "tap".into(),
        Some(x) if x == 2.0 => "hold".into(),
        Some(x) if x == 3.0 => "flick".into(),
        Some(x) if x == 4.0 => "drag".into(),
        _ => format!("?{}", t.map(py_display).unwrap_or_else(|| "None".into())),
    }
}

fn pec_note_name(kind: u8) -> String {
    match kind {
        1 => "tap".into(),
        2 => "hold".into(),
        3 => "flick".into(),
        4 => "drag".into(),
        _ => "?None".into(),
    }
}

fn pgr_note_name(t: Option<&Value>) -> String {
    match t.and_then(py_float) {
        Some(x) if x == 1.0 => "tap".into(),
        Some(x) if x == 2.0 => "drag".into(),
        Some(x) if x == 3.0 => "hold".into(),
        Some(x) if x == 4.0 => "flick".into(),
        _ => format!("?{}", t.map(py_display).unwrap_or_else(|| "None".into())),
    }
}

fn as_arr<'a>(v: Option<&'a Value>) -> &'a [Value] {
    match v.and_then(|x| x.as_array()) {
        Some(a) => a,
        None => &[],
    }
}

/// 公开版（lines 模块也用）。
pub fn arr<'a>(v: Option<&'a Value>) -> &'a [Value] {
    as_arr(v)
}

/// RPE 的 BPMList → `[(start_beat, bpm)]`（不含回退与问题记录，供 lines 模块复用）。
pub fn rpe_bpm_list(obj: &Value) -> Vec<(f64, f64)> {
    let mut bl: Vec<(f64, f64)> = vec![];
    for e in as_arr(obj.get("BPMList")) {
        if !e.is_object() {
            continue;
        }
        if let (Some(s), Some(b)) = (e.get("startTime"), e.get("bpm").and_then(py_float)) {
            if let Ok(t) = triple(s) {
                bl.push((t, b));
            }
        }
    }
    if bl.is_empty() {
        bl.push((0.0, obj.get("bpm").and_then(py_float).unwrap_or(120.0)));
    }
    bl
}

/// PGR 的 BPM（无 BPMList，单值）。
pub fn pgr_bpm(obj: &Value) -> f64 {
    obj.get("bpm").and_then(py_float).unwrap_or(120.0)
}

// ============================================================ RPE
/// RPE 解析。返回 `(notes, bpm_list, info, schema_problems)`。
///
/// 字段与「必需性」严格照官方 RPENote（`prpr/src/parse/rpe.rs:144-165`）：
/// `type / above / startTime / endTime / positionX` 均为**无 default 的必需字段**
/// （serde 缺字段即报错 → 在 Phira 里会「无法进入游戏」）。
/// 注意：官方**不使用** `holdTime`、也不使用 `x`；位置字段是 `positionX`。
/// Hold 的结束时间是 `endTime`（rpe.rs:541）。
pub fn parse_rpe(obj: &Value) -> Result<(Vec<Note>, Vec<(f64, f64)>, Info, Vec<String>), String> {
    let mut problems: Vec<String> = vec![];
    let mut bpm_list: Vec<(f64, f64)> = vec![];

    for e in as_arr(obj.get("BPMList")) {
        if !e.is_object() {
            continue;
        }
        // Python: triple(e.get("startTime")) + float(e.get("bpm"))，任一失败 → bpm_item_bad
        let st = e.get("startTime").filter(|v| !v.is_null());
        let bpmv = e.get("bpm").filter(|v| !v.is_null());
        let st = match st {
            Some(s) => match triple(s) {
                Ok(t) => t,
                Err(_) => {
                    problems.push("bpm_item_bad".into());
                    continue;
                }
            },
            None => {
                problems.push("bpm_item_bad".into());
                continue;
            }
        };
        match bpmv.and_then(py_float) {
            Some(b) => bpm_list.push((st, b)),
            None => problems.push("bpm_item_bad".into()),
        }
    }
    // Python: bpm_list.sort() —— 元组排序（先按拍，再按 bpm）
    bpm_list.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.partial_cmp(&b.1).unwrap()));
    if bpm_list.is_empty() {
        let bpm = obj.get("bpm").and_then(py_float).unwrap_or(120.0);
        bpm_list.push((0.0, bpm));
        problems.push("bpm_list_empty_fallback".into());
    }

    let mut notes: Vec<Note> = vec![];
    let lines = as_arr(obj.get("judgeLineList"));
    for l in lines {
        if !l.is_object() {
            continue;
        }
        // 判定线自身的 x 位移范围（moveXEvents 缩放同为 2/RPE_WIDTH，rpe.rs:683）
        let mut lx: Vec<f64> = vec![l.get("positionX").and_then(py_float).unwrap_or(0.0) / RPE_X_DIV];
        for lay in as_arr(l.get("eventLayers")) {
            if !lay.is_object() {
                continue;
            }
            for ev in as_arr(lay.get("moveXEvents")) {
                if !ev.is_object() {
                    continue;
                }
                for k in ["start", "end"] {
                    if let Some(v) = ev.get(k).and_then(py_float) {
                        lx.push(v / RPE_X_DIV);
                    }
                }
            }
        }
        let lmin = lx.iter().cloned().fold(f64::INFINITY, f64::min);
        let lmax = lx.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

        for n in as_arr(l.get("notes")) {
            if !n.is_object() {
                continue;
            }
            // 必需字段缺失/为 null → 记问题（不抛异常，继续解析）
            for f in ["type", "above", "startTime", "endTime", "positionX"] {
                if n.get(f).map_or(true, |v| v.is_null()) {
                    problems.push(format!("missing_{}", f));
                }
            }
            let ts = triple(n.get("startTime").unwrap_or(&Value::Null))?;
            let tval = n.get("type");
            let t = tval.and_then(py_float);
            let xr = n.get("positionX").and_then(py_float).unwrap_or(0.0) / RPE_X_DIV;
            let mut hold = 0.0f64;
            if t == Some(2.0) {
                let end = triple(n.get("endTime").unwrap_or(&Value::Null))?;
                hold = beats_to_sec(end, &bpm_list) - beats_to_sec(ts, &bpm_list);
            }
            let ab = beats_to_sec(ts, &bpm_list);
            let is_fake = match n.get("isFake") {
                Some(v) if py_truthy(v) => 1,
                _ => 0,
            };
            notes.push(Note {
                t: ab,
                kind: rpe_note_name(tval),
                x_rel: xr,
                hold_sec: hold,
                beat: ts,
                x_abs_lo: Some(xr + lmin),
                x_abs_hi: Some(xr + lmax),
                is_fake,
            });
        }
    }
    // Python: notes.sort(key=lambda z: z[0]) —— 稳定排序
    notes.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap());
    let info = Info {
        judge_lines: Some(lines.len()),
        bpm_count: bpm_list.len(),
        bpm_min: Some(bpm_list.iter().map(|x| x.1).fold(f64::INFINITY, f64::min)),
        bpm_max: Some(bpm_list.iter().map(|x| x.1).fold(f64::NEG_INFINITY, f64::max)),
        pec_offset_sec: None,
        error: None,
    };
    Ok((notes, bpm_list, info, problems))
}

// ============================================================ PEC
/// PEC 行式文本解析。
///
/// 字段序与官方一致（`pec.rs:228-300`）：
/// ```text
/// n1 <line> <time> <x/1024> <above> <fake>
/// n2 <line> <time> <end_time> <x/1024> <above> <fake>
/// n3/n4 <line> <time> <x/1024> <above> <fake>
/// bp <beats> <bpm>
/// ```
/// 首行 = 偏移(ms)，官方换算 `pec.rs:221` → `sec = ms/1000 - 0.15`。
pub fn parse_pec(text: &str) -> (Vec<Note>, Vec<(f64, f64)>, Info, Vec<String>) {
    let mut problems: Vec<String> = vec![];
    let text = if let Some(rest) = text.strip_prefix('\u{feff}') {
        problems.push("bom".into());
        rest
    } else {
        text
    };

    let mut off = 0.0f64;
    let mut bpm_list: Vec<(f64, f64)> = vec![];
    // (line, t0_beat, t1_beat, kind_index, x_rel, is_fake)
    struct Raw {
        li: i64,
        t0: f64,
        t1: Option<f64>,
        kind: u8,
        x_rel: f64,
        is_fake: i64,
    }
    let mut raw_notes: Vec<Raw> = vec![];
    let mut line_x: std::collections::BTreeMap<i64, Vec<f64>> = Default::default();

    for (i, ln) in text.split('\n').enumerate() {
        let s = ln.trim();
        if s.is_empty() {
            continue;
        }
        let cs: Vec<&str> = s.split_whitespace().collect();
        let head = cs[0];

        // Python：首行若不是指令，就当偏移（⚠️ 用**整行** `s` 做 float()，不是 `head`）
        if i == 0
            && !matches!(head, "bp" | "cp" | "cm" | "cd" | "cr" | "ca" | "cf" | "cv"
                             | "n1" | "n2" | "n3" | "n4" | "#" | "&")
        {
            match s.parse::<f64>() {
                Ok(v) => off = v / 1000.0 - PEC_OFFSET_BIAS,
                Err(_) => problems.push("bad_first_line".into()),
            }
            continue;
        }

        match head {
            "bp" => {
                if cs.len() >= 3 {
                    match (cs[1].parse::<f64>(), cs[2].parse::<f64>()) {
                        (Ok(b), Ok(v)) => bpm_list.push((b, v)),
                        _ => problems.push("bp_value".into()),
                    }
                } else {
                    problems.push("bp_short".into());
                }
            }
            "cp" | "cm" => {
                // cp <line> <beat> <x> <y> ; cm <line> <b0> <b1> <x> <y> <easing>
                // ⚠️ x 的下标是 3（cp）/ 4（cm）。写成 4/5 会取到 **y 坐标**
                //    （默认 `cp 0 0 1024.00 700.00` → 700/1024 = 0.684），
                //    让 PEC 的 x_abs 整体错位。2026-10-01 审计发现并在两侧同步修正。
                let xi = if head == "cp" { 3usize } else { 4usize };
                match (py_int_str(cs[1]), cs.get(xi).and_then(|v| v.parse::<f64>().ok())) {
                    (Some(li), Some(x)) => {
                        // ⚠️ 判定线 x 的归一化是 **(x-1024)/1024**（官方 pec.rs：`end/2048.*2.-1.`），
                //    不是 `x/1024`（那是**音符**的缩放）。两者原点差 1.0。
                line_x.entry(li).or_default().push((x - PEC_X_DIV) / PEC_X_DIV);
                    }
                    _ => problems.push(format!("{}_value", head)),
                }
            }
            "n1" | "n2" | "n3" | "n4" => {
                let kind = head.as_bytes()[1] - b'0';
                let need = if kind == 2 { 7usize } else { 6usize };
                if cs.len() < need {
                    problems.push(format!("{}_short", head));
                    continue;
                }
                let li = match py_int_str(cs[1]) {
                    Some(v) => v,
                    None => {
                        problems.push(format!("{}_value", head));
                        continue;
                    }
                };
                let t0 = match cs[2].parse::<f64>() {
                    Ok(v) => v,
                    Err(_) => {
                        problems.push(format!("{}_value", head));
                        continue;
                    }
                };
                let (t1, xi, fi) = if kind == 2 {
                    match cs[3].parse::<f64>() {
                        Ok(v) => (Some(v), 4usize, 6usize),
                        Err(_) => {
                            problems.push(format!("{}_value", head));
                            continue;
                        }
                    }
                } else {
                    (None, 3usize, 5usize)
                };
                let x = match cs.get(xi).and_then(|v| v.parse::<f64>().ok()) {
                    Some(v) => v / PEC_X_DIV,
                    None => {
                        problems.push(format!("{}_value", head));
                        continue;
                    }
                };
                let is_fake = if cs.len() > fi && cs[fi] == "1" { 1 } else { 0 };
                raw_notes.push(Raw { li, t0, t1, kind, x_rel: x, is_fake });
            }
            _ => {}
        }
    }

    bpm_list.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.partial_cmp(&b.1).unwrap()));
    if bpm_list.is_empty() {
        bpm_list.push((0.0, 120.0));
        problems.push("no_bp_fallback_120".into());
    }
    // Python：len({round(b,6) for _,b in bpm_list}) > 1 —— 多 BPM 只记录不罚分，
    // 且会在返回前过滤掉 multi_bpm_ok
    let distinct: std::collections::BTreeSet<i64> = bpm_list
        .iter()
        .map(|x| crate::pyconv::py_round_to(x.1, 6) as i64)
        .collect();
    if distinct.len() > 1 {
        problems.push("multi_bpm_ok".into());
    }

    let mut notes: Vec<Note> = vec![];
    for r in &raw_notes {
        let s0 = beats_to_sec(r.t0, &bpm_list) + off;
        let hold = match r.t1 {
            Some(t1) => beats_to_sec(t1, &bpm_list) + off - s0,
            None => 0.0,
        };
        let (lmin, lmax) = match line_x.get(&r.li) {
            Some(v) if !v.is_empty() => {
                (v.iter().cloned().fold(f64::INFINITY, f64::min),
                 v.iter().cloned().fold(f64::NEG_INFINITY, f64::max))
            }
            _ => (0.0, 0.0),
        };
        notes.push(Note {
            t: s0,
            kind: pec_note_name(r.kind),
            x_rel: r.x_rel,
            hold_sec: hold,
            beat: r.t0,
            x_abs_lo: Some(r.x_rel + lmin),
            x_abs_hi: Some(r.x_rel + lmax),
            is_fake: r.is_fake,
        });
    }
    notes.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap());

    let info = Info {
        judge_lines: None,
        bpm_count: bpm_list.len(),
        bpm_min: Some(bpm_list.iter().map(|x| x.1).fold(f64::INFINITY, f64::min)),
        bpm_max: Some(bpm_list.iter().map(|x| x.1).fold(f64::NEG_INFINITY, f64::max)),
        pec_offset_sec: Some(off),
        error: None,
    };
    // Python: [p for p in problems if p != "multi_bpm_ok"]
    problems.retain(|p| p != "multi_bpm_ok");
    (notes, bpm_list, info, problems)
}

// ============================================================ PGR
/// PGR 解析。`time` 单位是 **1/32 拍**，`holdTime` 同理。
pub fn parse_pgr(obj: &Value) -> (Vec<Note>, Vec<(f64, f64)>, Info, Vec<String>) {
    let bpm = obj.get("bpm").and_then(py_float).unwrap_or(120.0);
    let mut notes: Vec<Note> = vec![];
    let lines = as_arr(obj.get("judgeLineList"));
    for l in lines {
        if !l.is_object() {
            continue;
        }
        for key in ["notesAbove", "notesBelow"] {
            for n in as_arr(l.get(key)) {
                if !n.is_object() {
                    continue;
                }
                let tb = n.get("time").and_then(py_float).unwrap_or(0.0) / 32.0;
                let t = n.get("type").and_then(py_float);
                let x = n.get("positionX").and_then(py_float).unwrap_or(0.0) * PGR_X_MUL;
                let mut hold = 0.0f64;
                if t == Some(3.0) {
                    let hb = n.get("holdTime").and_then(py_float).unwrap_or(0.0) / 32.0;
                    hold = hb * 60.0 / bpm;
                }
                notes.push(Note {
                    t: tb * 60.0 / bpm,
                    kind: pgr_note_name(n.get("type")),
                    x_rel: x,
                    hold_sec: hold,
                    beat: tb,
                    x_abs_lo: None,       // ⚠️ PGR 元组只有 5 个字段
                    x_abs_hi: None,
                    is_fake: 0,
                });
            }
        }
    }
    notes.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap());
    let info = Info {
        judge_lines: Some(lines.len()),
        bpm_count: 1,
        bpm_min: Some(bpm),
        bpm_max: Some(bpm),
        pec_offset_sec: None,
        error: None,
    };
    (notes, vec![(0.0, bpm)], info, vec![])
}

// ============================================================ 入口
/// 解析一个谱面文件（等价 Python `parse_chart_file`）。
pub fn parse_chart_file(path: &std::path::Path) -> Result<Parsed, String> {
    let raw = std::fs::read(path).map_err(|e| e.to_string())?;
    let fmt = official_format(&raw);
    if fmt == "pbc" {
        return Ok(Parsed {
            format: "pbc",
            notes: vec![],
            bpm_list: vec![],
            info: Info {
                judge_lines: None, bpm_count: 0, bpm_min: None, bpm_max: None,
                pec_offset_sec: None, error: Some("binary"),
            },
            problems: vec!["binary".into()],
        });
    }
    // Python：raw.decode("utf-8", errors="replace")
    let text = String::from_utf8_lossy(&raw).into_owned();
    if fmt == "rpe" {
        let obj: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let (notes, bpm_list, info, problems) = parse_rpe(&obj)?;
        return Ok(Parsed { format: "rpe", notes, bpm_list, info, problems });
    }
    if fmt == "pgr" {
        let obj: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let (notes, bpm_list, info, problems) = parse_pgr(&obj);
        return Ok(Parsed { format: "pgr", notes, bpm_list, info, problems });
    }
    let (notes, bpm_list, info, problems) = parse_pec(&text);
    Ok(Parsed { format: "pec", notes, bpm_list, info, problems })
}

/// 便于 FFI / 调试的 JSON 视图（字段与黄金夹具的 `parse` 完全一致）。
pub fn parsed_to_json(p: &Parsed) -> Value {
    serde_json::json!({
        "format": p.format,
        "bpm_list": p.bpm_list.iter()
            .map(|(b, v)| serde_json::json!([num(*b), num(*v)]))
            .collect::<Vec<_>>(),
        "info": p.info.to_json(),
        "problems": p.problems,
        "notes": p.notes.iter().map(|n| n.to_json()).collect::<Vec<_>>(),
        "n_real": p.notes.iter().filter(|n| n.is_fake == 0).count(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rpe_basic() {
        let obj = json!({
            "META": {}, "formatVersion": 3,
            "BPMList": [{"bpm": 120.0, "startTime": [0, 0, 1]}],
            "judgeLineList": [
                {"Name": "L0", "father": -1, "isCover": 0, "eventLayers": [],
                 "notes": [
                    {"type": 1, "above": 1, "positionX": -675.0, "isFake": 0,
                     "startTime": [0, 0, 1], "endTime": [0, 0, 1]},
                    {"type": 2, "above": 1, "positionX": 0.0, "isFake": 0,
                     "startTime": [2, 0, 1], "endTime": [4, 0, 1]}
                 ]},
                {"Name": "L1", "father": 0, "isCover": 0, "eventLayers": [], "notes": []}
            ]
        });
        let (notes, bpm, info, problems) = parse_rpe(&obj).unwrap();
        assert_eq!(bpm, vec![(0.0, 120.0)]);
        assert_eq!(info.judge_lines, Some(2));
        assert!(problems.is_empty(), "{:?}", problems);
        assert_eq!(notes.len(), 2);
        // positionX -675 → x_rel = -1.0（屏幕左缘）
        assert_eq!(notes[0].x_rel, -1.0);
        assert_eq!(notes[0].kind, "tap");
        // hold 从 2 拍到 4 拍 @120BPM = 1 秒
        assert_eq!(notes[1].kind, "hold");
        assert_eq!(notes[1].hold_sec, 1.0);
    }

    #[test]
    fn rpe_missing_field_recorded() {
        // ⚠️ 官方必需字段是 type/above/startTime/endTime/positionX，
        //    **不含 isFake**（isFake 缺失是合法的，按 0 处理）
        let obj = json!({
            "BPMList": [{"bpm": 120.0, "startTime": [0, 0, 1]}],
            "judgeLineList": [{"notes": [{"type": 1, "positionX": 0.0,
                                          "startTime": [0, 0, 1], "endTime": [0, 0, 1]}]}]
        });
        let (notes, _b, _i, problems) = parse_rpe(&obj).unwrap();
        assert!(problems.iter().any(|p| p == "missing_above"), "{:?}", problems);
        // isFake 缺失不算问题
        assert!(!problems.iter().any(|p| p == "missing_isFake"));
        assert_eq!(notes[0].is_fake, 0);
    }

    #[test]
    fn pec_basic_with_offset() {
        // 注意首行是偏移(ms)，第二行才是 bp
        let text = "500\nbp 0 120.000\ncp 0 0 1024.00 700.00\nn1 0 0 0 1 0\nn2 0 4 8 -512 1 0\n";
        let (notes, _bpm, info, problems) = parse_pec(text);
        // 首行 500ms → 偏移 0.5 - 0.15 = 0.35s
        assert_eq!(info.pec_offset_sec, Some(0.35));
        assert!(problems.is_empty(), "{:?}", problems);
        // n1: 0 拍 @120 → 0s + 0.35
        assert_eq!(notes[0].t, 0.35);
        assert_eq!(notes[0].kind, "tap");
        // ⚠️ 判定线 x 归一化 = **(x-1024)/1024**（官方 pec.rs `end/2048.*2.-1.`，2026-10-01 上网核对）。
        //    默认 `cp 0 0 1024 700` → lmin=lmax=(1024-1024)/1024=0 → x_abs = x_rel + 0。
        //    （曾用 `x/1024`，原点差 1.0 —— 第 16 个缺陷，黄金夹具以「实际=期望+1.0」暴露。）
        assert_eq!(notes[0].x_rel, 0.0);
        assert_eq!(notes[0].x_abs_lo, Some(0.0));
        // n2: hold 4→8 拍 = 2s，x=-0.5
        // ⚠️ 用容差断言：`4.35 - 2.35` 在二进制浮点里是 1.9999999999999996，
        //    Python 版算出的是**同一个值**（运算顺序一致）—— 这不是移植差异。
        assert_eq!(notes[1].kind, "hold");
        assert!((notes[1].hold_sec - 2.0).abs() < 1e-9, "{}", notes[1].hold_sec);
        assert_eq!(notes[1].x_rel, -0.5);
        assert_eq!(notes[1].x_abs_lo, Some(-0.5));
    }

    #[test]
    fn pgr_note_mapping_differs() {
        let obj = json!({
            "bpm": 120.0,
            "judgeLineList": [{"notesAbove": [
                {"type": 2, "time": 0, "positionX": 0.0},
                {"type": 3, "time": 32, "positionX": 160.0, "holdTime": 32}
            ], "notesBelow": []}]
        });
        let (notes, _b, info, _p) = parse_pgr(&obj);
        // ⚠️ PGR：2=drag 3=hold（与 RPE/PEC 不同）
        assert_eq!(notes[0].kind, "drag");
        assert_eq!(notes[1].kind, "hold");
        assert_eq!(notes[0].t, 0.0);
        assert_eq!(notes[1].t, 0.5);        // 32/32 = 1 拍 @120 = 0.5s
        assert_eq!(notes[1].hold_sec, 0.5); // holdTime 32/32 = 1 拍 → 0.5s
        assert_eq!(notes[1].x_abs_lo, None); // PGR 没有绝对 x
        assert_eq!(info.judge_lines, Some(1));
    }
}
