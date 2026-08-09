# ZStock 架构治理与用户体验改造计划

状态：核心代码实施完成；发布期/Beta/用户验收待执行  
制定日期：2026-08-09  
最近更新：2026-08-09  
适用版本：v0.0.40 起  
估算口径：1 名主开发人员；工期是工作量估算，不是发布日期承诺

## 1. 目标与顺序

本计划分成两个严格串行的阶段：

1. **先完成架构治理**：建立质量门禁，修正数据、持久化和币种正确性，拆解 `StockApp`，隔离非核心能力，形成稳定的领域接口和 ViewModel。
2. **架构闸门通过后再改用户体验**：围绕“今日、研究、机会、组合”四个用户任务重做信息架构，再逐步建设决策卡、组合风险、回测证据、基本面质量和复盘闭环。

架构阶段不修改评分算法、产品命名和主界面布局；除了修复会造成错误数据或误导的 P0 问题，不把重构与产品改版混在同一个 PR。

## 2. 当前基线

- 代码约 29,000 行 Rust。
- `StockApp` 有约 151 个字段，行情、图表、扫描、持仓、AI、设置和 UI 状态集中在同一对象。
- 11 个模块使用 `#![allow(unused_imports)]`；多个 app/UI 文件复制整套宽 import。
- `cargo fmt --all -- --check` 当前失败。
- `cargo clippy --all-targets -- -D warnings` 当前产生 137 项；其中约 50 项来自旧 Objective-C 宏，其余为真实的死代码、重复分支、复杂度和机械 lint。
- 共 100 个测试：88 个离线测试通过，12 个联网 smoke test 被忽略。
- release workflow 覆盖 macOS、Windows、Linux，但 PR 阶段没有 fmt、clippy、test 强制门禁。
- 行情主备切换按“整批是否有任一有效值”判断，不能逐代码补齐部分缺失。
- JSON 没有 schema version、原子写入和可靠恢复；解析失败会静默回到默认配置。
- API Key 明文保存在配置中。
- A 股和港股交易、现金与组合汇总没有币种模型，CNY/HKD 可能被直接相加。

以上数字作为重构基线。每个阶段不得让测试、启动性能、行情刷新或旧数据兼容性倒退。

## 3. 目标架构

```text
app/
├── StockApp                 # GPUI 生命周期、顶层路由、事件分发
├── state/                   # 聚合 UI/运行时状态，不含业务算法
├── controller/              # 用例编排、异步请求、跨 Store 协调
└── ui/                      # 小型 ViewModel + 用户事件

domain/                      # 纯 Rust 业务类型与规则
├── market
├── portfolio
├── discovery
└── analysis

services/                    # 端口/接口
├── market_data
├── repositories
└── secrets

infrastructure/              # 端口实现
├── market/eastmoney
├── market/tencent
├── storage/json_store
└── credential_store

features/
└── work_mode                # 可选的隐私/伪装模式
```

依赖方向必须保持：

```text
ui → controller → domain/services ← infrastructure
```

约束：

- `domain` 不依赖 GPUI、HTTP、文件系统或具体数据源。
- UI 不读取整个 `StockApp`，不在 render 中计算策略、持仓或评分。
- controller 只编排用例，不负责绘制和底层解析。
- 不建立包揽所有能力的万能 Trait、万能 Repository 或全局事件总线。
- 不通过 `Rc<RefCell<_>>` 掩盖状态所有权问题。

## 4. 架构阶段

### A0. 冻结基线与建立安全网

工作量：2–3 天。

任务：

- 固定 Rust toolchain，避免 `stable` 自动变化引入新 lint。
- 单独提交一次全仓 `cargo fmt`，不混入逻辑改动。
- 归档旧版 JSON fixtures、关键界面截图和 release 性能基线。
- 为以下行为增加特征测试：
  - 配置与持仓加载。
  - 持仓流水重放。
  - 行情部分成功与顺序保持。
  - 切股后丢弃旧异步响应。
  - 720×440、920×580、1280×800 布局。
- 建立 PR workflow，并让 tag 发布依赖同一套质量 workflow。

