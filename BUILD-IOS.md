# iOS 构建指南（chart-core 移植层）

> 目标：让 `chart-core`（谱面解析 + 判定线分析，Rust）跑在 iOS 上，
> 并最终随 phira 一起出 IPA。
>
> 当前状态（2026-10-01）：
> ✅ chart-core 已迁入本仓库 `chart-core/`（phira workspace 的 path 依赖）
> ✅ iOS 目标（aarch64-apple-ios）编译检查通过（Windows 交叉编译，纯 Rust 部分）
> ✅ iOS 启动自检已接入（`phira/src/lib.rs` → `chart_core_selftest()`，`cfg(target_os="ios")`）
> ⏳ 最终 IPA 构建需要 macOS（GitHub Actions workflow 已写好：`.github/workflows/ios-build.yml`）

---

## 架构一览

```
phira 仓库（GitHub CI 可完整构建）
├─ chart-core/            ← ★ 移植层（谱面解析 + 判定线分析，纯 Rust，无 C 依赖）
│    └─ src/{parse,lines,time,pyconv,note,info}.rs
├─ phira/                 ← 主 crate（已依赖 chart-core，iOS 启动自检在此）
├─ prpr/ phira-main/ …    ← phira 本体
└─ phira.xcodeproj/       ← iOS 壳工程（build 阶段跑 cargo，见下）

规则引擎/ios/rust/          ← Windows 上的开发/验收工程（独立 package）
├─ golden_check/           ← 黄金夹具校验器（依赖 ../../../phira/chart-core）
└─ chart_ffi/              ← C ABI（Windows DLL 已验证；iOS staticlib 待 CI 产出）
```

**Xcode 工程的构建机制**（`phira.xcodeproj/project.pbxproj`）：
`PLATFORM_NAME=iphoneos → RUST_TARGET=aarch64-apple-ios → cargo build --bin phira-main`
—— 所以 chart-core 作为 phira-main 的依赖会**自动**一起编译进 iOS App，无需改 Xcode 工程。

---

## 为什么 chart-core 迁进 phira 仓库

- **CI 可构建**：GitHub Actions 只拉这一个仓库，跨仓库的相对 path 依赖在 CI 上不存在。
- **纯 Rust 无 C 依赖**：chart-core 交叉编译零障碍（Windows 上 `cargo check
  --target aarch64-apple-ios` 直接通过）；phira-main 的 C 依赖才需要 macOS。
- **单一权威**：Windows 开发工程（`规则引擎/ios/rust/golden_check`、`chart_ffi`）通过
  `../../../phira/chart-core` 相对路径引用同一份代码，不存在两份要同步。

---

## 三条构建路径

### 路径 ① GitHub Actions（推荐，无需 Mac）

1. 把本仓库推到 GitHub（如 `WebExperiments-glitch/phira`）
2. push 或手动触发 `ios-build.yml` workflow
3. 下载 artifact `phira-ios-unsigned.ipa`（未签名）
4. 本地签名侧载：
   - Sideloadly 0.60 + Local Anisette（现有流程）
   - 或 SideStore / 爱思助手
5. 装上后启动 App，看日志验证：
   `[chart-core] selftest: 解析 ✅ … ✅ chart_core 在 iOS 上工作正常`

### 路径 ② 本地 Xcode（需要 Mac）

```bash
cargo build --release --bin phira-main --target aarch64-apple-ios
open phira.xcodeproj     # 选 phira scheme，连 iPad 直接跑
```

### 路径 ③ 只验证 chart-core（Windows 上就能做）

```bash
cd 规则引擎/phira
cargo check --target aarch64-apple-ios -p chart_core     # ✅ 已通过
```

---

## iOS 启动自检（已接入）

`phira/src/lib.rs` → `quad_main()` 开头（`cfg(target_os="ios")`，其他平台零影响）：

1. 在内存合成一张迷你 RPE（1 条线 + 20 音符 + 旋转 0→90°）
2. 写临时文件 → `chart_core::parse_chart_file` 解析
3. `chart_core::lines::analyze_lines` 做判定线分析
4. 日志输出：格式 / 实音符数 / 判定线数 / 旋转范围（期望含 **-90**，官方对 rotate 取负）
5. 失败只记日志，**绝不影响 App 启动**

这条日志就是「chart_core 在 iOS 上真实运行」的实证。

---

## 待办

| 项 | 说明 |
|---|---|
| S3/S2/S4/S6 移植 | 难度指标 / 基础统计 / 质量审计 / 组装（纯数值，可位级一致，见 PORT-SPEC） |
| iOS staticlib | CI 顺手产出 `libchart_ffi.a`，供将来 Swift 直接调用（chart_ffi 的 C ABI 已就绪） |
| flip 判定 | 以 `upside_down_pct` 为准（flips 受缓动近似影响，见 PORT-SPEC §4.4） |
| 推送 GitHub | phira 仓库当前无 remote；推送后 `ios-build.yml` 自动生效 |
