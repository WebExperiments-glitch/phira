//! S5 判定线分析 —— 逐字对照 `server/lines.py`。
//!
//! 回答：① 判定线多少条 ② 哪几条用来打 ③ 位移多少 ④ 有没有反过来打 ⑤ 音符滚动多快。
//!
//! ⚠️ **单位与口径**（这一块曾经是 15 个缺陷里 6 个的来源）：
//! · 时间轴统一是**秒**（RPE/PEC 原始时间是拍，先用 BPMList/`bp` 换算）
//! · 位置：屏幕半宽 = 1.0，原点在屏幕中心
//! · 速度：屏幕半宽 / 秒（峰速已排除瞬移）
//!
//! ⚠️ **Python 与 Rust 的浮点取模符号相反**：
//!   Python `-10.0 % 360.0 == 350.0`，Rust `-10.0f64 % 360.0 == -10.0`。
//!   所有 `% 360` 都必须走 [`pymod`]，否则翻转/倒置判定会整个错掉。

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;

use crate::pyconv::{py_float, py_int_str, py_round_to, py_truthy};
use crate::time::{make_to_sec, ToSec};
use crate::{parse, time::triple};

pub const SAMPLE_DT: f64 = 0.05; // 采样步长（**真实秒**）
/// 一个 50ms 采样步内位移超过此值 → 判定为瞬时传送（屏幕半宽=1.0，0.35 步 = 7 半宽/秒）
pub const JUMP_UNITS: f64 = 0.35;
pub const RPE_W: f64 = 1350.0;
pub const RPE_H: f64 = 900.0;
pub const PEC_X_DIV: f64 = 1024.0;
pub const PEC_Y_CENTER: f64 = 700.0;
pub const PEC_SPEED_DIV: f64 = 5.85;

/// Python 语义的浮点取模：结果与除数同号。
#[inline]
pub fn pymod(a: f64, b: f64) -> f64 {
    (a % b + b) % b
}

// ============================================================ 事件曲线
#[derive(Debug, Clone, Default)]
pub struct Curve {
    pts: Vec<(f64, f64)>,
    default: f64,
}

impl Curve {
    pub fn new(mut pts: Vec<(f64, f64)>, default: f64) -> Self {
        // Python: sorted(pts, key=lambda z: z[0]) —— 稳定排序，只按键
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        Curve { pts, default }
    }

    pub fn empty(default: f64) -> Self {
        Curve { pts: vec![], default }
    }