PR 必须执行：

```bash
cargo fmt --all -- --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

Clippy 处理规则：

- 旧 `objc` 宏优先升级依赖；无法升级时，只允许在两个 macOS FFI 模块局部豁免并记录原因。
- 删除无用代码，不用 crate/module 级宽泛 `allow`。
- 过渡期可使用最长两个迭代的旧 warning 清单，但任何新增 warning 立即失败。

验收：

- 88 个现有离线测试保持通过。
- fmt、check、clippy、test 在 CI 全绿。
- Windows/macOS/Linux 均进入 CI；macOS x64/arm64 均至少编译。
- 不再新增宽泛 lint 豁免。

### A1. 建立行情数据契约与 Provider 边界

工作量：5–7 天。

新增领域数据包络：

```text
QuoteRecord
├── code / market / currency
├── price / change / volume
├── source / fetched_at / market_time
├── availability
└── freshness

KlineSeries
├── code / source / as_of
├── adjustment
└── candles
```

拆分：

- `services/market_data.rs`：`QuoteProvider`、`KlineProvider`、`SearchProvider`。
- `infrastructure/market/eastmoney.rs`。
- `infrastructure/market/tencent.rs`。
- `infrastructure/market/service.rs`：主备编排与健康状态。

批量行情流程：

1. 标准化代码并按 A/H 市场分组。
2. 请求各市场首选源。
3. 逐代码识别缺失、无效、停牌或过期项。
4. 备用源只请求缺失代码。
5. 按原始请求顺序合并，并保留每只股票的数据源和时间。
6. 两个源都缺失时保留最近可用值并标记 stale；不能使用 0 冒充真实价格。

测试 fixtures 覆盖：部分返回、乱序、重复代码、空响应、字段类型变化、截断、超时、混合 A/H、主备都失败。

验收：

- 任意一只成功不再导致整批被判定成功。
- app/controller 不直接依赖东财或腾讯模块。
- 每条行情和 K 线都能说明来源、时间、币种和复权状态。
- 联网 smoke test 移到 nightly，不阻塞 PR，也不断言具体实时价格。

### A2. 持久化、配置迁移与 SecretStore

工作量：5–7 天。

拆分：

- `infrastructure/storage/paths.rs`
- `json_store.rs`
- `config.rs`
- `migrations.rs`
- `repositories.rs`
- `credential_store.rs`

设计：

- 为 `config.json`、`portfolio.json`、`journal.json` 增加 `schema_version`。
- 迁移是纯函数：识别版本 → 反序列化 → 逐版本迁移 → 校验不变量。
- 保存采用同目录临时文件、flush/sync、原子 rename。
- 每次迁移前创建版本化备份，至少保留最近 3 份。
- 只有文件不存在时可以返回默认值；损坏、未来版本、无权限必须进入可见错误/恢复模式。
- 所有 `let _ = storage::save_*` 消失，失败进入统一应用错误状态。
- API Key 通过 `SecretStore` 存入系统凭据库；只有安全存储成功后才能删除 JSON 中的明文。

fixtures 覆盖：空配置、字段缺失、未知字段、截断 JSON、未来 schema、写入中断、rename 失败和磁盘错误。

验收：

- 迁移幂等，重复运行结果一致。
- 任一步失败都不覆盖或删除原文件。
- 模拟进程中断后至少存在一个完整可读版本。
- `config.json` 不再包含 API Key。
- 用户可以从最近备份恢复。

### A3. 修正 A/H 多币种领域模型

工作量：5–7 天。

任务：

- 新增 `Currency::{Cny,Hkd}` 和不可跨币种直接相加的 `Money`。
- `Trade`、手续费、现金、成本、已实现盈亏和 `Position` 显式携带币种。
- 现金从单个 `cash` 迁移为按币种余额。
- `PortfolioSummary` 按币种输出；没有可靠 FX 时不展示伪精确的统一总资产。
- 旧记录根据明确可识别的代码推断币种；无法确定的记录进入“待确认”，不自动折算。
- 交易与现金建议使用 decimal；行情与技术指标继续使用 `f64`，不顺带重写指标系统。

验收：

- API/类型层不能直接把 HKD 加到 CNY。
- 纯 A、纯港、A/H 混合、手续费、部分卖出、清仓、现金约束和迁移全部有测试。
- 迁移前后交易数、代码、股数、当地币种成本和已实现盈亏不变。
- 混合组合至少展示 CNY/HKD 两组，不展示错误合计。

### A4. 拆解 `StockApp` 与 controller

工作量：7–10 天。

将 `StockApp` 收敛为约 8 个顶层聚合对象：

- `AppServices`
- `MarketState`
- `ChartState`
- `DiscoveryState`
- `PortfolioState`
- `AnalysisState`
- `UiState`
- `RuntimeState`
- 可选 `WorkModeFeature`

迁移顺序：

1. 输入框、焦点、弹层、dock 和 Tab → `UiState`。
2. 行情、K 线、缓存、请求状态 → `MarketState` / `ChartState`。
3. 长线、短线、行业下钻 → `DiscoveryState`。
4. 持仓、表单、组合 AI → `PortfolioState`。
5. loading/error/generation token 收敛为明确的请求槽或 `RequestState<T>`。
6. `market.rs` → `controller/market.rs`。
7. `symbols.rs` → `controller/watchlist.rs`、`controller/discovery.rs`。
8. `prefs.rs` → `controller/preferences.rs`。
9. `chart_ctrl.rs` → `controller/chart.rs`。

仅把字段装入子 struct 不算完成；散落的 `impl StockApp` 必须逐步迁成 controller/domain 方法。

验收：

- `StockApp` 顶层字段不超过约 10 个聚合对象。
- `app/mod.rs` 不再包含业务算法或具体数据源调用。
- domain 测试不需要启动 GPUI。
- 切股、取消扫描和刷新场景中，旧响应不能污染当前状态。
- 行情、扫描、持仓、设置具备独立状态边界和单元测试。

### A5. 隔离 Work Mode 与拆分 UI

工作量：7–10 天。

Work Mode：

- 迁入 `features/work_mode/{state,presenter,view,config}.rs`。
- controller 输出语义事件，不根据 `work_mode` 拼文案或伪装指标。
- Normal Presenter 与 Work Presenter 分别处理显示。
- 通过默认开启的 `work-mode` Cargo feature 保留现有能力；`--no-default-features` 仍应构建。

UI 拆分：

- `ui/detail.rs` → `overview / strategy / ai / indicators`。
- `ui/left.rs` → `watchlist / portfolio / discovery`。
- `ui/chrome.rs` → `navigation / status / settings`。
- 每个 View 只接收小型 ViewModel 与回调。
- 删除 11 个 `#![allow(unused_imports)]`，不创建 `prelude::*` 掩盖依赖。

