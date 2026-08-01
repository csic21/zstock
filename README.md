# Stock Analysis · A股（GPUI）

跨平台桌面 A 股分析原型：Zed 风格深色 UI + 东方财富免费公开行情 + MA/十字线 + 可拖拽多面板 + 本地自选持久化。

## 功能

| 模块 | 说明 |
|------|------|
| **真实 A 股数据** | 东财 → 腾讯（全免费、无 Key，自动切换）：日 K、批量行情、搜索 |
| **自选股** | 默认沪深龙头；可搜索添加 / 移除；刷新间隔可在设置里调（默认 1s） |
| **设置** | 标题栏 / `⌘,`：行情间隔、涨跌色、工作模式；写入本地 config |
| **🐭 寻宝鼠** | 多窗口历史低位扫描（1Y / 3Y / 全样本）；上行中继回撤降权；结果缓存 |
| **K 线 + 指标** | 前复权日 K；MA5/10/20/60 + 成交量副图；周期 1M–MAX；十字线悬停 OHLC；双击重置缩放 |
| **分时 / 分钟 K** | 分时（价格 + 均价 + 昨收基准线 + 分笔量，腾讯 `minute/query`，约 5s 自动刷新）；分钟 K（1/5/15/30/60 分，`mkline`，可缩放/平移/叠加 MA） |
| **策略雷达** | 联合 MA20/60、RSI14、20 日动量、年化波动、1Y 最大回撤和量能确认，输出可解释强弱分数与数据置信度 |
| **多面板** | 左自选/寻宝 / 中图表 / 底详情，可拖拽调整宽度高度 |
| **命令面板** | `⌘K` / `⌘P` 搜索代码或名称 |
| **涨跌配色** | 标题栏切换：中国·红涨 / 美国·绿涨（自选、涨跌幅、K 线同步） |
| **工作模式** | 标题栏 / `⌘⇧W`：整页服务监控台 + 迷你 spark；`p50` 对应现价，`drift` 对应涨跌幅，右侧 `cpu/mem/disk` 对应上证/沪深300/创业板真实点位；点 `Map` 临时显示股票和指数身份，点 `Hide` 一键恢复伪装；窗口标题 Notes |
| **持久化** | 自选、偏好、寻宝结果写入本地 JSON |

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

### 打包 macOS `.app`（带 S logo）

```bash
./scripts/package-macos.sh
open "dist/Stock Analysis.app"
```

图标资源在 `assets/logo/`（见下方）。

## 自动发布（GitHub Actions）

推送 `v*` tag 会触发 [release.yml](.github/workflows/release.yml) 自动打包并创建 GitHub Release：

```bash
git tag v0.0.1
git push origin v0.0.1
```

产物：

| 平台 | 文件 | 说明 |
|------|------|------|
| macOS（Apple Silicon） | `stock-analysis-macos-arm64.zip` | `Stock Analysis.app`（arm64） |
| macOS（Intel） | `stock-analysis-macos-x64.zip` | `Stock Analysis.app`（x86_64） |
| Windows | `stock-analysis-windows-x64.zip` | `stock.exe` + README |

说明：

- 未做正式签名：macOS 首次打开需右键「打开」，Windows 可能提示 SmartScreen。
- 在 Actions 页面手动运行该 workflow 只上传构建产物（artifacts），不会创建 Release。

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
    market.rs         # 多源调度（东财 → 腾讯）
    eastmoney.rs      # 东财行情 / K 线 / 搜索 / 榜单
    tencent.rs        # 腾讯行情 / K 线 / SmartBox（备用）
    indicators.rs     # SMA
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

## 数据说明与免责

- 行情主源**东方财富**，备源**腾讯财经**（公开接口，免费、无 SLA，可能变更或限流）。
- 日 K 默认**前复权**；腾讯备源日 K 约 640 根上限。
- 分时 / 分钟 K 走**腾讯财经**（`minute/query` / `kline/mkline`）：分时当日约 240–267 点；分钟 K 单次上限 m1≈320 根、m5–m60≈800 根。
- **仅供学习研究，不构成任何投资建议。** 商业用途请使用有授权的数据服务。

## 寻宝鼠说明

- **为何不只看 1 年**：上行趋势里，近一年回撤可能只是中继，3 年/全样本仍在高位 → 标签 **「上行中继回撤」** 并降权。
- **评分窗口**：1Y（~252 日）+ 3Y（~750 日）+ 全样本（最多约 1000 日前复权）；长窗口权重大于短窗口。
- **宇宙（扩大搜索）**：东财 clist 按**总市值**取约 400 只沪深 A（过滤 ST/过小市值）∪ 自选；失败则回落内置龙头表。深评后只保留 **Top 100**。
- **数据**：仅前复权源（东财 → 腾讯）。扫描约 1 分钟量级，可取消。
- **用法**：左侧「🐭 寻宝」→「开始寻宝」→ 点选查看 3Y K 线；底栏展示分项位置/分位/标签。

## 下一步可做

- [x] 东财 + 腾讯自动切换（全免费，两家够用）  
- [x] 寻宝鼠多窗口历史低位扫描  
- [x] 成交量副图、MA60、布局宽高持久化、自选排序 / 键盘切换
- [x] 分时 / 分钟 K（腾讯 `minute/query` + `mkline`）
- [ ] 更多指标（MACD、BOLL）与画线  
- [ ] 完整 Dock 布局序列化（当前为 resizable 三栏宽高已持久化）
- [ ] 寻宝：指数成分动态拉取 / 财务分位过滤

## License

MIT
