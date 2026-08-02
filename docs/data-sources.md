# A 股免费数据源调研（2026）

面向本项目（个人桌面分析工具）的数据源选择说明。

## 结论（我们采用什么）

**当前实现：东财主源 → 腾讯备用（均免费、无需注册 / API Key）**

| 用途 | 主源 | 备用 |
|------|------|------|
| 批量行情 | 东财 `ulist` | 腾讯 `qt.gtimg.cn` |
| 历史日 K（前复权） | 东财 `fqt=1` | 腾讯 `newfqkline`（`qfq`，约 640 根上限；旧 `fqkline` 再兜底） |
| 分时 / 分钟 K | —（暂未接入） | 腾讯 `minute/query` + `kline/mkline` |
| 代码搜索 | 东财 suggest | 腾讯 SmartBox |
| 宇宙榜单（寻宝池） | 东财 `clist` | —（失败回落内置池） |

调度逻辑在 `src/data/market.rs`。  
状态栏会显示本次实际命中的源（如「K线 · 腾讯财经」）。

**为何只两家**

- 本项目只做 **A 股**（行情 / 日 K / 搜索 / 寻宝），不需要港美、ETF 持仓、多年回测专用链。
- 东财覆盖主路径；腾讯 HTTP 全备（行情 + 前复权 K + 搜索），不必再叠新浪（K 不复权）或 BaoStock（TCP 会话）。
- 腾讯日 K 约 **640 根**上限；寻宝鼠优先东财长窗（~1000），东财失败时腾讯短窗仍可用。

优点：零成本、厂商少、备源也是前复权。  
缺点：非官方 SLA；腾讯 K 窗短于东财。  
限频：本应用约 1s 刷一次行情。

> 仅供学习研究，不构成投资建议；商业产品请换有授权的数据商。

---

## 接口一览

### 东方财富

| 用途 | 接口 |
|------|------|
| 批量行情 | `https://push2.eastmoney.com/api/qt/ulist.np/get` |
| 日 K | `https://push2his.eastmoney.com/api/qt/stock/kline/get`（`klt=101&fqt=1`） |
| 搜索 | `https://searchapi.eastmoney.com/api/suggest/get` |
| 榜单 | `https://push2.eastmoney.com/api/qt/clist/get` |

### 腾讯财经

| 用途 | 接口 |
|------|------|
| 批量行情 | `https://qt.gtimg.cn/q=sh600519,sz000001`（`~` 分隔，GBK） |
| 日 K | `https://web.ifzq.gtimg.cn/appstock/app/newfqkline/get?param={sym},day,,,{n},qfq` |
| 日 K 兜底 | `https://web.ifzq.gtimg.cn/appstock/app/fqkline/get?param=…` |
| 分时 | `https://web.ifzq.gtimg.cn/appstock/app/minute/query?code={sym}` |
| 分钟 K | `https://ifzq.gtimg.cn/appstock/app/kline/mkline?param={sym},m1|m5|m15|m30|m60,,{n}` |
| 搜索 | `https://smartbox.gtimg.cn/s3/?q=关键词&t=all` |

实现：`eastmoney.rs`、`tencent.rs`；调度：`market.rs`。

### 腾讯分时 / 分钟 K 说明

- **分时**：`minute/query` 返回当日每分钟 `HHMM price cum_volume cum_amount`（累计手数 / 累计成交额），
  由 `cum_amount / (cum_volume×100)` 客户端计算均价线；昨收取返回的 `qt` 字段（index 4），作为分时基准线。
- **分钟 K**：`kline/mkline` 的 `m1/m5/m15/m30/m60` 返回 `[datetime, open, close, high, low, volume]`；
  m1 单次约 320 根，m5–m60 约 800 根。分钟 K 复用日 K 的 MA / 缩放 / 十字线链路。
- 分时在 Intraday 模式下约每 5 秒自动刷新一次；分钟 K 与日 K 一样按需加载、`⌘R` 刷新。

---

## 免费 / 低成本方案对比

| 方案 | 费用 | 实时 | 历史 K | 稳定度 | 需 Key | 适合 |
|------|------|------|--------|--------|--------|------|
| **东财 + 腾讯**（本项目） | 免费 | 近实时 | 日 K 完整 / 备源短窗 | 中 | 否 | 个人工具 / MVP |
| **AKShare** | 免费开源 | 近实时 | 很全 | 中（爬虫聚合） | 否 | Python 生态 |
| **Tushare Pro** | 积分 / 付费 | 受限 | 规范、全 | 高 | 是 | 严肃回测 |
| **Yahoo / 新浪单用** | 免费 | 一般 | 一般 / 不复权 | 中 | 否 | 备源片段；不宜作 A 股唯一主源 |

### 不推荐作为本项目扩展

- **Yahoo**：A 股覆盖与时效弱；港美场景再考虑。
- **新浪日 K**：免费接口多为不复权，寻宝鼠禁用。
- **BaoStock**：前复权可靠但 TCP 协议重，在已有腾讯前复权备源后不再维护。

---

## 合规与风险

1. **公开接口 ≠ 永久授权**：可能限流、改字段、封 IP。  
2. **延迟**：非交易所直连；盘中「近实时」即可，不适合超低延迟交易。  
3. **复权**：K 线均为前复权（东财 `fqt=1` / 腾讯 `qfq`）。  
4. **商业使用**：上架或对客服务前请换持牌 / 有合同的数据源。

---

## 本应用中的数据流

```
Watchlist codes ──► market::fetch_quotes (东财→腾讯, 1s)      ──► 左侧列表报价
                   └─► status_bar_codes (macOS NSStatusItem)  ──► 菜单栏现价/涨跌
Selected code  ──► market::fetch_klines (东财→腾讯)          ──► 日 K + MA
Selected code  ──► market::fetch_minute_series (腾讯)       ──► 分时（价格/均价/基准线/量）
Selected code  ──► market::fetch_minute_klines (腾讯)       ──► 1/5/15/30/60 分 K + MA
⌘K 搜索       ──► market::search (东财→腾讯 SmartBox)        ──► 添加自选
寻宝鼠扫描    ──► market::fetch_klines_adjusted (东财→腾讯)  ──► treasure 评分
              ──► treasure_cache.json
自选 / 偏好   ──► config.json（含 status_bar_enabled / codes）
```

macOS 配置路径示例：

```text
~/Library/Application Support/stock-analysis/config.json
```

---

## 调度伪代码

```rust
// market.rs
// quotes  → Eastmoney → Tencent
// klines  → Eastmoney → Tencent (both 前复权)
// search  → Eastmoney → Tencent SmartBox
```

已实现：`eastmoney.rs` + `tencent.rs` + `market.rs`。