验收：

- 行情、持仓、提醒、扫描 controller 中搜索不到 `work_mode` 分支。
- 禁用 work-mode feature 后核心应用仍能构建和启动。
- UI 不调用 HTTP、文件系统或数据源实现。
- 巨型复制 import 块和宽泛 unused allow 全部消失。

### A6. 架构冻结与稳定发布

工作量：至少 5 个交易日 Beta 观察。

质量预算：

- 冷启动到可交互：p95 ≤ 1.5s，且不比基线恶化超过 10%。
- 缓存页面切换：p95 ≤ 100ms。
- 图表交互：帧耗时 p95 ≤ 16.7ms，p99 ≤ 33ms。
- 100 只股票应用到 Store：p95 ≤ 50ms。
- 1,000 只股票本地评分：≤ 200ms。
- 空闲内存建议 ≤ 250MB；持续运行 1 小时增长不超过 10%。

发布方法：

1. 新 Store 先 shadow run，旧 UI 仍读旧状态，对比关键状态摘要。
2. Internal 开启 `architecture_v2`、`persistence_v2`、`portfolio_currency_v2`。
3. Beta 默认开启新架构，旧 UX 不变，观察至少 5 个交易日。
4. Stable 开启新架构。
5. 经过一个稳定版本周期且无回退后删除旧实现和 flags。