    /// 线性插值（二分）。空曲线返回默认值。
    pub fn at(&self, t: f64) -> f64 {
        if self.pts.is_empty() {
            return self.default;
        }
        if t <= self.pts[0].0 {
            return self.pts[0].1;
        }
        let last = self.pts.len() - 1;
        if t >= self.pts[last].0 {
            return self.pts[last].1;
        }
        let mut lo = 0usize;
        let mut hi = last;
        while lo + 1 < hi {
            let mid = (lo + hi) / 2;
            if self.pts[mid].0 <= t {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let (t0, v0) = self.pts[lo];
        let (t1, v1) = self.pts[hi];
        if t1 <= t0 {
            return v1;
        }
        v0 + (v1 - v0) * (t - t0) / (t1 - t0)
    }

    pub fn values(&self) -> Vec<f64> {
        self.pts.iter().map(|x| x.1).collect()
    }
}

/// RPE / PEC 事件列表 → Curve（时间已换算成秒）。
///
/// RPE：`{"startTime":[a,b,c], "endTime":[…], "start":v, "end":v}`
/// PEC 内部形式：`{"time":秒, "endTime":秒, "value":v, "end":v}`
fn events_to_curve(events: &[Value], scale: f64, to_sec: &ToSec) -> Curve {
    let mut pts: Vec<(f64, f64)> = vec![];
    for e in events {
        if !e.is_object() {
            continue;
        }
        // Python: isinstance(st, (list, tuple)) → RPE，否则走 PEC 分支
        let is_rpe = e.get("startTime").map(|v| v.is_array()).unwrap_or(false);
        if is_rpe {
            let t0 = e.get("startTime").and_then(|v| triple(v).ok()).unwrap_or(0.0);
            // ⚠️ Python：`t1 = _triple(et) if isinstance(et,(list,tuple)) else t0`
            //    —— endTime **不是数组**（比如写成了数字）时 Python 直接用 t0，
            //    不能拿数字当拍值用。这一点曾让 12324 的倒置占比差了 6.5 个百分点。
            let t1 = match e.get("endTime") {
                Some(v) if v.is_array() => triple(v).unwrap_or(t0),
                _ => t0,
            };
            let v0 = e.get("start").and_then(py_float)
                .or_else(|| e.get("value").and_then(py_float)).unwrap_or(0.0);
            let v1 = e.get("end").and_then(py_float).unwrap_or(v0);
            pts.push((to_sec.at(t0), v0 * scale));
            if t1 > t0 {
                pts.push((to_sec.at(t1), v1 * scale));
            }
        } else {
            let t0 = e.get("time").and_then(py_float).unwrap_or(0.0);
            let t1 = e.get("endTime").and_then(py_float).unwrap_or(t0);
            let v0 = e.get("value").and_then(py_float)
                .or_else(|| e.get("start").and_then(py_float)).unwrap_or(0.0);
            let v1 = e.get("end").and_then(py_float).unwrap_or(v0);
            pts.push((t0, v0 * scale));
            if t1 > t0 {
                pts.push((t1, v1 * scale));
            }
        }
    }
    Curve::new(pts, 0.0)
}

// ============================================================ 判定线
#[derive(Debug, Clone)]
pub struct Line {
    pub index: usize,
    pub name: String,
    pub father: i64,
    pub is_cover: i64,
    pub movex: Curve,
    pub movey: Curve,
    pub rot: Curve,
    pub speed: Curve,
    pub notes: Vec<(f64, f64)>, // (t_sec, x_rel)
    pub n_events: usize,
}

/// 宽容取整。
///
/// ⚠️ 不要写 `int(x or -1)` —— x 合法地为 0 时，`0 or -1` 会变成 -1。
///    踩过：`father` 用这个写法，导致 **father=0（挂到第一条线上）被当成"没有父线"**，
///    整条父子线层级直接塌掉。
fn as_int(v: Option<&Value>, default: i64) -> i64 {
    match v {
        Some(Value::Number(n)) => n
            .as_i64()
            .unwrap_or_else(|| n.as_f64().unwrap_or(default as f64) as i64),
        _ => default,
    }
}

fn parse_rpe_lines(obj: &Value, to_sec: &ToSec) -> Vec<Line> {
    let mut out = vec![];
    for (i, l) in parse::arr(obj.get("judgeLineList")).iter().enumerate() {
        if !l.is_object() {
            continue;
        }
        let mut mx: Vec<Value> = vec![];
        let mut my: Vec<Value> = vec![];
        let mut rt: Vec<Value> = vec![];
        let mut sp: Vec<Value> = vec![];
        for lay in parse::arr(l.get("eventLayers")) {
            if !lay.is_object() {
                continue;
            }
            mx.extend(parse::arr(lay.get("moveXEvents")).iter().cloned());
            my.extend(parse::arr(lay.get("moveYEvents")).iter().cloned());
            rt.extend(parse::arr(lay.get("rotateEvents")).iter().cloned());
            sp.extend(parse::arr(lay.get("speedEvents")).iter().cloned());
        }
        let mut notes: Vec<(f64, f64)> = vec![];
        for n in parse::arr(l.get("notes")) {
            if !n.is_object() {
                continue;
            }
            if n.get("isFake").map(py_truthy).unwrap_or(false) {
                continue; // 假音符
            }
            // 音符相对判定线的坐标：官方 positionX/(WIDTH/2)，与 moveXEvents 同一把尺
            let xr = n.get("positionX").and_then(py_float).unwrap_or(0.0) * 2.0 / RPE_W;
            let t = n.get("startTime").and_then(|v| triple(v).ok()).unwrap_or(0.0);
            notes.push((to_sec.at(t), xr));
        }
        // ⚠️ Python 是 `sorted(notes)` 对 (t, x_rel) **元组**排序 ——
        //    同一时刻的音符会再按 x 排。只按 t 排（稳定排序）会保留 JSON 顺序，
        //    与 Python 产生不同的 note_x 顺序（黄金夹具抓出来的）。
        notes.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.partial_cmp(&b.1).unwrap()));
        out.push(Line {
            index: i,
            name: l.get("Name").and_then(|v| v.as_str()).unwrap_or("L").to_string(),
            father: as_int(l.get("father"), -1),
            is_cover: as_int(l.get("isCover"), 0),
            movex: events_to_curve(&mx, 2.0 / RPE_W, to_sec),
            movey: events_to_curve(&my, 2.0 / RPE_H, to_sec),
            rot: events_to_curve(&rt, -1.0, to_sec),
            speed: events_to_curve(&sp, 1.0, to_sec),
            notes,
            n_events: mx.len() + my.len() + rt.len(),
        });
    }
    out
}

