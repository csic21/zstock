# ZStock · A股 / 港股（GPUI）

跨平台桌面 A 股 + 港股分析原型：Zed 风格深色 UI + 东财/腾讯免费公开行情 + MA/十字线 + 可拖拽多面板 + 本地自选持久化。

## 功能

| 模块 | 说明 |
|------|------|
| **真实行情** | A 股 + 港股：东财 → 腾讯（全免费、无 Key，自动切换）；港股代码 5 位如 `00700` |
| **自选股** | 默认沪深龙头；可搜索添加港股；**仅交易时段轮询**（A 含竞价 09:15 起；港 09:00–16:10）；盘外启动只拉一次 |
| **持仓** | 买入/卖出/清仓流水；平均成本与浮动盈亏；可选现金约束；AI 结合成本+现价+技术面给出买卖观察建议；本地 `portfolio.json` |
| **菜单栏行情** | macOS：设置里从自选固定最多 5 只，**多只同时**显示现价与涨跌；**未固定时显示 S logo**；行情轮询实时刷新；下拉打开对应标的 / 唤起主窗口 |
| **设置** | 标题栏 / `⌘,`：全页设置（常规 · 菜单栏 · AI · 更新 · 关于），不再用弹窗；写入本地 config |
| **🐭 寻宝鼠（搜罗）** | ①扫低位 → ②**自动筛可买**（Top20 深评）→ 默认只看「可关注」与建仓价；完整寻宝榜折叠；可选 LLM 整榜摘要；上行中继/ST 降权 |
| **K 线 + 指标** | 前复权日 K；MA5/10/20/60 + 成交量 / **MACD 副图** + **BOLL 通道**；周期 1M–MAX；十字线悬停 OHLC（含 MACD/BOLL 数值）；双击重置缩放 |
| **画线** | 工具栏「画线」进入绘制模式：拖拽画趋势线、单击画水平价格线；按标的持久化，支持清除与多色循环 |
| **分时 / 分钟 K** | 分时（价格 + 均价 + 昨收基准线 + 分笔量，腾讯 `minute/query`，约 5s 自动刷新）；分钟 K（1/5/15/30/60 分，`mkline`，可缩放/平移/叠加 MA） |
| **策略雷达** | 联合 MA20/60、RSI14、20 日动量、年化波动、1Y 最大回撤和量能确认，输出可解释强弱分数与数据置信度 |
| **AI 点评** | 一键生成中文分析（趋势/动量/量能/位置/风险 + **参考建仓/减仓元价位**）：默认本地规则即时出结果；可选 **API**（OpenAI 兼容 Responses/Chat）或 **CLI**（Grok / ChatGPT·Codex / OpenCode / Claude），只上传指标快照，失败自动回退本地 |
| **多面板** | 左自选/寻宝 / 中图表 / **底部分区分析台**（概览 · 策略 · AI · 寻宝 · 指标 Tab，避免信息横向堆叠），可拖拽调整宽高；**完整 Dock 布局序列化**（面板尺寸 + 窗口位置/大小写入 config，重启恢复） |
| **命令面板** | `⌘K` / `⌘P` 搜索代码或名称 |
| **涨跌配色** | 标题栏切换：中国·红涨 / 美国·绿涨（自选、涨跌幅、K 线同步） |
| **工作模式** | 标题栏 / `⌘⇧W`：整页服务监控台 + 迷你 spark；`p50` 对应现价，`drift` 对应涨跌幅，右侧 `cpu/mem/disk` 对应上证/沪深300/创业板真实点位；点 `Map` 临时显示股票和指数身份，点 `Hide` 一键恢复伪装；窗口标题 Notes |
| **持久化** | 自选、偏好、寻宝结果、持仓流水写入本地 JSON |

数据源调研见 [docs/data-sources.md](docs/data-sources.md)。

## 运行

```bash
cargo run
```

Release：

```bash
cargo run --release
```

离线单元测试（联网数据源 smoke tests 默认忽略）：

```bash
cargo test
# 手动验证公网行情接口
cargo test -- --ignored
```

### macOS

依赖 GPUI `runtime_shaders`（避免本机缺少 Metal Toolchain）。若已安装：

```bash
xcodebuild -downloadComponent MetalToolchain
```

可去掉 `Cargo.toml` 里的 `runtime_shaders` feature。

### Linux

依赖系统库：`libxkbcommon`、`libwayland`、`libx11`、`libxcb-*`、`libfontconfig`、
`libfreetype`（Debian/Ubuntu 用 `-dev` 后缀，例如 `sudo apt install libxkbcommon-dev
libwayland-dev libx11-dev libxcb1-dev libxcb-render0-dev libxcb-shape0-dev
libxcb-xfixes0-dev libxkbcommon-x11-dev libfontconfig1-dev libfreetype6-dev pkg-config`）。

