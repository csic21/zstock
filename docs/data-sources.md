# A 股 / 港股免费数据源调研（2026）

面向本项目（个人桌面分析工具）的数据源选择说明。

## 结论（我们采用什么）

**当前实现：东财主源 → 腾讯备用（均免费、无需注册 / API Key）；港股行情/日 K 偏腾讯**

| 用途 | 主源 | 备用 |
|------|------|------|
| 批量行情（A） | 东财 `ulist` | 腾讯 `qt.gtimg.cn` |
| 批量行情（港） | 腾讯 `hkxxxxx` | 东财 `secid=116.xxxxx`（delay 节点） |
| 历史日 K（前复权，A） | 东财 `fqt=1` | 腾讯 `newfqkline`（`qfq`，约 640 根上限） |
| 历史日 K（港） | 腾讯 `hkxxxxx` 日 K | 东财（常空，作兜底） |
| 分时 | 腾讯 `minute/query`（A / 港） | — |
| 分钟 K | 腾讯 `mkline`（A；港股该接口暂不可用） | — |
| 代码搜索 | 东财 suggest（含港股） | 腾讯 SmartBox（`hk~`） |
| 宇宙榜单（寻宝池） | 东财 `clist`（仅 A） | —（失败回落内置池） |

调度逻辑在 `src/data/market.rs`；交易时段门控在 `src/data/session.rs`。  
状态栏会显示本次实际命中的源（如「K线 · 腾讯财经」）。

**市场与代码约定**

- **A 股**：6 位纯数字（`600519` / `000001`），东财 `1.` / `0.` secid。
- **港股**：5 位纯数字（`00700`），东财 `116.00700`，腾讯 `hk00700`。搜索支持 `hk700` / `700.HK`。
- **寻宝鼠**仍只扫 A 股池。

**交易时段轮询（省流量）**

| 市场 | 轮询窗口（本地 UTC+8，工作日） |
|------|--------------------------------|
| A 股 | **09:15–11:30**、**13:00–15:00**（含集合竞价） |
| 港股 | **09:00–12:00**、**13:00–16:10**（含开市前 / 收市竞价） |

- 自选 / 持仓里**有哪个市场**，只在该市场开盘时把对应代码加入轮询。
- **盘外 / 周末**：不轮询；打开应用时 `refresh_all` **拉一次**快照即可。
- 盘中刷新间隔仍由设置 `quote_interval_secs` 控制（默认 2s；价格未变时跳过整页重绘）。

**为何只两家**

- 覆盖 A 股 + 港股主路径，不需要美股 / 多年回测专用链。
- 东财覆盖 A 股主路径；腾讯 HTTP 全备（含港股行情 / 前复权日 K / 分时 / 搜索）。
- 腾讯日 K 约 **640 根**上限；寻宝鼠优先东财长窗（~1000），东财失败时腾讯短窗仍可用。

优点：零成本、厂商少、备源也是前复权。  
缺点：非官方 SLA；腾讯 K 窗短于东财；港股分钟 K 暂缺。

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
| 批量行情 | `https://qt.gtimg.cn/q=sh600519,sz000001,hk00700`（`~` 分隔，GBK） |
| 日 K | `https://web.ifzq.gtimg.cn/appstock/app/newfqkline/get?param={sym},day,,,{n},qfq` |
| 日 K 兜底 | `https://web.ifzq.gtimg.cn/appstock/app/fqkline/get?param=…` |
| 分时 | `https://web.ifzq.gtimg.cn/appstock/app/minute/query?code={sym}` |
| 分钟 K | `https://ifzq.gtimg.cn/appstock/app/kline/mkline?param={sym},m1|m5|m15|m30|m60,,{n}`（A；港股 param 常失败） |
| 搜索 | `https://smartbox.gtimg.cn/s3/?q=关键词&t=all`（含 `hk~00700~…`） |

实现：`eastmoney.rs`、`tencent.rs`；调度：`market.rs`。

### 腾讯分时 / 分钟 K 说明

- **分时**：`minute/query` 返回当日每分钟 `HHMM price cum_volume cum_amount`（累计手数 / 累计成交额），
  由 `cum_amount / (cum_volume×100)` 客户端计算均价线；昨收取返回的 `qt` 字段（index 4），作为分时基准线。
- **分钟 K**：`kline/mkline` 的 `m1/m5/m15/m30/m60` 返回 `[datetime, open, close, high, low, volume]`；
  m1 单次约 320 根，m5–m60 约 800 根。分钟 K 复用日 K 的 MA / 缩放 / 十字线链路。
- 分时在 Intraday 模式下、**该标的所属市场交易时段内**约每 5 秒自动刷新；盘外不刷。
- 分钟 K 与日 K 一样按需加载、`⌘R` 刷新（港股分钟 K 暂不可用）。

---

## 免费 / 低成本方案对比

| 方案 | 费用 | 实时 | 历史 K | 稳定度 | 需 Key | 适合 |
|------|------|------|--------|--------|--------|------|
| **东财 + 腾讯**（本项目） | 免费 | 近实时 | 日 K 完整 / 备源短窗；港股日 K 可 | 中 | 否 | 个人工具 / A+港 |
| **AKShare** | 免费开源 | 近实时 | 很全 | 中（爬虫聚合） | 否 | Python 生态 |
| **Tushare Pro** | 积分 / 付费 | 受限 | 规范、全 | 高 | 是 | 严肃回测 |
| **Yahoo / 新浪单用** | 免费 | 一般 | 一般 / 不复权 | 中 | 否 | 备源片段；不宜作 A 股唯一主源 |

### 不推荐作为本项目扩展

- **Yahoo**：A 股覆盖与时效弱；美股场景再考虑。
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
Watchlist+持仓 ──► session 过滤（A/港各自时段）
                └─► market::fetch_quotes（盘中按 quote_interval；盘外不轮询）
                     · 纯港：腾讯优先；混仓/纯A：东财→腾讯
                     └─► status_bar_codes (macOS) ──► 菜单栏现价/涨跌
启动 / ⌘R    ──► refresh_all 拉一次全量快照（含盘外）
Selected code  ──► market::fetch_klines（A: 东财→腾讯；港: 腾讯→东财）
Selected code  ──► market::fetch_minute_series（腾讯；仅该市场盘中自动刷）
Selected code  ──► market::fetch_minute_klines（腾讯，A）
⌘K 搜索       ──► market::search（东财→腾讯，含港股）
寻宝鼠扫描    ──► 仅 A 股池 + fetch_klines_adjusted
自选 / 偏好   ──► config.json
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