fn parse_pgr_lines(obj: &Value, to_sec: &ToSec) -> Vec<Line> {
    let bpm = parse::pgr_bpm(obj);
    let mut out = vec![];
    for (i, l) in parse::arr(obj.get("judgeLineList")).iter().enumerate() {
        if !l.is_object() {
            continue;
        }
        let mut sp: Vec<(f64, f64)> = vec![];
        let mut sp_n = 0usize;
        for e in parse::arr(l.get("speedEvents")) {
            if !e.is_object() {
                continue;
            }
            let t = e.get("startTime").and_then(|v| triple(v).ok()).unwrap_or(0.0);
            // ⚠️ Python 是 `float(e.get("value") or 1.0)` —— value 为 0/null/缺失时都取 1.0
            //    （0 是 falsy）。直接 `unwrap_or(1.0)` 会把合法的 0 保留下来，
            //    让哨兵过滤的基准变成 0 → note_speed 全变 [0,0]。
            let vv = e.get("value").filter(|v| py_truthy(v)).and_then(py_float).unwrap_or(1.0);
            sp.push((to_sec.at(t), vv));
            sp_n += 1;
        }
        let mut notes: Vec<(f64, f64)> = vec![];
        for key in ["notesAbove", "notesBelow"] {
            for n in parse::arr(l.get(key)) {
                if !n.is_object() {
                    continue;
                }
                // PGR：time 单位是 1/32 拍，x 用另一套缩放（×2·9/160）
                let tb = n.get("time").and_then(py_float).unwrap_or(0.0) / 32.0;
                notes.push((
                    tb * 60.0 / bpm,
                    n.get("positionX").and_then(py_float).unwrap_or(0.0) * 2.0 * 9.0 / 160.0,
                ));
            }
        }
        // ⚠️ Python 是 `sorted(notes)` 对 (t, x_rel) **元组**排序 ——
        //    同一时刻的音符会再按 x 排。只按 t 排（稳定排序）会保留 JSON 顺序，
        //    与 Python 产生不同的 note_x 顺序（黄金夹具抓出来的）。
        notes.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.partial_cmp(&b.1).unwrap()));
        out.push(Line {
            index: i,
            name: format!("Line {}", i),
            father: -1,
            is_cover: 0,
            movex: Curve::empty(0.0),
            movey: Curve::empty(0.0),
            rot: Curve::empty(0.0),
            speed: Curve::new(sp, 1.0),
            notes,
            n_events: sp_n, /* sp 已被 Curve::new 移动，长度先取 */
        });
    }
    out
}

fn parse_pec_obj_lines(obj: &Value, to_sec: &ToSec) -> Vec<Line> {
    let mut out = vec![];
    for (i, l) in parse::arr(obj.get("lines")).iter().enumerate() {
        if !l.is_object() {
            continue;
        }
        let mut mx: Vec<Value> = vec![];
        let mut my: Vec<Value> = vec![];
        let mut rt: Vec<Value> = vec![];
        let mut sp: Vec<Value> = vec![];
        for lay in parse::arr(l.get("eventLayers")) {
            if !lay.is_object() {
                continue;
            }
            mx.extend(parse::arr(lay.get("moveXEvents")).iter().cloned());
            my.extend(parse::arr(lay.get("moveYEvents")).iter().cloned());
            rt.extend(parse::arr(lay.get("rotateEvents")).iter().cloned());
            sp.extend(parse::arr(lay.get("speedEvents")).iter().cloned());
        }
        let mut notes: Vec<(f64, f64)> = vec![];
        for n in parse::arr(l.get("notes")) {
            if !n.is_object() {
                continue;
            }
            let t = n.get("startTime").and_then(|v| triple(v).ok()).unwrap_or(0.0);
            let xr = n.get("positionX").and_then(py_float).unwrap_or(0.0) * 2.0 / 1024.0;
            notes.push((to_sec.at(t), xr));
        }
        // ⚠️ Python 是 `sorted(notes)` 对 (t, x_rel) **元组**排序 ——
        //    同一时刻的音符会再按 x 排。只按 t 排（稳定排序）会保留 JSON 顺序，
        //    与 Python 产生不同的 note_x 顺序（黄金夹具抓出来的）。
        notes.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.partial_cmp(&b.1).unwrap()));
        out.push(Line {
            index: i,
            name: format!("Line {}", i),
            father: -1,
            is_cover: 0,
            movex: events_to_curve(&mx, 2.0 / 1024.0, to_sec),
            movey: events_to_curve(&my, 2.0 / 700.0, to_sec),
            rot: events_to_curve(&rt, -1.0, to_sec),
            speed: events_to_curve(&sp, 1.0, to_sec),
            notes,
            n_events: mx.len() + my.len() + rt.len(),
        });
    }
    out
}