```bash
cargo run --release
```

运行时需要 Vulkan 驱动（如 `mesa-vulkan-drivers`）或软件渲染（lavapipe）。

### 打包 macOS `.app`（带 S logo）

```bash
./scripts/package-macos.sh
open "dist/ZStock.app"
```

### 打包 macOS 安装包（DMG / PKG）

用户安装建议直接用安装包，两个脚本都会先调用 `package-macos.sh` 生成最新的
`ZStock.app`：

```bash
# .pkg 安装器：双击后走安装向导，自动装进「应用程序」，无需拖拽
./scripts/package-pkg.sh            # 默认当前架构（arm64 / x64）
./scripts/package-pkg.sh arm64      # 指定架构

# .dmg 安装包：经典拖拽到「应用程序」的体验
./scripts/package-dmg.sh
./scripts/package-dmg.sh x64
```

产物在 `dist/` 下：`zstock-macos-<arch>.pkg` 与
`zstock-macos-<arch>.dmg`。

图标资源在 `assets/logo/`（见下方）。

### macOS：避免 Launchpad / Spotlight 出现两个 ZStock

`dist/ZStock.app` 与装进 `/Applications` 的正式版是**两份独立的 `.app`**。
macOS 会把磁盘上找到的应用都编进索引，所以在已安装正式版的同时保留
`dist/` 里的构建产物时，搜索 `zstock` 会看到两个同名图标（不是程序 bug）。

建议习惯：

1. **日常使用**只打开 `/Applications/ZStock.app`（或通过 `.pkg` / `.dmg` 安装）。
2. **本地试跑**可用 `open "dist/ZStock.app"`，测完后清理产物，避免被索引：
   ```bash
   rm -rf dist/*
   ```
3. 若只想去掉应用包、保留安装器：
   ```bash
   rm -rf dist/*.app
   ```
4. 仓库里若还残留旧名 `Stock Analysis.app`，同样会被索引，可一并删掉。

清理后若 Launchpad 仍短暂显示旧结果，稍等索引刷新或重新打开搜索即可。

### 安装 Linux（桌面集成）

Linux 安装包内含二进制、`.desktop` 入口、hicolor 图标和一键安装脚本：

```bash
tar -xzf zstock-linux-x64.zip
cd zstock-linux-x64
./install.sh              # 安装到 ~/.local，注册应用菜单与图标
./install.sh --uninstall  # 卸载（保留应用配置）
```

### 安装 Windows

直接运行 `zstock-windows-x64-setup.exe`（Inno Setup 安装器，按用户安装无需
管理员权限，自动创建开始菜单快捷方式与卸载入口；桌面快捷方式为可选）。
安装后的自动更新仍走 zip 包就地替换 `stock.exe`。

## 自动发布（GitHub Actions）

推送 `v*` tag 会触发 [release.yml](.github/workflows/release.yml) 自动打包并创建 GitHub Release：

```bash
# 发布前先把 Cargo.toml 的 version 改成目标版本（与 tag 一致，workflow 会校验）
git tag v0.0.1
git push origin v0.0.1
```

macOS 的 `Info.plist` 版本会在打包时根据 tag 自动同步。

产物：

| 平台 | 文件 | 说明 |
|------|------|------|
| macOS（Apple Silicon） | `zstock-macos-arm64.zip` / `.dmg` / `.pkg` | `ZStock.app`（arm64）；`.pkg` 免拖拽安装，`.dmg` 拖拽安装 |
| macOS（Intel） | `zstock-macos-x64.zip` / `.dmg` / `.pkg` | `ZStock.app`（x86_64）；同上 |
| Windows | `zstock-windows-x64.zip` | `stock.exe` + README；另产出 `zstock-windows-x64-setup.exe`（Inno Setup 安装器） |
| Linux | `zstock-linux-x64.zip` | `stock` + README + `.desktop`/图标/`install.sh`（安装到 `~/.local`） |

说明：

- 未做正式签名：macOS 首次打开安装包或 App 需右键「打开」（或先
  `xattr -dr com.apple.quarantine`），Windows 可能提示 SmartScreen。
- `.pkg` 安装器内置 preinstall 脚本，会先移除旧版本（含更名前的
  `Stock Analysis.app`）再写入新版本。
- `.zip` 仍保留给应用内自动更新使用（更新时只同步 `.app` 内容，不重装）。
- 在 Actions 页面手动运行该 workflow 只上传构建产物（artifacts），不会创建 Release。
- 每次 tag 发布后会自动生成 `updates/stable.json`（Zed 风格更新清单，含各平台
  SHA-256）并推送到 `main`，供客户端静默检查更新。

