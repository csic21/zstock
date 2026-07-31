# Stock Analysis · A股（GPUI）

跨平台桌面 A 股分析原型：Zed 风格深色 UI + 东方财富免费公开行情 + MA/十字线 + 可拖拽多面板 + 本地自选持久化。

## 功能

| 模块 | 说明 |
|------|------|
| **真实 A 股数据** | 东财 → 腾讯（全免费、无 Key，自动切换）：日 K、批量行情、搜索 |
| **自选股** | 默认沪深龙头；可搜索添加 / 移除；约 8s 刷新报价 |
| **🐭 寻宝鼠** | 多窗口历史低位扫描（1Y / 3Y / 全样本）；上行中继回撤降权；结果缓存 |
| **K 线 + 指标** | 前复权日 K；MA5 / MA10 / MA20；周期 1M–MAX；十字线悬停 OHLC |
| **多面板** | 左自选/寻宝 / 中图表 / 底详情，可拖拽调整宽度高度 |
| **命令面板** | `⌘K` / `⌘P` 搜索代码或名称 |
| **涨跌配色** | 标题栏切换：中国·红涨 / 美国·绿涨（自选、涨跌幅、K 线同步） |
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
| `⌘R` / `Ctrl+R` | 刷新行情 + 当前 K 线 |
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
- [ ] 分时 / 分钟 K（可走腾讯 `minute/query`）、成交量副图  
- [ ] 更多指标（MACD、BOLL）与画线  
- [ ] 完整 Dock 布局序列化（当前为 resizable 三栏）  
- [ ] 寻宝：指数成分动态拉取 / 财务分位过滤

## License

MIT