/// 纯文本 PEC。`bp` 行给 BPM；首行数字是偏移(ms)。
fn parse_pec_text_lines(txt: &str) -> Vec<Line> {
    let mut bl: Vec<(f64, f64)> = vec![];
    let mut off = 0.0f64;
    for (i, ln) in txt.split('\n').enumerate() {
        let s = ln.trim();
        if s.is_empty() {
            continue;
        }
        let cs: Vec<&str> = s.split_whitespace().collect();
        if cs.is_empty() {
            continue;
        }
        if cs[0] == "bp" && cs.len() >= 3 {
            if let (Ok(b), Ok(v)) = (cs[1].parse::<f64>(), cs[2].parse::<f64>()) {
                bl.push((b, v));
            }
        } else if i == 0
            && !matches!(cs[0], "bp" | "cp" | "cm" | "cd" | "cr" | "cv"
                             | "n1" | "n2" | "n3" | "n4" | "#" | "&")
        {
            // ⚠️ 用**整行** s 做 float()（对齐 Python），不是 cs[0]
            if let Ok(v) = s.parse::<f64>() {
                off = v / 1000.0 - 0.15;
            }
        }
    }
    let to_sec = make_to_sec(bl);

    #[derive(Default)]
    struct L {
        mx: Vec<Value>,
        my: Vec<Value>,
        rot: Vec<Value>,
        sp: Vec<Value>,
        notes: Vec<(f64, f64)>,
        n_events: usize,
    }
    let mut lines: BTreeMap<i64, L> = BTreeMap::new();
    let mut order: Vec<i64> = vec![];

    for ln in txt.split('\n') {
        let s = ln.trim();
        if s.is_empty() {
            continue;
        }
        let cs: Vec<&str> = s.split_whitespace().collect();
        if cs.is_empty() {
            continue;
        }
        let head = cs[0];
        // ⚠️ Python 的判据是「首字符在 \"cn\" 里」：
        //    · 任何以 'n' 开头的行都当**音符**处理（它只取 line 和 time，不看 kind）
        //    · 任何 `c?` 双字符行都进命令分支并 **n_events += 1**（包括没实现的 ca/cf）
        //      —— 只认 p/m/d/r/v 会让 n_events 偏少（实测 10018 差 8%），
        //         而 n_events 是「这条线编排复杂度」的参考量，不能少算。
        if !(s.starts_with('c') || s.starts_with('n')) || s.chars().count() < 2 {
            continue;
        }
        let is_note = head.as_bytes()[0] == b'n';
        let is_cmd = head.as_bytes()[0] == b'c' && head.len() == 2;
        if !(is_note || is_cmd) {
            continue;
        }
        let li = match py_int_str(cs[1]) {
            Some(v) => v,
            None => continue,
        };
        if !lines.contains_key(&li) {
            lines.insert(li, L::default());
            order.push(li);
        }
        let t0 = match cs[2].parse::<f64>() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let l = lines.get_mut(&li).unwrap();
        if is_note {
            // Python：`xi = 4 if p[0]=="n2" else 3`（只看是不是 n2，不看 kind）
            let xi = if head == "n2" { 4usize } else { 3usize };
            let xr = cs.get(xi).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0) / PEC_X_DIV;
            l.notes.push((to_sec.at(t0) + off, xr));
            continue;
        }
        let t = to_sec.at(t0) + off;
        match head.as_bytes()[1] as char {
            'p' | 'P' => {
                // cp <line> <t> <x> <y>   （瞬时传送）
                // ⚠️ 坐标原点：cp 的 x 以 1024 为中心、y 以 700 为中心
                if let (Some(x), Some(y)) = (
                    cs.get(3).and_then(|v| v.parse::<f64>().ok()),
                    cs.get(4).and_then(|v| v.parse::<f64>().ok()),
                ) {
                    l.mx.push(json!({"time": t, "value": (x - PEC_X_DIV) / PEC_X_DIV}));
                    l.my.push(json!({"time": t, "value": (y - PEC_Y_CENTER) / PEC_Y_CENTER}));
                }
            }
            'm' | 'M' => {
                // cm <line> <t0> <t1> <x> <y> <easing>
                if let (Some(t1), Some(x), Some(y)) = (
                    cs.get(3).and_then(|v| v.parse::<f64>().ok()),
                    cs.get(4).and_then(|v| v.parse::<f64>().ok()),
                    cs.get(5).and_then(|v| v.parse::<f64>().ok()),
                ) {
                    let t1s = to_sec.at(t1) + off;
                    l.mx.push(json!({"time": t, "endTime": t1s, "value": (x - PEC_X_DIV) / PEC_X_DIV}));
                    l.my.push(json!({"time": t, "endTime": t1s, "value": (y - PEC_Y_CENTER) / PEC_Y_CENTER}));
                }
            }
            'd' | 'D' => {
                // cd <line> <t> <deg>  —— ⚠️ 官方对 rotate 取负
                if let Some(d) = cs.get(3).and_then(|v| v.parse::<f64>().ok()) {
                    l.rot.push(json!({"time": t, "value": -d}));
                }
            }
            'r' | 'R' => {
                if let (Some(t1), Some(d)) = (
                    cs.get(3).and_then(|v| v.parse::<f64>().ok()),
                    cs.get(4).and_then(|v| v.parse::<f64>().ok()),
                ) {
                    l.rot.push(json!({"time": t, "endTime": to_sec.at(t1) + off, "value": -d}));
                }
            }
            'v' | 'V' => {
                if let Some(v) = cs.get(3).and_then(|v| v.parse::<f64>().ok()) {
                    l.sp.push(json!({"time": t, "value": v / PEC_SPEED_DIV}));
                }
            }
            _ => {}
        }
        l.n_events += 1;
    }

    order.iter().map(|i| {
        let l = lines.get(i).unwrap();
        // ⚠️ Python 的 _parse_pec **不排序** notes（保持文件顺序）；
        //    只有 _parse_rpe / _parse_pgr / _parse_pec_obj 用 sorted(notes)。
        //    这里多排一次会让同一时刻音符的 note_x 顺序与 Python 不同（黄金夹具抓出来的）。
        let notes = l.notes.clone();
        Line {
            index: *i as usize,
            name: format!("Line {}", i),
            father: -1,
            is_cover: 0,
            movex: events_to_curve(&l.mx, 1.0, &to_sec),
            movey: events_to_curve(&l.my, 1.0, &to_sec),
            rot: events_to_curve(&l.rot, 1.0, &to_sec),
            speed: events_to_curve(&l.sp, 1.0, &to_sec),
            notes,
            n_events: l.n_events,
        }
    }).collect()
}