## 5. 架构完成闸门

只有同时满足以下条件，才能开始 UX 主改版：

- fmt、check、clippy `-D warnings`、全部离线测试三平台全绿。
- `StockApp` 只负责窗口、路由和 Store/Service 引用。
- domain 不依赖 GPUI、HTTP 或文件系统。
- 行情、扫描、持仓、设置拥有独立边界。
- 行情具备逐代码主备补齐、来源、时间、币种、新鲜度和复权信息。
- 多币种汇总正确，旧持仓迁移可恢复。
- JSON 版本化、原子写入、备份、迁移和 SecretStore 全部完成。
- Work Mode 与核心 controller 隔离。
- UI 通过 ViewModel 消费领域结果。
- 性能没有突破预算。
- Beta 至少稳定运行 5 个交易日，没有 P0/P1 缺陷。

架构闸门未通过时，只允许修复 P0 正确性或误导性文案，不进行大规模界面重排。

## 6. UX 目标信息架构

一级业务导航控制为四项：

1. **今日**：行情状态、市场概况、提醒、持仓风险和待处理事项。
2. **研究**：搜索、自选、当前标的图表和决策卡。
3. **机会**：长线观察、短线信号、规则扫描和候选清单。
4. **组合**：持仓、交易流水、组合风险与复盘。

全局搜索、设置和数据状态放工具区，不与业务导航并列。市场分析并入“今日”；日记放入“组合 > 复盘”；AI 不作为一级或常驻 Tab，只附着于决策卡和候选清单提供摘要。

## 7. UX 阶段

### U0. 语义纠偏与评分治理

工作量：5–7 天。

文案映射：

| 当前 | 调整后 |
|---|---|
| 现在找 / 寻宝 | 机会 / 低位策略 |
| 筛可买 / AI 筛可买 | 规则筛选 |
| 可买观察 | 候选观察 |
| 可买分 | 策略匹配度 |
| 置信 / 数据置信 | 数据完整度 |
| 建议买 / 建议卖 | 参考观察区间 / 目标区间 |
| 强势 85/100 | 技术状态：偏强；分数放证据区 |

评分改为三层：

1. **资格门槛**：ST、流动性、样本、缺失和基本风险；门槛失败不能靠其他加分抵消。
2. **因子贡献**：位置、趋势、动量、量能、风险分组，每组设上限，避免相关指标重复加分。
3. **展示校准**：验证前显示高/中/低与本轮排名；完成历史校准后才显示历史百分位，仍不表示上涨概率。

验收：

- 用户可见界面不再把规则结果称为“可买”或“AI 筛选”。
- `confidence` 只能显示为“数据完整度”。
- 每个结论都显示截至时间；缺数据时显示“证据不足”。
- 固定样本中最高端分数饱和率 < 2%，Top 20 至少 90% 有可区分排序值。
- 属性测试保证：增加风险不能提高结论等级，缺数据不能提高完整度，资格拦截不能被加分覆盖。

### U1. 主导航与任务型首页

工作量：5–7 天。

任务：

- 落地“今日、研究、机会、组合”四个一级任务。
- 合并重复的长线/短线入口；“机会”内部再选择策略。
- “研究”默认显示决策摘要，原始指标放折叠证据区。
- 无内容的分析台自动折叠；高级指标和画线工具进入二级菜单。
- 设置、颜色、工作模式和更新移入工具区。
- 为核心任务记录默认本地的匿名事件与耗时，不记录股票代码、持仓金额或日记正文。

验收：

- 一级业务导航不超过 4 项。
- 至少 80% 测试用户第一次点击进入正确任务区域。
- “打开机会 → 保存候选”中位时间 ≤ 90 秒。
- 空态、加载、失败、离线、过期和多币种状态均有 UI 测试。

### U2. 决策卡 MVP

工作量：7–10 天。

决策卡固定回答：