## 自动更新

更新检查参考 Zed：客户端**不调用 GitHub API**，而是轮询一个静态更新清单
`updates/stable.json`（每次发版由 release workflow 自动生成并推送到 `main`）。
清单只有极小的一段 JSON：版本号、各平台下载直链和 SHA-256。清单源依次尝试
raw.githubusercontent 与 jsDelivr CDN（国内网络 raw 常被代理/拦截，CDN 兜底）。

- 有新版本时，标题栏出现「更新 vX」按钮（设置面板里也有「检查更新 / 立即更新」）。
- 点击后从 GitHub Releases 直链下载当前平台（macOS arm64/x64、Windows x64 或
  Linux x64）的安装包，**校验 SHA-256** 后安装并重启应用（macOS 用 rsync
  就地同步 `.app` 内容，避免整体替换目录；Windows/Linux 直接替换二进制）。
- 检查时机：启动后 + 每 4 小时一次；设置面板可手动触发。
- 自动检查失败保持静默（离线 / 清单暂缺不打扰用户），手动检查会显示错误。
- 版本比较基于清单里的 semver 版本，需要与 release workflow 的产物命名一致。

注意：清单与安装包均走 GitHub 静态地址，**仓库需设为 public** 客户端才能匿名访问
（private 仓库的 raw 文件与安装包下载都需要登录）。

## Logo

主识别为几何双线 **S**（Stock），Zed 风格深色圆角 + 银白线。

| 文件 | 用途 |
|------|------|
| `assets/logo/app-icon.png` | 深色 App 图标 |
| `assets/logo/app-icon-light.png` | 浅色 App 图标 |
| `assets/logo/AppIcon.icns` | macOS 打包用 |
| `assets/logo/mark-s.png` | 深色纯 S mark |
| `assets/logo/mark-s-light.png` | 浅色纯 S mark |
| `assets/logo/drafts/` | 历史草案 |

## 快捷键

| 快捷键 | 作用 |
|--------|------|
| `⌘K` / `Ctrl+K` | 命令面板（搜索 / 添加自选） |
| `⌘P` / `Ctrl+P` | 同上 |
| `⌘T` / `Ctrl+T` | 切换左侧「自选 / 寻宝鼠」 |
| `⌘⇧W` / `Ctrl+Shift+W` | 工作模式（中性配色与文案，标题变 Notes） |
| `⌘,` / `Ctrl+,` | 设置（刷新间隔、涨跌色、工作模式） |
| `⌘R` / `Ctrl+R` | 刷新行情 + 当前 K 线 |
| `↑` / `↓` 或 `k` / `j` | 自选上一只 / 下一只 |
| `Esc` | 关闭命令面板 / 设置 |
| `0` 或图表双击 | 重置 K 线缩放 |
| `⌘Q` / `Alt+F4` | 退出 |

## 项目结构

```
src/
  main.rs
  app.rs              # 主界面与数据调度
  chart.rs            # K 线 / MA / 十字线绘制
  model.rs            # Symbol / Candle
  storage.rs          # config.json + treasure_cache.json
  data/
    ai.rs             # AI 点评：本地规则生成 + Responses/Chat 双协议 LLM
    market.rs         # 多源调度（东财 → 腾讯）
    eastmoney.rs      # 东财行情 / K 线 / 搜索 / 榜单
    tencent.rs        # 腾讯行情 / K 线 / SmartBox（备用）
    indicators.rs     # SMA
    sina.rs           # 新浪指数成分（含 PE/PB）
    treasure.rs       # 寻宝鼠多窗口评分
    universe.rs       # 扫描候选池
docs/
  data-sources.md     # 免费数据源对比
assets/
  logo/               # App 图标 / S mark / .icns
  macos/Info.plist    # .app 清单
scripts/
  package-macos.sh    # 打 macOS .app
```

## 配置文件位置