// ============================================================ father 复合
/// 沿 father 链复合，返回每条线的父链（下标序列，从根到自身）。
fn world(lines: &[Line]) -> Vec<Vec<usize>> {
    let byidx: BTreeMap<usize, usize> =
        lines.iter().enumerate().map(|(i, l)| (l.index, i)).collect();
    let mut cache: BTreeMap<usize, Vec<usize>> = BTreeMap::new();

    fn chain(
        i: usize,
        lines: &[Line],
        byidx: &BTreeMap<usize, usize>,
        cache: &mut BTreeMap<usize, Vec<usize>>,
        seen: &mut Vec<usize>,
    ) -> Vec<usize> {
        if let Some(c) = cache.get(&i) {
            return c.clone();
        }
        if seen.contains(&i) {
            return vec![]; // 环 / 断链 → 当根处理，不死循环
        }
        seen.push(i);
        let father = lines[i].father;
        let mut res: Vec<usize> = if father >= 0 {
            match byidx.get(&(father as usize)) {
                Some(&fi) => chain(fi, lines, byidx, cache, seen),
                None => vec![],
            }
        } else {
            vec![]
        };
        res.push(i);
        cache.insert(i, res.clone());
        res
    }

    (0..lines.len())
        .map(|i| {
            let mut seen = vec![];
            chain(i, lines, &byidx, &mut cache, &mut seen)
        })
        .collect()
}

/// 一条父链在某时刻的复合位置与旋转（x, y, rot_deg）。
fn world_pos(lines: &[Line], chain: &[usize], t: f64) -> (f64, f64, f64) {
    let mut x = 0.0f64;
    let mut y = 0.0f64;
    let mut rot = 0.0f64;
    for &i in chain {
        let l = &lines[i];
        let lx = l.movex.at(t);
        let ly = l.movey.at(t);
        let c = rot.to_radians().cos();
        let s = rot.to_radians().sin();
        x += lx * c - ly * s;
        y += lx * s + ly * c;
        rot += l.rot.at(t);
    }
    (x, y, rot)
}

// ============================================================ 速度清洗
/// 剔除哨兵值（9999/2340 这类）。基准用 **10 分位** —— 中位数会被占一半的哨兵值污染。
fn clean_speed(c: &Curve) -> Vec<f64> {
    let mut vals = c.values();
    if vals.is_empty() {
        vals.push(1.0);
    }
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = (((vals.len() as f64) * 0.10) as usize).saturating_sub(1);
    let base = vals[idx.min(vals.len() - 1)];
    let keep: Vec<f64> = vals.iter().cloned().filter(|v| *v <= base.max(1e-9) * 50.0).collect();
    let keep = if keep.is_empty() { vec![1.0] } else { keep };
    vec![
        py_round_to(keep.iter().cloned().fold(f64::INFINITY, f64::min), 2),
        py_round_to(keep.iter().cloned().fold(f64::NEG_INFINITY, f64::max), 2),
    ]
}

fn fmin(v: &[f64]) -> f64 {
    v.iter().cloned().fold(f64::INFINITY, f64::min)
}
fn fmax(v: &[f64]) -> f64 {
    v.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
}

// ============================================================ 主入口
#[derive(Debug, Clone)]
pub struct LinesResult {
    pub supported: bool,
    pub reason: Option<String>,
    pub format: &'static str,
    pub total_lines: usize,
    /// 逐音符真实世界坐标 [t, x_rel, x_world, y_world, line_index]
    pub note_x: Vec<[f64; 5]>,
    pub decision: Value,
    pub flip: Value,
    pub lines: Vec<Value>,
    pub method: Value,
}

impl LinesResult {
    pub fn unsupported(reason: String) -> Self {
        LinesResult {
            supported: false,
            reason: Some(reason),
            format: "",
            total_lines: 0,
            note_x: vec![],
            decision: Value::Null,
            flip: Value::Null,
            lines: vec![],
            method: Value::Null,
        }
    }