- 当前状态：符合策略 / 等待触发 / 不符合 / 证据不足。
- 最多 3 条支持因素、2 条风险。
- 参考观察条件或区间。
- 明确的失效条件。
- 数据充分时的目标区间和风险收益比。
- 行情时间、来源、复权、样本量、策略版本和历史证据等级。
- 下一步：加入观察、创建提醒、记录计划；不提供自动下单。

AI 只位于“解释更多”，必须标注来源；技术指标位于折叠证据区。

验收：

- 决策卡完全由领域 ViewModel 生成，UI 不重新计算规则。
- 相同输入与策略版本生成一致结果并通过快照测试。
- 缺失失效条件时不能显示“符合策略”。
- 用户判断“是否需要行动、为什么、何时失效”的中位时间 ≤ 45 秒。

### U3. 组合风险中心

工作量：7–10 天。

第一版：

- CNY/HKD 分币种资产、现金和盈亏。
- 可选基准币种换算，显示汇率来源和时间。
- 单股权重、最大持仓、现金比例和行业集中度。
- 按决策卡失效价计算的单笔风险金额与组合风险预算。
- 数据缺失和行情过期覆盖率。
- 风险事项按严重度排列，可下钻到持仓决策卡。

第二版再增加组合回撤、相关性和压力场景，并要求历史覆盖充分。

验收：

- 无汇率时不同币种绝不直接合计。
- 权重合计误差 < 0.01%，每个汇总值可下钻。
- 风险覆盖不足时显示覆盖比例，不把缺失当低风险。
- 用户能在 30 秒内指出最大集中风险及对应标的。

### U4. 回测证据与策略实验室

工作量：10–15 天。

回测引擎必须支持：

- 信号日后的次日开盘或明确的可配置成交规则。
- 市场对应的手续费、印花税和滑点。
- 基准收益、超额收益、最大回撤、收益分布和交易数。
- 时间切分、滚动样本外验证和市场状态分组。
- 策略、参数、数据集和成本模型版本。
- 前视偏差、重复持仓和幸存者偏差检查。
- 样本量和置信区间。

证据等级：

```text
无证据 → 样本不足 → 样本内探索 → 样本外观察 → 多阶段稳定
```

只有达到预设交易数、扣除成本后优于基准且样本外没有明显失效的规则，才进入默认机会页；其他规则归入实验策略。

验收：

- 修改未来 K 线不会改变此前日期信号。
- 报告强制显示成交规则、成本、基准、区间、交易数和版本。
- 禁止只显示胜率。
- 固定数据集结果可复现。
- 决策卡可打开对应证据报告。

### U5. 基本面质量防线

工作量：10–15 天，依赖可靠且可追溯的财务数据。

建议字段：

- ROE/ROIC 趋势。
- 经营现金流与净利润匹配度。
- 资产负债率与变化。
- 营收、利润增长趋势。
- 商誉、减值和审计风险。
- 分红连续性。
- PE/PB 历史分位。

规则：

- 基本面是独立质量门槛，不继续堆进技术总分。
- 财务数据按公告日期 point-in-time 处理，回测不能提前使用未来财报。
- 缺失显示未知，不能默认通过。
- 质量红线不能被“低位”抵消。

验收：

- 每个字段有报告期、公告日、来源和单位。
- 公告日晚于信号日的数据不会进入当时评分。
- 固定价值陷阱样本即使低位分高也会被拦截。
- 决策卡能解释拦截原因。

### U6. 决策日记闭环

工作量：7–10 天。

将日记升级为“计划—执行—结果—复盘”：

- 保存策略版本和当时证据快照。
- 保存观察期限、触发条件、区间、失效条件、目标和风险金额。
- 到期后自动进入待复盘。
- 自动计算 5/10/20 个交易日结果及最大有利/不利波动。
- 用户记录是否执行、退出原因和是否遵守计划。
- 至少 20 个完整计划后才展示个人行为趋势，并明确样本量。

验收：

- 从决策卡一键建计划，关键字段自动带入且可修改。
- 策略升级后旧日记仍保留原始版本和证据。
- 提醒和复盘记录幂等，不重复创建。
- 支持本地导出与可确认删除。

