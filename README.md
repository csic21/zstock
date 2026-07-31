# Stock Analysis · A股（GPUI）

跨平台桌面 A 股分析原型：Zed 风格深色 UI + 东方财富免费公开行情 + MA/十字线 + 可拖拽多面板 + 本地自选持久化。

## 功能

| 模块 | 说明 |
|------|------|
| **真实 A 股数据** | 东财 → 新浪 → BaoStock（全免费、无 Key，自动切换）：日 K、批量行情、搜索 |
| **自选股** | 默认沪深龙头；可搜索添加 / 移除；约 8s 刷新报价 |
| **K 线 + 指标** | 前复权日 K；MA5 / MA10 / MA20；十字线悬停 OHLC |
| **多面板** | 左自选 / 中图表 / 底详情，可拖拽调整宽度高度 |
| **命令面板** | `⌘K` / `⌘P` 搜索代码或名称 |
| **涨跌配色** | 标题栏切换：中国·红涨 / 美国·绿涨（自选、涨跌幅、K 线同步） |
| **持久化** | 自选列表、选中标的、周期、MA 开关、涨跌色写入本地 JSON |

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

## 快捷键

| 快捷键 | 作用 |
|--------|------|
| `⌘K` / `Ctrl+K` | 命令面板（搜索 / 添加自选） |
| `⌘P` / `Ctrl+P` | 同上 |
| `⌘R` / `Ctrl+R` | 刷新行情 + 当前 K 线 |
| `⌘Q` / `Alt+F4` | 退出 |

## 项目结构

```
src/
  main.rs
  app.rs              # 主界面与数据调度
  chart.rs            # K 线 / MA / 十字线绘制
  model.rs            # Symbol / Candle
  storage.rs          # 本地 config.json
  data/
    market.rs         # 多源调度（东财 → 新浪 → BaoStock）
    eastmoney.rs      # 东财行情 / K 线 / 搜索
    sina.rs           # 新浪行情 / K 线（备用）
    baostock.rs       # BaoStock 日 K（前复权，TCP）
    indicators.rs     # SMA
docs/
  data-sources.md     # 免费数据源对比
```

## 配置文件位置

- macOS: `~/Library/Application Support/stock-analysis/config.json`
- Linux: `~/.local/share/stock-analysis/config.json`
- Windows: `%APPDATA%\stock-analysis\config.json`

## 数据说明与免责

- 行情来自**东方财富网页公开接口**，免费、无 SLA，可能变更或限流。
- 日 K 默认**前复权**；成交量字段按东财返回展示。
- **仅供学习研究，不构成任何投资建议。** 商业用途请使用有授权的数据服务。

## 下一步可做

- [x] 东财 + 新浪 + BaoStock 自动切换（全免费）  
- [ ] 通达信公服实时 / 更多免费源  
- [ ] 分时 / 分钟 K、成交量副图  
- [ ] 更多指标（MACD、BOLL）与画线  
- [ ] 完整 Dock 布局序列化（当前为 resizable 三栏）

## License

MIT
