# 架构说明

> 本文面向想读源码或想扩展数据源的人，说明 `otatool` 的模块划分、一次查询的完整数据流、两种协议层的加密差异，以及断点续传的位图格式。

## 1. 分层总览

项目刻意切成**两层**：`src/core/`（不依赖 egui 的核心逻辑）与 `src/app/`（egui 界面层）。`src/main.rs` 只做窗口与面板布局，`src/models.rs` 是静态数据表。

```
src/main.rs          mod app; mod core; mod models; —— 建窗口、排面板、120ms 重绘节流
src/models.rs        MODELS: &[(&str /*代号*/, &str /*市场名*/, &str /*品牌*/)]，539 条
src/core/            核心层：无 egui 依赖，可单独测试
  mod.rs             pub mod download / downgrade / history / meta / ota / search / theme / violet
  search.rs          机型模糊搜索（纯函数）
  ota.rs             官方 OTA 协议 + Package 数据模型 + 来源枚举
  violet.rs          社区源 VioletToolBox 客户端（公开 HTTP API；含社区售后包分桶与补齐）
  downgrade.rs       售后降级服务器客户端（AES-256-GCM + RSA-OAEP）
  meta.rs            从 ZIP 首部读 META-INF/com/android/metadata，补齐售后包真名
  download.rs        多线程分块下载 / 断点续传 / MD5 校验
  history.rs         下载历史记录（含从磁盘反推进度、状态枚举）
  theme.rs           深色调色板 P + 自绘组件（card / pill / progress_bar …）
src/app/             界面层：只消费 core 的结果，不直接发请求
  mod.rs             pub mod state / format / shell / views
  state.rs           App 状态机：查询、探测、下载、暂停、续传、历史、错误提示
  format.rs          bytes() / date() / dur() 纯格式化
  shell.rs           在资源管理器中定位文件（COM STA 线程 + 父/子 PIDL + Win10/Win11 Shell 回退 + 按需 UAC helper）
  views/mod.rs       pub mod about / bottom / history / packages / sidebar / topbar
  views/topbar.rs    顶栏（logo、标题、「历史下载」「数据来源 · 致谢」入口）
  views/sidebar.rs   左侧机型搜索面板
  views/packages.rs  中部版本卡片列表（售后分区在前，升级包在后）
  views/bottom.rs    底部常驻下载栏 + 状态行
  views/history.rs   「历史下载」弹窗（续传 / 打开位置 / 删除 / 清空）
  views/about.rs     「数据来源与致谢」弹窗
```

**分层的硬约束**：`core/` 里任何文件都不得 `use eframe` / `use egui`（`theme.rs` 是唯一例外，它本来就是组件库）。这条约束换来的是：协议、搜索、下载、校验都能在**没有窗口**的环境下被单元测试覆盖，也让界面重写不会牵连网络逻辑。

## 2. 数据流：一次查询发生了什么

```
用户输入关键词
     │
     ▼
[app/state.rs] 关键词交给 core::search::search()
     │         多 token 空格分隔 = AND；按分数 → 代号短 → 字典序排序，按代号去重
     ▼
  选择机型（Model { code, name, brand }）
     │
     ▼
[app/state.rs] 起后台线程跑 core::ota::query_merged(code, name, brand)
     │
     ├──────────────► 线程 A：query_all_branches()
     │                 A–Z 26 个字母各拼 otaVersion = "{model}_11.{L}.01_0001_197001010000"
     │                 每批 6 个并发，只保留 manual_url 非空的结果
     │
     ├──────────────► 线程 B（与 A 并行）：query_chain()
     │                 从 A.01 起把响应里的 realOtaVersion 原样回送，最多 40 步
     │                 用 version_key 去重 → 补出分支扫描拿不到的中间历史版本
     │
     ├──────────────► 线程 C（与 A/B 并行）：售后降级服务器
     │                 downgrade::query()：A–Z 扫描顺带捞到的项目号 → query-v3
     │                 成功后 enrich()：每个包一次 Range 读 ZIP 首部 metadata
     │
     └──────────────► 线程 D：社区源：violet::find() + /versions
                       packages() 拆成 Community { normal, after }
                       enrich_after()：每个降级包换直链 + 读 ZIP 首部补大小/日期
                       std::panic::catch_unwind 包裹，失败则 community_ok = false
     │
     ▼
  合并 Merged { packages, aftersale, community_ok }
     │  packages 去重：version_key()（先按 "(" / "（" 截断，再取尾部数字与点）
     │  packages 排序：版本号倒序 → 官方优先于社区 → 字符串
     │  aftersale：官方售后包在前 + 社区售后包在后，两路互不去重
     ▼
[app/state.rs] 发布到 self.packages / self.aftersale，状态栏「官方 X 个，社区源补 Y 个；另有 N 个售后全量包」
     │
     ├─► probe_community()：后台批量探测社区包链接是否仍可下（link_ok: Option<bool>）
     │
     ▼
用户点「下载」或「直链」
     │
     ├─ 直链：resolve_direct_url()（官方，302 换签）或 violet::download_link()（社区）
     │        解析 Expires= / x-oss-expires= → 约 10 分钟有效期
     │
     ▼
[core/download.rs] start(url_getter, target, total, threads, expect_md5)
     │  线程数 clamp 1..64；预分配 set_len(total)
     │  读 .partstate 位图（大小与块数都匹配才复用）
     │  N 个 worker 用 AtomicU64 抢块号 → Range: bytes=start-end → seek + write_all
     │  失败重试 ≤12 次，退避 300ms*(attempt+1)；attempt≥1 时重新申请直链
     │  测速线程每 400ms 用 6 秒滑动窗口；主线程每 5s（暂停后 100ms）落位图
     ▼
  verify()：expect_md5 长度为 32 才校验，4MB 缓冲流式计算，忽略大小写比对
     │  失败 → "MD5 校验不符（得到 …），文件可能损坏，建议删除后重新下载"
     ▼
  删除 .partstate → 底栏显示完成状态
```