## 8. UX 与产品有效性指标

优先衡量决策质量和证据完整性，不把短期收益、交易频率或点击量作为成功指标。

- 研究标的、保存候选、识别组合最大风险、创建计划的任务成功率均 ≥ 90%。
- 一级导航首次点击正确率 ≥ 80%。
- 90% 以上计划包含触发、失效和期限。
- 100% 策略结论带数据时间、来源和策略版本。
- 100% AI 内容带来源标签，且不能改变确定性排序、价位或提醒规则。
- 过期或缺失数据产生强结论的数量为 0。
- 评分端点饱和率 < 2%。
- 不同币种错误合计为 0。
- 回测报告缺成本、基准或样本量的数量为 0。

可用性测试每个 UX 迭代至少使用 5 名目标用户。任何上传型遥测必须显式选择；默认仅在本地记录任务事件和耗时。

## 9. 建议 PR 切分

每个 PR 保持可编译、可测试、可回滚，不混合机械格式化、文件搬迁和功能变化。

架构：

1. `A-001`：固定 toolchain + 全仓 rustfmt。
2. `A-002`：PR 质量 workflow + warning 基线。
3. `A-003`：特征测试、旧 JSON fixtures、性能/截图基线。
4. `A-004`：行情领域数据包络和 Provider traits。
5. `A-005`：逐代码主备补齐与契约 fixtures。
6. `A-006`：Repository 边界和原子 JSON store。
7. `A-007`：schema migrations、恢复模式与 SecretStore。
8. `A-008`：Currency/Money、多币种持仓迁移。
9. `A-009`：State 聚合对象与请求状态。
10. `A-010`：Market/Chart controllers。
11. `A-011`：Discovery/Portfolio/Preferences controllers。
12. `A-012`：Work Mode feature 隔离。
13. `A-013`：UI ViewModel 化和巨型文件拆分。
14. `A-014`：shadow run、性能验收、删除旧依赖。

UX：

1. `U-001`：文案语义和评分领域模型。
2. `U-002`：机会页单流程与评分展示校准。
3. `U-003`：四任务导航、空面板折叠与首页。
4. `U-004`：决策卡 ViewModel 和 UI。
5. `U-005`：组合风险中心第一版。
6. `U-006`：回测引擎正确性与证据报告。
7. `U-007`：基本面数据契约与质量门槛。
8. `U-008`：计划—复盘日记闭环。

## 10. Feature flags 与发布顺序

建议 flags：

- `architecture_v2`
- `persistence_v2`
- `portfolio_currency_v2`
- `home_experience_v2`
- `decision_card_v1`

发布顺序：

1. 架构 shadow run。
2. Internal 开启新架构和数据迁移。
3. Beta 开启新架构，保留旧 UX。
4. Stable 开启新架构。
5. Beta 单独开启新 UX，观察至少 5 个交易日。
6. Stable 开启通过验收的 UX 迭代。
7. 一个稳定版本周期后删除对应旧实现和 flag。

当前没有可靠遥测后端，不做虚假的百分比灰度；使用 Internal/Beta/Stable 渠道、本地健康日志和用户主动导出的诊断包。

## 11. 明确不做

- 不大爆炸式重写整个应用。
- 不在架构阶段同时大改主界面和评分算法。
- 不同时更换 HTTP 库或异步运行时。
- 不继续增加 MACD、KDJ 等指标制造功能丰富感。
- 不增加更多 AI Provider，先收敛 AI 适配层。
- 不让 LLM 决定候选排序、价位、仓位、止损或回测结论。
- 不把 0–100 分、数据完整度或 AI 文案包装成上涨概率。
- 不以胜率单独评价策略。
- 不在没有 point-in-time 数据时做基本面历史回测。
- 不把 CNY 与 HKD 直接相加。
- 不在缺少验证和回滚能力时接入自动交易。

## 12. 总体里程碑

