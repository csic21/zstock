# A 股免费数据源调研（2026）

面向本项目（个人桌面分析工具）的数据源选择说明。

## 结论（我们采用什么）

**当前实现：东财 → 新浪 → BaoStock 自动切换（均免费、无需注册 / API Key）**

| 用途 | 主源 | 备用 1 | 备用 2 |
|------|------|--------|--------|
| 批量行情 | 东财 `ulist` | 新浪 `hq.sinajs.cn` | — |
| 历史日 K | 东财前复权 | 新浪日 K（不复权） | **BaoStock 前复权**（TCP） |
| 代码搜索 | 东财 suggest | 6 位代码兜底 | — |

调度逻辑在 `src/data/market.rs`。  
状态栏会显示本次实际命中的源（如「K线 · BaoStock」）。

**BaoStock**：`public-api.baostock.com:10030`，匿名登录，日 K `adjustflag=2` 前复权；纯 Rust 实现协议（见 `src/data/baostock.rs`）。

优点：零成本、K 线多一层稳定兜底。  
缺点：非官方 SLA；新浪日 K 不复权；BaoStock 为 TCP 会话，略慢于 HTTP。  
限频：本应用约 8s 刷一次行情。

> 仅供学习研究，不构成投资建议；商业产品请换有授权的数据商。

---

## 免费 / 低成本方案对比

| 方案 | 费用 | 实时 | 历史 K | 稳定度 | 需 Key | 适合 |
|------|------|------|--------|--------|--------|------|
| **东方财富公开接口**（本项目） | 免费 | 近实时 | 日 K 完整 | 中 | 否 | 个人工具 / MVP |
| **AKShare** | 免费开源 | 近实时 | 很全 | 中（爬虫聚合） | 否 | Python 生态；Rust 可参考其源 |
| **adata** | 免费开源 | 有 | 有 | 中高（多源切换） | 否 | Python 量化 |
| **Tushare Pro** | 积分 / 付费 | 受限 | 规范、全 | 高 | 是 | 严肃回测；免费档有配额 |
| **iTick / AllTick 等** | 有免费档 | 好 | 有 | 较高 | 是 | 要官方 API 文档时 |
| **Yahoo / Alpha Vantage** | 免费档 | 一般 | 一般 | 中 | 部分要 | **A 股弱**，不推荐主战场 |

### 1. AKShare（推荐作对照实现）

- 完全免费开源，底层大量走东财 / 新浪等。
- 文档与社区最大；接口偶发因源站改版而挂。
- 我们的 Rust 客户端语义上等价于「精简版东财 adapter」。

### 2. Tushare Pro

- 数据规范、适合因子与财务；日线需积分。
- 实时与高阶权限通常要付费或贡献数据。
- 若你要「可复现研究库」，后期可加 `TUSHARE_TOKEN` 后端。

### 3. 商业免费档（iTick、AllTick、Infoway…）

- 有正式 API、WebSocket；免费档常有订阅数 / QPS 限制。
- 适合以后做 Level-1 推送时再接入，作为 `DataProvider` 第二实现。

### 4. 不推荐作为 A 股主源

- yfinance / Finnhub / Alpha Vantage：A 股覆盖与时效通常不够。

---

## 合规与风险

1. **公开接口 ≠ 永久授权**：可能限流、改字段、封 IP。  
2. **延迟**：非交易所直连；盘中「近实时」即可，不适合超低延迟交易。  
3. **复权**：当前 K 线使用东财 `fqt=1`（前复权）；可配置扩展后复权 / 不复权。  
4. **商业使用**：上架或对客服务前请换持牌 / 有合同的数据源。

---

## 本应用中的数据流

```
Watchlist codes ──► market::fetch_quotes (东财→新浪, 8s)      ──► 左侧列表报价
Selected code  ──► market::fetch_klines (东财→新浪→BaoStock) ──► 日 K + MA
⌘K 搜索       ──► market::search (东财为主)                   ──► 添加自选
自选 / 偏好   ──► ~/Library/Application Support/stock-analysis/config.json
```

macOS 配置路径示例：

```text
~/Library/Application Support/stock-analysis/config.json
```

---

## 后续可插拔设计建议

```rust
// 当前为函数式 failover（market.rs）：
// quotes  → Eastmoney → Sina
// klines  → Eastmoney → Sina → BaoStock
// search  → Eastmoney（6 位兜底）
```

已实现：`eastmoney.rs` + `sina.rs` + `baostock.rs` + `market.rs`。