**为什么 A–Z 扫描和升级链要并行**：分支扫描是「广度」（一次问遍 26 个分支，拿每支最新），升级链是「深度」（沿一条链往回走，拿中间版本）。两者互不依赖，串行会让首次查询时间翻倍，所以 `query_chain()` 单独跑在一个 `std::thread` 里与分支扫描并行，最后在主线程合并。

## 3. 两种加密协议

两套接口共享同一个思路：**用非对称加密只保护「会话密钥」，真正的报文用对称加密**。这样既有 RSA 的密钥交换安全性，又避免了大报文走 RSA 的性能灾难。

| | 官方 OTA 通道 | 售后降级通道 |
| --- | --- | --- |
| 服务地址 | `https://component-ota-cn.allawntech.com` | `https://downgrade.coloros.com` |
| 路径 | `/update/v6`（POST） | `/downgrade/query-v3`（POST） |
| 报文加密 | **AES-256-CTR**（`ctr::Ctr128BE<Aes256>`） | **AES-256-GCM**（`aes-gcm`） |
| 会话密钥包裹 | **RSA-OAEP(SHA1)**，内置 RSA-2048 公钥（pkcs8 DER） | **RSA-OAEP(SHA1)**，同款流程 |
| 完整性 | **无**（CTR 是流模式，不认证；靠下载后的 MD5 兜底） | **有**（GCM 自带 128-bit 认证标签，密文被改直接解密失败） |
| IV / Nonce | 随报文一起传（`params.iv`，base64） | 随报文一起传 |
| 用途 | 查询版本清单、取全量包直链 | 查询售后（降级 / 维修）全量包 |

### 官方 OTA（AES-256-CTR + RSA-OAEP(SHA1)）

1. 本地 `rand` 生成 32 字节随机 AES 密钥；
2. 用内置 RSA 公钥按 **OAEP(SHA1)** 加密该密钥 → `protectedKey`；
3. 请求头 `protectedKey` 为一段 JSON：`{"SCENE_1":{"protectedKey","version","negotiationVersion":"1615879139745"}}`；
4. 请求体 JSON 经 **AES-256-CTR** 加密后放入 `params.cipher`，IV 放 `params.iv`（均 base64）；
5. 请求头另带 `model` / `otaVersion` / `operator: ALLNET` / `pipelineKey: ALLNET` / 随机 `deviceId` 等；
6. 响应体是**字符串化的 JSON**，再解一层拿到 `cipher` / `iv`，用同一会话密钥 AES-CTR 解密得到真正的 JSON。

错误码映射（只挑用户能看懂的）：`2004` → 「该机型或该分支没有可用的全量包」；`2100` → 「参数不符合服务器要求」。

### 售后降级通道（AES-256-GCM + RSA-OAEP(SHA1)）

流程与上面同构，差别只在对称算法：**AES-256-GCM** 自带认证标签，因此服务器返回的密文一旦被中间设备改写或截断，解密阶段就会报错，而不是吐出一段垃圾 JSON。这和 CTR 通道形成互补——CTR 通道的安全兜底落在**下载完成后的 MD5 校验**上，GCM 通道则在**协议层**就把完整性做掉了。

实测要点（都是踩过的坑，改代码时别改回去）：