| 里程碑 | 包含阶段 | 单人工作量估算 | 交付结果 |
|---|---|---:|---|
| M1 质量基线 | A0 | 1 周内 | PR 门禁、格式和特征测试 |
| M2 基础设施正确性 | A1–A3 | 3–4 周 | 数据契约、可靠存储、多币种 |
| M3 应用架构 v2 | A4–A6 | 3–4 周 + Beta | Store/controller/ViewModel、架构闸门通过 |
| M4 UX 核心版 | U0–U3 | 4–6 周 | 任务导航、评分治理、决策卡、组合风险 |
| M5 有效性证据 | U4–U6 | 6–10 周 | 回测证据、质量门槛、复盘闭环 |

第一阶段目标不是减少总代码行数，而是让依赖方向、状态所有权、数据正确性和失败恢复都可测试。第二阶段目标不是增加功能数量，而是让用户能更快回答：现在是否需要行动、为什么、何时失效、组合里最大的风险是什么。

## 13. 立即开始项

执行从 `A-001` 开始：

1. 建立 `codex/architecture-v2` 分支。
2. 固定 Rust toolchain。
3. 单独完成全仓 rustfmt。
4. 运行 fmt/check/clippy/test 并保存基线。
5. 提交后进入 `A-002`，建立 PR 质量 workflow。

在 `A-001` 完成前不移动模块、不修改业务逻辑、不调整 UX。

## 14. 实施结果（2026-08-09）

本节是执行账本。代码交付与本地可自动验证项已经落地；必须依赖 GitHub runner、真实交易日、持续运行或目标用户的项目保留为发布验收，不能用一次本地运行替代。

### 14.1 架构交付

| 阶段 | 状态 | 已交付 | 仍需发布期验证 |
|---|---|---|---|
| A0 | 代码完成 | Rust 1.93.1、全仓 rustfmt、旧 JSON fixtures、三档布局测试、PR/release 共用质量 workflow、nightly 联网测试、v0.0.40 基线 | GitHub 上首次完整矩阵结果 |
| A1 | 代码完成 | `QuoteRecord`/`KlineSeries` 契约、Provider ports/adapters、A/H 分组、逐代码备用源补齐、顺序/重复保持、last-good stale、健康状态与故障 fixtures | nightly 真实源稳定性 |
| A2 | 代码完成 | schema version、纯迁移、同目录原子写、3 份备份、恢复状态、Repository/SecretStore；macOS Keychain、Linux Secret Service、Windows DPAPI | 三平台凭据库实机验证 |
| A3 | 代码完成 | `Currency`/`Money`、交易与持仓币种、分币种现金和汇总、旧数据迁移、未知币种待确认、风险中心不跨币种伪合计 | 混合币种 Beta 数据复核 |
| A4 | v2 路径完成 | `StockApp` 顶层收敛为 9 个聚合对象加一个兼容状态；Market/Chart/Discovery 请求槽接入真实异步路径；Watchlist/Preferences controllers 与领域边界就位 | `legacy AppState` 按 A6 要求保留到一个稳定版本周期后删除；继续把剩余兼容方法迁出 `impl StockApp` |
| A5 | 代码完成 | Work Mode feature/presenter/state/config/view 隔离；无默认 feature 零告警构建；detail/left/chrome 拆为子 View；移除 11 个宽泛 unused allow | 无默认 feature 的三平台 CI/启动检查 |
| A6 | 测量能力完成 | 100 条行情 apply < 50ms、1,000 条本地评分 < 200ms 的固定预算测试；冷启动首帧、缓存任务切换、UI build p95/p99、每分钟 RSS 与满一小时增长的匿名本地报告；架构 flags 默认开启。release 单次实测：首帧 327.76ms、UI build 0.74ms、RSS 101,548,032 bytes | 20 次导航样本、GPU 帧耗时、1 小时 RSS、5 个交易日 Beta、稳定版后删除旧实现 |

依赖方向已经建立为 `ui → controller → domain/services ← infrastructure`；`domain` 不依赖 GPUI、HTTP 或文件系统。UI 子模块没有直接进行 HTTP、文件系统写入或调用具体行情 Provider。

### 14.2 UX 交付