    pub fn to_json(&self) -> Value {
        if !self.supported {
            return json!({"supported": false, "reason": self.reason});
        }
        json!({
            "supported": true,
            "format": self.format,
            "total_lines": self.total_lines,
            "decision": self.decision,
            "flip": self.flip,
            "lines": self.lines,
            "method": self.method,
        })
    }
}

pub fn analyze_lines(chart_path: &Path) -> Result<LinesResult, String> {
    let raw = std::fs::read(chart_path).map_err(|e| e.to_string())?;
    let obj: Option<Value> = match std::str::from_utf8(&raw) {
        Ok(t) => serde_json::from_str(t).ok(),
        Err(_) => None,
    };
    let txt = String::from_utf8_lossy(&raw).into_owned();

    let (fmt, lines): (&'static str, Vec<Line>) = if let Some(o) = &obj {
        if o.get("judgeLineList").is_some() {
            if o.get("META").is_some() {
                let to_sec = make_to_sec(parse::rpe_bpm_list(o));
                ("rpe", parse_rpe_lines(o, &to_sec))
            } else {
                let to_sec = make_to_sec(vec![(0.0, parse::pgr_bpm(o))]);
                ("pgr", parse_pgr_lines(o, &to_sec))
            }
        } else if o.get("lines").is_some() {
            let to_sec = make_to_sec(vec![(0.0, o.get("bpm").and_then(py_float).unwrap_or(120.0))]);
            ("pec", parse_pec_obj_lines(o, &to_sec))
        } else {
            ("pec", parse_pec_text_lines(&txt))
        }
    } else {
        ("pec", parse_pec_text_lines(&txt))
    };

    if lines.is_empty() {
        return Ok(LinesResult::unsupported("该格式未提供判定线信息".into()));
    }

    let total = lines.len();
    let all_notes: usize = lines.iter().map(|l| l.notes.len()).sum();
    let tmax = lines.iter().filter_map(|l| l.notes.last().map(|n| n.0)).fold(f64::NEG_INFINITY, f64::max);
    let tmin = lines.iter().filter_map(|l| l.notes.first().map(|n| n.0)).fold(f64::INFINITY, f64::min);
    let (tmax, tmin) = (
        if tmax.is_finite() { tmax } else { 0.0 },
        if tmin.is_finite() { tmin } else { 0.0 },
    );
    let span = (tmax - tmin).max(1e-6);

    let chains = world(&lines);
    let ngrid = (span / SAMPLE_DT) as usize + 1;
    let grid: Vec<f64> = (0..ngrid).map(|i| tmin + i as f64 * SAMPLE_DT).collect();

    // ---- 逐音符的真实世界坐标 ----
    // 「左手/右手」「同键连击」必须用**屏幕位置**。audit_dataset 的 x_abs 只是
    // 判定线位移区间的端点，线一摆动端点就覆盖整屏，没有分辨力。
    let mut note_x: Vec<[f64; 5]> = vec![];
    for l in &lines {
        let ch = &chains[l.index];
        for (t, xr) in &l.notes {
            let (wx, wy, rot) = world_pos(&lines, ch, *t);
            let (c, s) = (rot.to_radians().cos(), rot.to_radians().sin());
            note_x.push([
                py_round_to(*t, 4),
                py_round_to(*xr, 4),
                py_round_to(wx + xr * c, 4),
                py_round_to(wy + xr * s, 4),
                l.index as f64,
            ]);
        }
    }

    // ---- 逐线采样世界轨迹 ----
    let mut infos: Vec<Value> = vec![];
    let mut line_meta: Vec<(usize, usize, f64, i64)> = vec![]; // (index, notes, pct, flips)
    for l in &lines {
        let ch = &chains[l.index];
        let mut xs: Vec<f64> = Vec::with_capacity(grid.len());
        let mut ys: Vec<f64> = Vec::with_capacity(grid.len());
        let mut rots: Vec<f64> = Vec::with_capacity(grid.len());
        let mut prev: Option<(f64, f64)> = None;
        let mut peak = 0.0f64;
        let mut dist = 0.0f64;
        let mut move_dist = 0.0f64;
        let mut jumps = 0i64;
        for &t in &grid {
            let (x, y, r) = world_pos(&lines, ch, t);
            xs.push(x);
            ys.push(y);
            rots.push(r);
            if let Some((px, py)) = prev {
                let d = (x - px).hypot(y - py);
                dist += d;
                if d > JUMP_UNITS {
                    jumps += 1; // 瞬时传送，不计入速度
                } else {
                    move_dist += d;
                    peak = peak.max(d / SAMPLE_DT);
                }
            }
            prev = Some((x, y));
        }
        // 翻转：|norm|<90 = 朝上，>90 = 倒置；翻转次数 = 进入倒置的次数（带 ±5° 滞回）
        let ns: Vec<f64> = rots.iter().map(|r| pymod(r + 180.0, 360.0) - 180.0).collect();
        let upright = ns.iter().filter(|v| v.abs() < 90.0).count();
        let upside_pct = 100.0 * (ns.len() - upright) as f64 / ns.len().max(1) as f64;
        let mut flips = 0i64;
        let mut state = ns.first().map(|v| v.abs() > 90.0).unwrap_or(false);
        for v in &ns[1..] {
            let inv = if v.abs() > 95.0 {
                true
            } else if v.abs() < 85.0 {
                false
            } else {
                continue; // 滞回区：保持原状态
            };
            if inv && !state {
                flips += 1; // 只数「翻成倒置」的次数
            }
            state = inv;
        }
        let n_note = l.notes.len();
        let pct = 100.0 * n_note as f64 / all_notes.max(1) as f64;
        line_meta.push((l.index, n_note, pct, flips));

        infos.push(json!({
            "index": l.index,
            "name": l.name,
            "notes": n_note,
            "pct": py_round_to(pct, 2),
            "x_min": py_round_to(fmin(&xs), 3),
            "x_max": py_round_to(fmax(&xs), 3),
            "y_min": py_round_to(fmin(&ys), 3),
            "y_max": py_round_to(fmax(&ys), 3),
            "x_amp": py_round_to(fmax(&xs) - fmin(&xs), 3),
            "y_amp": py_round_to(fmax(&ys) - fmin(&ys), 3),
            "travel": py_round_to(move_dist, 2),
            "travel_raw": py_round_to(dist, 2),
            "jumps": jumps,
            "mean_speed": py_round_to(move_dist / span, 3),
            "peak_speed": py_round_to(peak, 3),
            "rot_min": py_round_to(fmin(&rots), 1),
            "rot_max": py_round_to(fmax(&rots), 1),
            "rot_norm": [py_round_to(fmin(&ns), 1), py_round_to(fmax(&ns), 1)],
            "upside_down_pct": py_round_to(upside_pct, 1),
            "flips": flips,
            "note_speed": clean_speed(&l.speed),
            "is_child": l.father >= 0,
            "is_cover": l.is_cover,
            "n_events": l.n_events,
        }));
    }

    // ---- 用来打的线：承载音符，按音符数降序，累计到 90% 覆盖 ----
    let mut playing: Vec<(usize, usize, f64)> = line_meta
        .iter()
        .filter(|(_, n, _, _)| *n > 0)
        .map(|(i, n, p, _)| (*i, *n, *p))
        .collect();
    playing.sort_by(|a, b| b.1.cmp(&a.1));
    let mut cover = 0.0f64;
    let mut main: Vec<Value> = vec![];
    for (idx, n, pct) in &playing {
        let name = lines
            .iter()
            .find(|l| l.index == *idx)
            .map(|l| l.name.clone())
            .unwrap_or_default();
        main.push(json!({"index": idx, "name": name, "notes": n, "pct": py_round_to(*pct, 2)}));
        cover += pct;
        if cover >= 90.0 {
            break;
        }
    }
    let flipping: Vec<Value> = line_meta
        .iter()
        .filter(|(_, n, _, f)| *f > 0 && *n > 0)
        .map(|(i, n, _, f)| {
            let info = infos.iter().find(|v| v["index"] == json!(i)).unwrap();
            json!({
                "index": i, "notes": n, "flips": f,
                "upside_down_pct": info["upside_down_pct"],
                "rot_range": [info["rot_min"], info["rot_max"]],
            })
        })
        .collect();

    Ok(LinesResult {
        supported: true,
        reason: None,
        format: fmt,
        total_lines: total,
        note_x,
        decision: json!({
            "playing_lines": playing.len(),
            "main_lines": main,
            "coverage_pct": py_round_to(cover, 1),
            "decorative_lines": total - playing.len(),
        }),
        flip: json!({"any": !flipping.is_empty(), "lines": flipping}),
        lines: infos,
        method: json!({
            "sample_dt": SAMPLE_DT,
            "speed_unit": "屏幕半宽/秒",
            "time_unit": "秒（已用 BPMList 把拍换算成秒）",
            "note": "缓动按关键帧线性插值：位移范围准确，速度是近似值（50ms 网格会低估极快往复）；子线已沿 father 链复合父线位置与旋转。",
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tap_notes(ts: &[f64], x: f64) -> Vec<Value> {
        ts.iter()
            .map(|t| json!({"type": 1, "above": 1, "positionX": x, "isFake": 0,
                            "startTime": [t, 0, 1], "endTime": [t, 0, 1]}))
            .collect()
    }

    fn analyze_value(obj: serde_json::Value) -> Value {
        // ⚠️ 测试并行跑在同一个进程里，临时文件名必须唯一，否则会互相覆盖
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("lc_test_{}_{}.json", std::process::id(), n));
        std::fs::write(&p, serde_json::to_string(&obj).unwrap()).unwrap();
        let r = analyze_lines(&p).unwrap();
        let _ = std::fs::remove_file(&p);
        r.to_json()
    }

    #[test]
    fn amplitude_and_units() {
        // 线 1：2 秒内从 x=0 匀速到 +675（半屏）→ x ∈ [0,1]，amp 1.0，峰速 0.5 半宽/秒
        let obj = json!({
            "META": {}, "BPMList": [{"bpm": 120.0, "startTime": [0, 0, 1]}],
            "judgeLineList": [
                // ⚠️ 音符时间是**拍**：0~4 拍 @120BPM = 0~2 秒，
                //    采样网格必须覆盖事件的整段（0→+675 用 0~4 拍）
                {"notes": tap_notes(&(0..=8).map(|k| k as f64 * 0.5).collect::<Vec<_>>(), -675.0)},
                {"eventLayers": [{"moveXEvents": [
                     {"startTime": [0, 0, 1], "endTime": [4, 0, 1],
                      "start": 0.0, "end": 675.0, "easingType": 1}]}],
                 "notes": [{"type": 1, "above": 1, "positionX": 400.0, "isFake": 0,
                            "startTime": [0, 0, 1], "endTime": [0, 0, 1]}]}
            ]
        });
        let v = analyze_value(obj);
        assert_eq!(v["total_lines"], json!(2));
        assert_eq!(v["decision"]["playing_lines"], json!(2));
        let l1 = &v["lines"][1];
        assert_eq!(l1["x_amp"], json!(1.0), "{}", l1);
        assert_eq!(l1["x_min"], json!(0.0));
        assert_eq!(l1["x_max"], json!(1.0));
        // 0 → +1.0（半屏）用 2 秒 → 峰速 0.5 半宽/秒
        assert!((l1["peak_speed"].as_f64().unwrap() - 0.5).abs() < 0.01,
                "peak_speed = {}", l1["peak_speed"]);
        assert!((l1["mean_speed"].as_f64().unwrap() - 0.5).abs() < 0.01);
    }

    #[test]
    fn rotate_is_negated() {
        // 官方对 rotate 取负：+90° → -90°
        let obj = json!({
            "META": {}, "BPMList": [{"bpm": 120.0, "startTime": [0, 0, 1]}],
            "judgeLineList": [{"notes": tap_notes(&[0.0, 1.0, 2.0, 3.0, 4.0], 0.0),
                "eventLayers": [{"rotateEvents": [
                {"startTime": [0, 0, 1], "endTime": [1, 0, 1], "start": 0.0, "end": 90.0}]}]}]
        });
        let v = analyze_value(obj);
        assert_eq!(v["lines"][0]["rot_norm"], json!([-90.0, 0.0]), "{}", v["lines"][0]);
    }

    #[test]
    fn flip_detected() {
        // 平滑转到 180° 并停住 → 检出「反过来打」
        let obj = json!({
            "META": {}, "BPMList": [{"bpm": 120.0, "startTime": [0, 0, 1]}],
            "judgeLineList": [{"eventLayers": [{"rotateEvents": [
                {"startTime": [0, 0, 1], "endTime": [2, 0, 1], "start": 0.0, "end": 180.0},
                {"startTime": [2, 0, 1], "endTime": [10, 0, 1], "start": 180.0, "end": 180.0}]}],
            "notes": tap_notes(&(0..30).map(|k| k as f64 * 0.2).collect::<Vec<_>>(), 0.0)}]
        });
        let v = analyze_value(obj);
        assert_eq!(v["flip"]["any"], json!(true), "{}", v["flip"]);
        assert_eq!(v["flip"]["lines"][0]["flips"], json!(1));
        assert!(v["flip"]["lines"][0]["upside_down_pct"].as_f64().unwrap() > 80.0);
    }

    #[test]
    fn father_chain_composite() {
        // 父线常驻 +675，子线无自身位移 → 子线世界 x = +1.0
        let obj = json!({
            "META": {}, "BPMList": [{"bpm": 120.0, "startTime": [0, 0, 1]}],
            "judgeLineList": [
                {"father": -1, "eventLayers": [{"moveXEvents": [
                  {"startTime": [0, 0, 1], "endTime": [0, 0, 1], "start": 675.0, "end": 675.0}]}],
                 "notes": []},
                {"father": 0, "notes": [{"type": 1, "above": 1, "positionX": 0.0, "isFake": 0,
                                         "startTime": [0, 0, 1], "endTime": [0, 0, 1]}]}
            ]
        });
        let v = analyze_value(obj);
        let c = &v["lines"][1];
        assert_eq!(c["x_min"], json!(1.0), "{}", c);
        assert_eq!(c["x_max"], json!(1.0));
        assert_eq!(c["is_child"], json!(true));
        // 逐音符世界坐标也应为 +1.0
        assert_eq!(v["decision"]["playing_lines"], json!(1));
    }
}