- **端点必须是 `query-v3`**，`query-v2` 一律回 500 `AES decrypt error`；
- **RSA 公钥必须是降级服务器那一份**，与官方 OTA 的 key 不同。用错 key 会回 `decrypt from crypto-dist-domain error`；
- **RSA 包裹的是 `base64(session_key)` 的字节**，不是裸 32 字节密钥；
- **`deviceId` 必须是「已建档」的真实 DUID**（64 位大写十六进制，手机上 `*#6776#` 可查），传 64 个 `0` 会回 `code=1004 duid查询guid为空`；
- 服务端约 1/3~1/2 概率抽风返回 500 `AES decrypt error`，重试 8 次内必成；
- `otaVersion` 前缀与结果无关，服务器只认 `model + prjNum`（5 位项目号）。
- 降级服务器对 `model + prjNum` 做绑定校验：拿别的机型的项目号去查会被打回（`no list` 或 500），**项目号不能借用**。

### 项目号为什么必须扫 A–Z 全部分支

项目号不在降级服务器上，而在官方 OTA 响应的 `components[0].componentPackets.vabInfo.data.header` 数组里，形如 `oplus_separate_soft=23607`。它是两条协议之间唯一的桥。

实测（`_build/tmp` 里的 Node 探针）踩过两个坑：

1. **只试 6 个分支会漏掉一大半机型**。最初只用 `A / F / C / B / E / D` 去问，539 款机型里只有 215 款查到项目号，剩下 324 款判成「官方没给」。逐字母扫到 Z 之后，其中抽样的 143 款**全部**都查到了（0 款缺失）—— 老机型和平板的项目号往往躺在 H / J / L / M 这类分支下。所以 `query_all_branches_harvest()` 一次问遍 26 个字母，顺带把项目号捞出来（同一个响应两用，不必为售后通道再问一轮官方 OTA）。

2. **「没有字段」不等于「没查通」**。判据必须是 `components` 数组非空（`fn responded()`），不能看有没有 `vabInfo`：老机型（如 RMX1901）的正常响应里**压根没有 `vabInfo` 字段**，那恰恰是「官方确实没给它项目号」这个有结论的答案；反过来，界面查询时的并发把官方接口打到降级，响应里连 `components` 都是空的 —— 那才是「这一轮没查通」。

于是 `Soft` 分成三态：`Found(项目号)` / `Absent`（问到了，官方没有）/ `Unknown`（没问通）。这个区分一路传到界面：

- `Found` 但降级服务器 8 次都被抽风打回 → 分区照常显示，提示「售后包这次没查到，可点重新扫描重试」；
- `Absent` → 显示「该机型的官方数据里没有售后全量包」；
- 售后列表非空 → 显示「另有 N 个售后全量包」。

项目号只在 `Found` 时进内存缓存：坏结论不该被记住，否则用户重扫时拿到的是一个可能已经过期的「没有」。

### 售后包的版本号为什么要额外读 ZIP

降级服务器**不下发真实版本号**：同机型所有售后包的 `colorosVersion` 都是 `ColorOS16.1.0` 这种只剩大版本号的字符串（实测 RMX3888 与 PKX110 均如此）。所以 `core/meta.rs` 在查询后对每个售后包发一次 `Range: bytes=0-65535` 的请求，从 ZIP 首部读出 `META-INF/com/android/metadata`（stored 未压缩，明文 key=value），解析出真正的 `version_name` / `ota_version` / `android_version` / `security_patch` / `post-timestamp` 并覆盖到 `Package` 上。

成本约 64 KB/包，并发（`std::thread::scope`）执行，单个包失败只影响该包（回落到服务器给的粗版本号），不影响整次查询。

> 注意：售后包的 `version_name` 可能与官方包**完全相同**（RMX3888 两者都是 `RMX3888_16.0.10.500(CN01)`，但售后 10.24 GB / 官方 8.76 GB）。所以落盘文件名给售后包加 `_售后` 后缀（`RMX3888_16.0.10.500(CN01)_售后.zip`），避免同名撞同一个 `.partstate` 位图。

### 社区售后包为什么要单独分桶

社区源的版本清单里混着一类名字带「降级包」后缀的条目（实测全库 41 条，标记词只有「降级包」这一种，另兼容「回退包 / 维修包 / 售后包」）。它们的版本号往往很旧（如 `RMX3888_14.1.0.804(CN01)`）：

- 按普通版本合并进升级清单，会被当成「一个很老的升级包」，而它真正的用途是**刷回去**；
- 直接丢掉，官方降级服务器没下发包的机型就彻底没有售后包可用。

所以 `violet::packages()` 的返回类型是 `Community { normal, after }`，两个桶**各自按 `version_key` 去重**。必须分开的理由是实测出来的：8 个机型各有一条「降级包」与同机型某个普通版本的版本号**完全相同**（RMX3706 `14.0.0.903(CN01)`、RMX3700 `13.1.0.120(CN01)` 与 `14.0.0.903(CN01)`、RMX3300 `14.0.0.1410(CN01)`、RMX3551 `14.0.0.1410(CN01)`、RMX3820 `14.0.0.1120(CN01)`、RMX3823 `14.0.0.1120(CN01)`、RMX3841 `14.0.0.1501(CN01)`），但两者换出来的直链完全不同（普通包走 `gauss-componentotacostmanual-cn.allawnfs.com`，降级包走 `rbp01.realme.net` 的 `506_sign_*` / `ROLL_BACK_*`）—— 是两个不同的文件，用同一个桶去重必然丢一个。