| 阶段 | 状态 | 已交付 | 仍需验证/依赖 |
|---|---|---|---|
| U0 | 代码完成 | 用户可见语义纠偏；资格门槛、分组封顶因子、风险不可反向提分、完整度单调性、端点饱和率与 Top 20 区分度测试 | 真实候选分布校准 |
| U1 | 代码完成 | 今日/研究/机会/组合四任务导航；设置与数据状态归工具区；仅本地记录任务名、完成时间与耗时 | 5 名用户的首次点击正确率与 90 秒任务测试 |
| U2 | 代码完成 | 领域生成的决策卡、支持/风险/观察/失效/目标/RR、来源/时间/复权/样本/版本/证据等级与下一步动作 | 5 名用户的 45 秒判断测试 |
| U3 | 代码完成 | CNY/HKD 分组、集中度、现金比例、按失效价计算风险、行情/失效/行业覆盖率、未知与过期显式展示 | 5 名用户的 30 秒风险识别测试 |
| U4 | 代码完成 | 次日开盘成交、市场成本/滑点、基准/超额/回撤/分布/CI、70/30 样本外、偏差检查、策略/数据集/成本版本、三种规则 | 更长固定数据集与滚动样本外观察 |
| U5 | A 股 Provider 完成 | point-in-time 财务契约；东财主指标与资产负债表按报告期合并并取较晚公告日；ROE/ROIC、现金利润匹配、负债、增长、商誉与审计意见均保留值、单位、报告期、公告日和来源；价值陷阱、未来公告、缺失未知测试；决策卡展示逐项证据 | 港股免费接口缺少可靠公告日，继续显示未知；分红连续性与 PE/PB 历史分位仍需可追溯数据 |
| U6 | 代码完成 | 计划—执行—结果—复盘、证据快照和版本、到期状态、5/10/20 交易日结果、MFE/MAE、幂等、20 份后才显示趋势、本地导出与确认删除 | Beta 真实日记数据复核 |

### 14.3 本地质量门禁

以下结果在 macOS arm64、Rust 1.93.1 上通过：

```text
cargo fmt --all -- --check                         PASS
cargo check --all-targets                          PASS (0 warnings)
cargo clippy --all-targets -- -D warnings          PASS (0 warnings)
cargo check --all-targets --no-default-features    PASS (0 warnings)
cargo clippy --all-targets --no-default-features -- -D warnings
                                                    PASS (0 warnings)
cargo test --all-targets                           PASS (134 passed, 14 ignored)
cargo build --release                              PASS
```

14 个 ignored 测试是联网或原生凭据库 smoke tests，由 nightly workflow 串行执行；A 股财务 Provider 真实接口与 macOS Keychain 写入/读取/删除 smoke test均已单独通过。质量矩阵使用 Linux x64、Windows x64、macOS 15 arm64 和 macOS 15 Intel x64 原生 runner；两种 macOS 架构均执行离线测试。

### 14.4 发布验收清单

以下项目尚未被标记为完成：

- [ ] GitHub Actions 四平台/架构质量矩阵全绿。
- [x] macOS Keychain 保存、读取、删除实机通过。
- [ ] Linux Secret Service、Windows DPAPI 的保存、读取、删除实机通过。
- [ ] Internal/Beta 连续运行至少 5 个交易日，无 P0/P1。
- [x] A6 冷启动、任务切换、UI build 与 RSS 本地匿名测量链路及预算判定完成。
- [ ] 累积 20 次缓存导航、GPU 图表帧 p95/p99 及连续 1 小时 RSS 证据并达到预算。
- [ ] 至少 5 名目标用户完成首次导航、保存候选、决策判断、组合风险识别和建计划测试。
- [x] A 股 point-in-time 财务 Provider 接入，并满足 U5 的来源、单位、报告期和公告日覆盖。
- [ ] 港股公告日、分红连续性及 PE/PB 历史分位接入可追溯数据源。
- [ ] 一个稳定版本周期无回退后，删除 `legacy AppState`、兼容 flags 和旧实现。

这些检查完成后，才可把本文状态改为“全部验收完成”。
