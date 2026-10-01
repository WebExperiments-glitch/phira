# 规则引擎/ios —— Phira fork 工程说明（Windows 构建）

> 本目录是 **Phira 0.8.2 的 fork**（GPL-3.0-only，来源：https://github.com/TeamFlos/phira ）
> 目标：做成「谱面分析 + DeepSeek 云端分析 + Phira 同款试玩 + 规则引擎写谱+实时预览」四合一的
> 谱面工作台。**本项目也将以 GPL-3.0 开源**，符合上游许可证要求。

---

## 一、Windows 构建（已跑通，2026-10-01 验证）

### 1. 工具链
上游 pin 了 `nightly-2026-01-01`（见 `rust-toolchain.toml`），rustup 会自动拉取。
本机实测：`cargo 1.94.0-nightly` + Visual Studio 2022 (MSVC)。

### 2. FFmpeg 静态库（**必须预下载，否则链接失败**）
`prpr-avc/build.rs` 会从 GitHub Releases 下载预编译 FFmpeg 静态库：

```
https://github.com/TeamFlos/prpr-avc-ffmpeg/releases/download/20260730_v0/x86_64-pc-windows-msvc.tar.gz
```

国内直连 GitHub 会失败（本机实测是 **schannel 证书吊销检查失败**
`CRYPT_E_NO_REVOCATION_CHECK`，不一定是墙）。用镜像可用：

```bash
cd prpr-avc/static-lib
mkdir -p x86_64-pc-windows-msvc && cd x86_64-pc-windows-msvc
curl -L -o ff.tar.gz \
  "https://ghfast.top/https://github.com/TeamFlos/prpr-avc-ffmpeg/releases/download/20260730_v0/x86_64-pc-windows-msvc.tar.gz"
tar -xzf ff.tar.gz && rm ff.tar.gz
printf '20260730_v0\n' > .version      # cache_is_valid() 靠这个文件判断是否需要重新下载
```
解压后应有 5 个 `.a`：`libavcodec / libavformat / libavutil / libswresample / libswscale`。

### 3. 本 fork 对上游代码的改动
| 文件 | 改动 | 原因 |
|---|---|---|
| `prpr-avc/build.rs` | Windows 下**不再 `rustc-link-lib=z`** | 上游无条件链接 `z`（Linux 下 zlib 叫 libz），**MSVC 下没有 `z.lib`** → `LNK1181`。实测 FFmpeg 静态库里 `inflate`/`deflate`/`zlibVersion` 引用数均为 0，不需要 zlib |

### 4. 缺失资源（上游 .gitignore 掉了，需自行补）
`.gitignore` 忽略了若干资源，其中两个**启动必需**：

| 文件 | 说明 | 我们的处理 |
|---|---|---|
| `assets/font.ttf` | 主 UI 字体，需覆盖 CJK（`phira/src/lib.rs:227`） | 用系统 `C:\Windows\Fonts\Deng.ttf`（等线，静态 TTF）。**待替换成与官方一致的字体** |
| `assets/background.jpg` | 默认背景图 | 生成了一张 1920×1080 深色对角渐变 |

其余被忽略的（`assets/res`、`chart.zip`、`avatar`、`flavor`、`../icon/*`）均为
**运行时生成/上传的名字或可选资源**，不缺。

### 5. 构建与运行
```bash
cd 规则引擎/ios
cargo build --release -p phira-main      # 首次约 15 分钟（~380 个依赖），之后增量 1~2 分钟
./target/release/phira-main.exe          # 必须在项目根目录运行（资源路径是相对的 assets/）
```
产物：`target/release/phira-main.exe`（约 32 MB，静态链接）。

---

## 二、导入自定义谱面

**机制**：`data/charts/custom/` 是**启动时扫描**的目录（`phira/src/data.rs:195-230`），
对每个条目调 `prpr::fs::fs_from_file` + `load_info`。

⚠️ **必须放「解压后的目录」，不能直接放 `.pez` 文件。**
虽然 `fs_from_file` 对文件走 `ZipFileSystem`、解析也成功（会登记进 `data/data.json`），
但后续 `scene.rs:313` / `song.rs:191` 等路径用 `prpr::dir::Dir::new(charts/{local_path})`
**假设它是目录** → 抛 `not dir`（实测每个 .pez 弹一个错误框）。

正确做法：
```bash
# 把 .pez 解压成目录放进 data/charts/custom/<名字>/
python -c "import zipfile; zipfile.ZipFile('output/s3c_54467.pez').extractall('规则引擎/ios/data/charts/custom/s3_54467')"
```
目录内需有 `info.yml` + `chart.json`（+ `song.mp3` / `bg.png`）。

**已验证**：我们生成的 .pez 被 Phira 完整解析、登记、显示、并**可实际游玩**
（判定线/下落音符/分数/进度条全部正常）。

---

## 三、已修复的生成质量问题

### 动作类型分布崩坏（80% 是 hold）
**现象**：s3_54467 共 895 音符，其中 **713 个是 hold（80%）**，而人类谱 hold 只占 4.6%。
视觉上就是满屏长条。

**根因**：ACTION 词表按「类型 × x 桶」展开，但 **hold 额外带 6 个时长桶**：
```
tap 17 / drag 17 / flick 17 / hold 17×6 = 102
```
→ **hold 的 token 质量天然是其他类型的 6 倍**，直接按 token 做 softmax 采样必然偏斜。

**修复**（`tools/generate_chart.py` 新增 `_sample_action()`）：
把采样拆成两步 —— ① 按**语义类型先验**抽类型 ② 在该类型的 token 分布内抽具体 x 桶。
先验默认用人类谱统计 `HUMAN_TYPE_PRIOR = (tap 82.8, drag 8.7, flick 4.0, hold 4.6)`。

**修复前后对比**：
| 类型 | 修复前 | 修复后 | 人类参考 |
|---|---|---|---|
| tap | 8.3% | **84.2%** | 82.8% |
| drag | 4.8% | **7.3%** | 8.7% |
| hold | **79.7%** | **5.2%** | 4.6% |
| flick | 7.3% | **3.3%** | 4.0% |

---

## 四、待办
- [ ] `assets/font.ttf` 换成与官方一致的字体
- [ ] Phase 2：谱面分析（难度 / 难点定位）
- [ ] Phase 3：规则引擎写谱 + 实时预览（复用 `../tools/rule_chart_generator.py`）
- [ ] Phase 4：DeepSeek API 接入
- [ ] Phase 5：iOS 目标（**需要 macOS + Xcode，本机 Windows 无法编译验证**）