并入售后分区时**不与官方售后包互相去重**：官方卡的版本号读自包内 `version_name`，社区写的是它自己的版本名，字符串对不上，硬比只会把同一个包列两遍。宁可多一张卡，也不要让人以为「少了一个版本」。官方在前、社区在后 —— 官方直链长期有效且带 MD5，社区直链是当场签发的（约 10 分钟）。

`enrich_after()` 负责补齐社区降级包的信息：逐个 `download_link()` 换取直链，再 `meta::fetch()` 一次 Range 同时取回 `content-range` 里的**总大小**与 ZIP 首部的 `post-timestamp` / `security_patch`。按 `chunks(3)` 三路并发而不是全展开 —— 社区源是免费公共服务，单机型通常也只有 1~3 个降级包，三路足够把往返压到一条链路的量级。换不出直链才置 `link_ok = Some(false)`（那才是真的失效）；换得出来但读不到首部只是补不全信息，仍然算活。

**占位补丁日**：realme 有一部分包把 `security_patch` 写成 `2088-08-05` 这类明显虚构的值。`meta::sane_patch()` 对年份做 2010–2030 的合理性判断，超出范围直接丢弃 —— 卡片上宁可不显示，也不显示假日期。

## 4. 断点续传的位图格式

进度文件与目标文件同目录、同名，后缀 `.partstate`：

```
<版本号>.zip.partstate
┌─────────────────────────────────
│ 8589934592          ← 第 1 行：文件总字节数（u64 十进制）
│ 1111111111111111111111000…  ← 第 2 行：分块位图，'1' 完成 / '0' 未完成
└─────────────────────────────────
```

规则：

- **分块大小固定 8 MB**（`const CHUNK = 8 * 1024 * 1024`），块数 = `ceil(total / CHUNK)`；
- 位图第 `i` 个字符对应第 `i` 块，字符数 == 块数；
- **读取时严格校验**：第 1 行必须等于本次任务的总大小，第 2 行长度必须等于块数，**任一不符就整体作废**（当作全是 0 重下）。这条校验是防止「换了包但复用了旧位图」把文件写坏的关键；
- 写入时机：主线程每 **5 秒**落一次；收到取消信号后 **100 ms** 内再落一次（保证暂停那一刻的进度不丢）；
- 生命周期：下载**成功或失败都会删除**该文件（校验完成后删除），避免残留位图污染下一次同名下载。

它与多线程的安全性来自同一个前提：**块编号 ↔ 绝对偏移的映射是固定且唯一的**，worker 只写自己抢到的块、只写该块的偏移区间，续传只做「跳过已完成的块」。因此任意线程数、任意次中断，落盘字节都一致（README 的「下载与校验」一节给了完整论证）。

## 5. 这样分层为什么便于扩展新来源

现在已经有四条通道（官方 OTA / 升级链 / 官方售后 / 社区与社区售后），再加一个数据源（比如某个厂商的公开版本库）实际改动仍收敛在四点：

1. **数据模型不动**：新来源只需产出与现有相同的 `Package`（`version / android / size / manual_url / published / security_patch / branch / md5 / ota_version / source / vio / link_ok / note`）。界面层拿到的永远是 `Vec<Package>`。
2. **来源枚举加一支**：`Source` 加一个变体（现有 `Official / Community / AfterSale / AfterCommunity` 四支），实现 `label()` / `key()` / `tip()` 三个小函数，卡片角标、筛选、致谢文案自动跟上。注意 `match` 都是穷尽的：漏改任何一处编译器都会直接报出来，不会静默漏掉。
3. **合并逻辑复用**：`query_merged` 已经把「多来源并行 → 归一化去重 → 排序」写成通用流程，新来源挂进去就是一个新的并行分支（并且和新来源失败时一样，用 `catch_unwind` 隔离，失败只降级不崩）。
4. **下载器零改动**：下载只认「一个能换出新直链的闭包」`UrlGetter`（`Arc<dyn Fn() -> Result<String, String>>`）。官方包的闭包走 302 换签、社区包的闭包走 `/download-link` 接口，新来源只是提供第三个闭包实现——分块、重试、续传、校验全部照旧。

反过来说，**不该**扩展进来的东西：任何需要登录/鉴权的通道、任何把固件本体塞进仓库的做法、任何会把界面状态泄漏到 `core/` 的改动。