- macOS: `~/Library/Application Support/stock-analysis/`
- Linux: `~/.local/share/stock-analysis/`
- Windows: `%APPDATA%\stock-analysis\`

其中 `config.json` 为自选与 UI 偏好，`treasure_cache.json` 为寻宝鼠最近一次扫描结果。

## AI 点评

底部详情栏的「AI 点评」为当前标的生成一段结构化中文分析（综合 / 趋势 / 动量 /
量能 / 位置 / 风险），按 `代码 + 最后K线日期` 缓存。

- **本地模式（默认）**：纯本地规则生成，离线可用、即时返回，无需任何配置。
- **LLM 模式（可选）**：设置 → AI 分析 中开启，**调用方式**可选：
  - **API**：填写 API 地址 / 模型 / Key，支持 OpenAI **Responses** 与
    **Chat Completions**（DeepSeek、通义等兼容服务把协议切成 `Chat` 即可）。
  - **CLI**：使用本机已登录的代理 CLI，支持 **Grok**（`grok`）、
    **ChatGPT**（`chatgpt`，自动回退 `codex`）、**OpenCode**（`opencode`）、
    **Claude**（`claude`）。模型与 CLI 路径可选；路径留空则在 PATH 与常见安装目录中查找。
  请求失败或未配置时自动回退本地点评。
- **来源标注**：每条点评上方标注来源（`本地规则`、`LLM · 模型名` 或
  `CLI · Grok` 等），缓存命中时也会保留原来源；LLM 失败回退时同时显示失败原因。
- **隐私与成本**：只把本地算好的指标快照（JSON）发给模型 / CLI，不上传原始 K 线与
  行情；API Key 仅保存在本机 `config.json`；CLI 模式复用本机登录态。
- 输出均为统计/生成结果，**不构成任何投资建议**。

## 数据说明与免责

- 行情主源**东方财富**，备源**腾讯财经**（公开接口，免费、无 SLA，可能变更或限流）。
- 日 K 默认**前复权**；腾讯备源日 K 约 640 根上限。
- 分时 / 分钟 K 走**腾讯财经**（`minute/query` / `kline/mkline`）：分时当日约 240–267 点；分钟 K 单次上限 m1≈320 根、m5–m60≈800 根。
- **仅供学习研究，不构成任何投资建议。** 商业用途请使用有授权的数据服务。

## 合规声明

本应用定位为**个人学习研究工具**，请遵守以下边界：

- **数据来源**：东方财富 / 腾讯财经公开接口，未取得任何授权协议；接口可能随时变更或限流。请勿将抓取的数据用于商业用途、对外再分发或二次加工后提供服务。
- **不构成投资建议**：应用内的指标、评分、标签均为统计数据展示，不代表任何买入 / 卖出 / 持有建议；不提供荐股服务。请勿以此为依据进行证券交易决策。
- **不作交易执行**：本应用不接任何券商交易接口，不提供自动下单能力。
- **个人信息**：本应用不采集、不上传任何个人信息，所有数据仅保存在本地。
- **开源合规**：MIT 许可；依赖组件版权归原作者所有，分发二进制时请保留第三方许可证声明。

## 寻宝鼠说明

- **为何不只看 1 年**：上行趋势里，近一年回撤可能只是中继，3 年/全样本仍在高位 → 标签 **「上行中继回撤」** 并降权。
- **评分窗口**：1Y（~252 日）+ 3Y（~750 日）+ 全样本（最多约 1000 日前复权）；长窗口权重大于短窗口。
- **宇宙（扩大搜索）**：东财 clist 按**总市值**取约 400 只沪深 A（过滤 ST/过小市值）∪ 自选；失败则回落内置龙头表。深评后只保留 **Top 100**。
- **数据**：仅前复权源（东财 → 腾讯）。扫描约 1 分钟量级，可取消。
- **用法**：左侧「🐭 寻宝」→「开始搜罗」→ 扫完**自动「AI 筛可买」**；顶部出现「可买观察」清单（代码、结论、建仓/减仓带、可买分），无需一只只点开。也可对已有缓存榜手动点「AI 筛可买」。
- **筛可买逻辑**：寻宝分 + 多年低位/双低/深回撤加分；上行中继、ST、流动性弱、RSI 过热等减分；现价贴近建仓带加分。本地规则决定排序与入选；开启 AI 时另生成整榜中文摘要（失败回退本地）。
- **参考价位**：近 20/60 日高低点、MA20/60、ATR14。**仅供学习研究，不构成投资建议。**

## 下一步可做

- [x] 东财 + 腾讯自动切换（全免费，两家够用）  
- [x] 寻宝鼠多窗口历史低位扫描  
- [x] 成交量副图、MA60、布局宽高持久化、自选排序 / 键盘切换
- [x] 分时 / 分钟 K（腾讯 `minute/query` + `mkline`）
- [x] 更多指标（MACD、BOLL）与画线（趋势线 / 水平线，按标的持久化）
- [x] 完整 Dock 布局序列化（三栏全部尺寸 + 窗口位置/大小）
- [x] 寻宝：指数成分动态拉取（沪深300 / 中证500 / 上证50 / 创业板指 / 科创50）与财务分位过滤（PE / PB）

## License

MIT
