# 独立工单业务试点

独立源码发行目录的导出、干净构建、部署与兼容矩阵见[独立业务交付](../../docs/independent-business-delivery.md)。当前最终验收以[Phase34报告](../../docs/validation/phase-34.md)为准，下文保留各阶段的历史证据边界。

Phase32 增加默认禁用的私有指标端点：显式设置独立 `WORK_ORDERS_METRICS_TOKEN` 后，从私有网络采集 `/work-orders-api/internal/metrics`。源码镜像从仓库根构建：`docker build -f business-apps/work-orders/backend/Dockerfile -t work-orders-api:candidate .`。固定指标协议不共享业务 SQL 或身份模型，指标含义及容量/告警限制见[监控说明](../../deploy/monitoring/README.md)。

业务模型和平台边界见[系统架构](../../docs/architecture.md#platform-capabilities)。平台管理用户、租户、组织、操作权限、独立数据授权、应用 token、通知和发布指针；工单服务使用自己的 PostgreSQL 保存实际业务数据。Vue 前端通过默认 opaque iframe 和 MicroProps 2.0 bridge 接入宿主。

`backend/` 独立Rust crate拥有自己的Cargo.toml／Cargo.lock、薄入口与窄library，不依赖平台实现或Node。Phase16–20依次提供库接管、授权／读取／CSV、幂等创建／版本写入、成员分配及持久提醒意图／正式通知API。Phase21将 `frontend/` 改为独立Vue模块／Vite工程，消费Rust创建header、版本写入和显式未知结果恢复；Node入口已退役，参考模块仅用于历史兼容测试。各阶段实际运行结果单独报告，当前实现说明不预记本轮验收通过。

## 部署组成

| 组件 | 位置或入口 | 配置边界 |
| --- | --- | --- |
| Vue／Vite 业务前端 | `frontend/`，公开版本 `/sub-apps/work-orders/versions/<version>/` | 独立Vue／Vite锁依赖，相对ESM／动态chunk；仅静态资源、精确宿主origin、业务API地址和版本标识；内存草稿／事件按当前业务身份分区 |
| 独立 Rust API | `backend/Cargo.toml`，`work-orders-api` 二进制 | 独立业务DB、双历史与live/ready；授权、读写／CSV、创建幂等、成员目录／分配预校验及持久提醒意图／正式通知发布 |
| Node参考测试模块 | backend/server.mjs及兼容测试 | 仅验证历史协议/CORS/迁移；index已退役，npm start运行Rust |
| 业务 PostgreSQL | 独立数据库和账号 | 不读取平台表，不复用平台数据库 URL |
| 平台 bootstrap/授权/发布工具 | [deploy/README.md](deploy/README.md) | 当前平台管理员 session，明确应用及授权对象，分步执行 |
| 持久通知实时服务 | [notification-ws](../../mini-services/notification-ws/README.md) | 平台 DB 与内部 session 校验，真实收件箱为权威来源 |

客户端不持有OAuth client secret或JWT签名密钥。Rust受保护请求先Basic内省，再以同一应用Bearer取 `work-orders` context，功能permission与 `ticket/<action>` grant分别检查。SELF仍按owner，处理人不扩大范围；tenant、组织硬上限及grant落实到读取和锁定查询。需要分配变更时，先作scope内非锁预读并释放连接，再用普通应用Bearer调用平台成员HTTP；BEGIN／FOR UPDATE后复核组织、version和oldAssignee，错配409。外部HTTP不持业务连接或行锁，锁内复核授权期限和成员校验窗口。

## 1. 管理员登记与授权

先按[管理员操作手册](deploy/README.md)创建机密客户端、登记 `work-orders` 应用和七个入口／操作菜单。`work-orders-client` 是客户端名称；平台生成的实际 ID 配置为业务服务的 `PLATFORM_CLIENT_ID`。

七个节点保留稳定 key：根 MENU、隐藏的 `ticket:read` 详情 MENU `/micro/work-orders/tickets/:id`、可见的 `ticket:create` 创建 MENU `/micro/work-orders/ticket-create`，以及四个操作 BUTTON。入口授权逐路径明确，父入口和详情读权限不会放行创建路径，数据 grant 仍独立配置。已有旧 BUTTON 形状会由工具拒绝并要求人工核对；新菜单形状须与采用新路径的前端版本一起更新，不能回滚到将 `/tickets/new` 当作创建的旧前端。

租户、组织、角色、用户和当前直接 membership 的数据 grant 必须逐项明确。Bootstrap 不默认分配业务授权或标记 TRUSTED。Rust CLI 的离线授权计划列出正式数据 grant API 的 GET／preview／PATCH 顺序，显式 grants-apply --apply 读取目标实时版本并通过独立 read/write 权限和平台显式 `DATA_GRANT_POLICY` 检查；旧 Node 管理入口与 SQL 草案输出已退役。已有角色菜单和用户集合的管理 API 是全量替换，提交前仍需读取当前完整集合。成员维护对保留项采用差量写入，保持 membership UUID；实际删除/重建会级联删除旧成员的数据 grants，不能沿用旧目标快照。操作步骤见[管理员指南](deploy/README.md#3-明确租户组织角色成员和数据范围)。

## 2. 独立业务 API 和数据库

Rust 独立构建、配置与回退见 [Rust 工单说明](../../docs/work-orders-rust-baseline.md)。启动在同一事务内使用旧Node的advisory lock，验证迁移摘要和真实schema。0001／0002原字节保持；0002保存创建receipt，[0003](backend/migrations/0003_reminder_intents.sql)新增不可变提醒intent，支持合法Node／SQLx迁移前缀接管，最终登记0001／0002／0003的Node SHA-256和SQLx SHA-384双历史。已执行迁移不重放，不改写存量工单、UUID、归属、version或计数器；异常结构／历史拒绝启动。原Phase19及更早严格基线不认识0003，不能直接回退旧二进制，须保留intent、receipt和平台publication并使用兼容构建，不能删表、改checksum或清除历史绕过校验。

Rust的 `GET /work-orders-api/health/live` 为进程检查，`/health/ready`及`/health`检查0001／0002／0003双历史、必需对象和启用的意图不可变触发器，不证明每个调用方授权。GET列表／详情／CSV、POST创建、PATCH／DELETE、GET `/assignees`及POST `/tickets/{id}/remind`提供相应操作。路径均保留 `/work-orders-api` 前缀，尚未切换全量流量。

Rust `POST /tickets` 必须传单个 `Idempotency-Key` 请求头，1–128个ASCII字母、数字或 `._:-`，正文沿用 `{title,description?,priority?,assigneeUserId?}`。标题trim、描述默认空、优先级默认NORMAL及省略／null处理人归一后计算固定字段SHA-256；当前tenant／org／user／create／key内，同键同规范化正文重放原201 JSON，不同正文409。新请求和重放均重新授权；切组织或失权不能取回旧域结果。创建响应即使对应工单后来编辑／软删仍是原始创建历史，需GET详情确认当前状态。

Phase21前端创建调用显式携带该header，首次提交固定一次逻辑创建的键及完整正文；提醒正文的 `idempotencyKey` 仍是独立事件协议。网络／协议／5xx不确定结果保留原事件，不自动换键或重发；只有校验合法成功DTO后才结束事件，畸形2xx同样按结果未知处理。实际交互和iframe内存生命周期见下一节，不据接入说明操作生产切流。

网络响应丢失、COMMIT报错、提交后授权到期或15秒创建截止可能返回503 `CREATE_RESULT_UNKNOWN`，原键／正文配合当前有效凭据恢复；PATCH／DELETE未知结果503 `WRITE_RESULT_UNKNOWN`时先读回版本，不自动重试。提交前context过期401回滚。Phase19新建非空分配须是目标组织有效直接成员；update省略或同UUID保留原快照不重查旧成员，真正change／clear才受目录组织scope，非空change另做平台验证。已commit同key／hash的原201重放仍需当前create授权，但不重查历史member，也不证明当前成员有效或记录未变。

`GET /assignees`保留action=create／update、organizationId、q、page、size及 `{records:[{userId,displayName,organizationId}],total,page,size}` 形状。平台应用策略须显式登记ticket/create、ticket/update的 [memberLookups](../../docs/data-grant-management.md#member-lookups)，默认空，pair必须在同应用capabilities内；不增加成员grant或assign动作，普通业务用户不需grant管理权。成员校验从成员请求开始以单调时钟计 `min(PLATFORM_AUTH_TIMEOUT_MS,3000ms)`，响应和DB等待均消耗窗口，不在锁后续期。成员HTTP超时503 `PLATFORM_UNAVAILABLE`；提交前成员结果过期503 `MEMBER_VALIDATION_EXPIRED`并回滚，context同时复验；提交后context／成员到期则返回 `CREATE_RESULT_UNKNOWN`或 `WRITE_RESULT_UNKNOWN`并按原恢复协议对账。平台库与业务库不能原子提交，验证后仍可能撤销成员，不能把短TTL当跨库强一致。

Phase19自身未改schema／依赖；其成员能力回退至18时会恢复目录503及显式非null assignee的400，包括不能重放带非空处理人的原创建正文。Phase20升级0003后必须另用兼容新历史／意图的构建，已有分配、receipt和intent均保留；先核对在途结果。平台同时降至旧严格parser时还须处理其不认识memberLookups的新配置。具体采用／回退见 [Rust操作说明](../../docs/work-orders-rust-baseline.md)，不在本阶段执行实际切流。

Rust提醒仅接受 `{idempotencyKey}`，键为1–128个ASCII `[A-Za-z0-9._:-]`，独立于创建header和requestId。从同一次当前授权上下文同时要求 `ticket:read`／`ticket/read` 和 `notification:publish`／`notification/publish`，分别叠加grant与组织硬上限；恢复同样检查当前授权及未删除工单。目标为处理人，无处理人则为创建人，组织为工单组织；固定INFO正文只含编号和详情链接。短事务先保存tenant／发布者organization／可信application UUID／发布user／key域内不可变的工单、目标、完整正文及hash，确认COMMIT并释放连接／锁后才用当前应用Bearer调用正式平台通知API，每个传入请求最多一POST，不改工单或version。同域同键换工单或有效目标409，即使原平台POST从未提交；intent无TTL、无级联删除，不存token或成功缓存。

本地准备未确认或总deadline在尝试POST前耗尽返回503 `REMINDER_PREPARATION_UNKNOWN`，本次零平台POST；下一次确无持久intent时可按当前快照首次准备，未提交的准备不固定旧目标。尝试POST后的网络／超时／5xx／无效200等返回503 `REMINDER_RESULT_UNKNOWN`，保留同键和已固定正文，用当前凭据显式恢复，不自动换键重发。平台明确拒绝及409单独处理。成功200为经校验的 `publicationId`、两个值为1的计数、布尔 `duplicate` 和RFC3339 `createdAt`，接收者以真实平台session在当前组织收件箱按publicationId核对；实时服务未运行不等于持久失败，删除delivery后重放不会复活消息。业务意图和平台发布非跨库原子事务，校验成功的平台200不因工单随后变化而改判失败。授权登记见 [提醒说明](../../docs/data-grant-management.md#notification-reminders)，完整恢复边界见Rust操作说明；旧Node虽会迁移0003，提醒handler不使用intent，不能提供同样保证。

实际 Rust 启动须私有配置 `PLATFORM_API_ORIGIN`、平台生成的 `PLATFORM_CLIENT_ID` 和 `PLATFORM_CLIENT_SECRET`，应用编码固定为 `work-orders`。`PLATFORM_AUTH_TIMEOUT_MS` 默认3000毫秒，是内省与context含响应读取的总预算；并发默认32，饱和立即503不排队，平台JSON每份最多256 KiB。CORS默认字面 `null`，额外 origin 必须精确配置。分页、CSV1000行／4 MiB、同范围快照和错误规则见 [Rust 操作说明](../../docs/work-orders-rust-baseline.md)。

当前生产交付使用Rust工单API、专属数据库及独立Vite静态镜像。按[根部署说明](../../deploy/README.md)与[镜像交付说明](../../deploy/microapps/README.md)配置固定digest、HTTPS平台、客户端和独立数据库，通过business override接入；通知使用独立notification profile。旧Node index和legacy服务配置已在Phase34退役，legacy文件名仅保留空兼容文件，不再提供服务。

业务数据库/账号由部署方单独创建，不复用平台数据库或账号。前端版本、Rust API、业务数据库分别升级和回退；实际目标配置、凭据和外部发行仍由运维提供。

在 gateway JSON 中显式设置 `workOrders: "work-orders-api:3004"`，需要通知时同时设置 `notification: "notification-ws:3003"`，然后重新渲染入口。两项默认均为 `null`，未启用时对应路径返回 404。网关将 `/work-orders-api` 和 `/work-orders-api/*` 保留完整前缀转发到业务 API；未知业务路径仍返回 API 404，不进入宿主 HTML。

Opaque iframe 的业务请求携带字面 `Origin:null` 和短期应用 Bearer，默认业务 CORS 只允许该 origin，不启用浏览器 cookie 或 credentialed CORS。其他精确 origin 需显式配置 `WORK_ORDERS_ALLOWED_ORIGINS`。公开不可变静态版本保留已有匿名 CORS `*`，该规则不扩展到业务 API、OAuth 或 Socket.IO。

## 3. 独立构建、发布与回滚

推荐镜像路线先在部署系统拉取固定digest并检查容器health，使用所有环境共用的权威持久inventory执行Rust CLI artifact-bind／verify，再配置保留旧版本的入口Nginx路由。然后从应用管理“发布版本”登记已部署HTML URL，或使用带artifact／inventory参数的guarded register。登记得到DRAFT，单独preflight必须检查正文PASSED；HTTP200加FAILED不能激活，激活还会重新预检。平台不拉镜像、启停容器、生成路由或存储镜像digest，UI／RegisterBody只使用原六字段，来源不变性由CLI／CI独立清单负责。旧无inventory登记不等于镜像身份已验证。

v1同域静态版本可用`/work-orders-api`；v2从批准的外部静态origin直载时，构建明确设置平台绝对HTTPS API base或该origin自有的受控代理。opaque iframe仍发送Origin:null和应用Bearer，省略Cookie，宿主白名单保持真实公开HTTPS origin。GitHub候选、受保护手工publication与Linux部署分别执行，实际registry／目标地址／凭据由操作者提供；源码工作流不表示已远端执行。两版本静态容器／路由须同时保留以服务旧chunk，前端回滚不回滚Rust API或数据库。完整步骤见[业务镜像交付](../../deploy/microapps/README.md)。

前端独立锁定Vue3.5.43／Vite8.3.4，标准 `index.html` → `src/main.mjs` → 动态业务模块，不依赖window.Vue、root node_modules或 `frontend/public/sub-apps`。`base:'./'` 保持整个版本目录内相对入口／chunk／CSS，登记SDK1.0.0的classic bridge按原字节输出。明确配置实际宿主origin，并使用新版本号生成新构建：

```powershell
cd business-apps/work-orders/frontend
npm ci --ignore-scripts
npm run build -- --version 1.0.0 --allowed-host-origin https://saas.example.com --api-base /work-orders-api
cd ../../..
node scripts/package-microapp.mjs --app work-orders --environment production --version 1.0.0 --input business-apps/work-orders/frontend/dist
```

`asset-manifest.json`记录实际入口／动态chunk／CSS，`build-info.json`记录应用／SDK／Vue／Vite／Node版本、独立lock摘要及逐文件source摘要。公开配置不自动注入VITE_*、secret、DB URL或平台session。生产Nginx需为ESM、动态chunk、classic bridge和CSS提供正确MIME与匿名CORS，缺资源真实404；dev／preview仅是本地工具，不能作为生产服务。详见 [前端README](frontend/README.md)。

上述打包器是保留的静态目录／ZIP兼容路线，生成 `deploy/micro-apps/work-orders/versions/1.0.0/` 和该版本的 `release.json`。将此完整目录放到只读版本根目录后，按管理员手册分别执行 register、preflight 和 activate。登记只得到 DRAFT；工具不自动激活、不改变trust或授权。发布新版本时重新构建并使用新版本目录；已有版本不可覆盖。

回滚使用平台部署的上一版本指针并重新预检。保留旧版本与注册清单，归档／删除只按批准的保留方案由管理员明确执行；本应用没有自动清理版本或软删除工单的作业。工单前端版本变化不重建宿主、不迁移业务数据，也不改变应用身份、OAuth客户端和授权。旧前端缺少创建header／恢复协议，不能当作当前Rust契约的等价回退；回退前核对在途写入、提醒和内存事件，不能通过删receipt／intent绕过。

<a id="frontend-recovery"></a>
### 前端草稿与未知结果生命周期

创建／提醒事件和合法编辑草稿仅保存在同一存活iframe的模块内存，按appCode／tenantId／organizationId／userId隔离，不保留token或完整props、不用opaque localStorage或新增SDK能力。跨路由、同业务身份续期及切组织后返回可以恢复各自域的数据；切组织立即取消旧请求并隐藏旧服务器内容。刷新、关闭页面、销毁iframe或换发布实例不保证保留，未知结果尚未核对时请保持页面。浏览器内存与后端0003持久提醒意图的保证不同。

创建首次提交后锁定事件键与完整正文，结果未知由用户选择“恢复原事件”；合法201及完整工单DTO确认后才结清，不因畸形2xx或普通网络成功就另建事件。提醒分别提示PREPARATION_UNKNOWN（本次零平台POST）和RESULT_UNKNOWN，用同键显式重试；409先刷新并确认当前目标，再由用户发起独立新事件。编辑409保留输入并显示新版本，须明确采纳version；PATCH／状态／删除未知先读回核对，不自动重试mutation。

纯主题更新不多取列表、不取消正在进行的写入；路由切换取消离开视图的读取，迟到结果不能覆盖新路由。真实授权替换、到期、401或失权时隐藏服务器快照并暂停相应操作，保留的事件仍须当前有效授权才能恢复。创建 `/micro/work-orders/ticket-create` 和详情 `/micro/work-orders/tickets/:id` 各需精确MENU，功能权限及data grant不互相附送，旧 `/tickets/new` 只作详情标识。CSV继续通过宿主downloadCsv broker，iframe不增加same-origin或下载权限。

## 4. 验证与运维边界

库接管历史见 [Phase16](../../docs/validation/phase-16.md)，读取／CSV见 [Phase17](../../docs/validation/phase-17.md)，写入／幂等／0002及恢复见 [Phase18](../../docs/validation/phase-18.md)，成员策略／目录／分配与TTL竞态见 [Phase19](../../docs/validation/phase-19.md)，提醒／0003 intent与真实平台收件箱见 [Phase20](../../docs/validation/phase-20.md)。Phase21的独立Vite、opaque模块／chunk、当前Rust UI与未知结果浏览器证据由 [Phase21](../../docs/validation/phase-21.md)单独记录。PG默认ignored、VM／HTTP构建测试、真实浏览器与真实平台联合各自报告，未运行或默认跳过不能算通过，也不以旧Node联合替代Rust验收。

后端真实 PostgreSQL/HTTP 测试、前端实际浏览器测试、管理工具安全测试与实际平台联合 E2E 分别报告其覆盖范围。根目录 `tests/micro-release-gateways.mjs` 使用真实 Linux Docker/Nginx、正常验证的本地 HTTPS 和独立 Rust transport fixture，覆盖静态版本、API 前缀／正文／查询／404、polling 形态请求和 WebSocket Upgrade。该 fixture 不实现完整 Engine.IO/Socket.IO，也不提供真实工单业务或平台授权；Phase 3 传输验收不能替代 Phase 22 协议兼容和实际业务联合验收。

实际平台联合测试位于 `tests/platform-e2e.mjs`，显式提供最新平台 main、预先构建的 Rust 管理 CLI、隔离测试数据库连接、Nginx、生产宿主构建和现有 Playwright/Chromium 后运行。测试创建两座随机自有数据库，使用实际平台迁移与登录、业务迁移与 CRUD、通知 PostgreSQL 与 Socket.IO，再由实际 Vue iframe 验证操作和 v1→v2→回滚；测试只关闭／删除本次创建的资源。缺少显式运行配置时明确跳过；已配置但路径无效或运行失败则报告失败。测试不会自动构建 CLI，也没有旧 Node 管理入口回退。

联合测试在 Linux Node 24 下运行，所有服务进程与浏览器使用同一系统的可执行文件，便于严格跟踪自有 PID。先准备当前宿主的 `frontend/dist`、工单前后端依赖和本机运行时，再从仓库根目录执行：

```bash
cargo build --manifest-path tools/platform-admin/Cargo.toml --locked
export WORK_ORDERS_E2E_BACKEND_BINARY=/absolute/path/to/saas-platform-backend
export PLATFORM_ADMIN_BIN=/absolute/path/to/saas-platform-admin
export WORK_ORDERS_E2E_HOST_DIST=/absolute/path/to/frontend/dist
export NGINX_BIN=/absolute/path/to/nginx
export PLAYWRIGHT_MODULE=/absolute/path/to/node_modules/playwright
export BROWSER_EXECUTABLE=/absolute/path/to/chromium
node --env-file=.env.test --test business-apps/work-orders/tests/platform-e2e.mjs
```

`PLATFORM_ADMIN_BIN` 必须为当前构建产物的绝对路径，`PHASE25_CLI_BIN` 可作同义输入。CLI 仅从子进程环境接收本次 fixture 的 Bearer，不放入 argv。开发使用明确允许的 IPv6 回环 HTTP；生产 HTTPS 保持证书与主机名验证，可通过 `PLATFORM_CA_FILE` 显式提供公开 PEM CA 链。此受管测试会将生产 fixture 的 `PRODUCTION_CA_FILE`（或其 `NODE_EXTRA_CA_CERTS`）显式映射为 CLI 的 `PLATFORM_CA_FILE`；CLI 自身不读取 Node 配置，也不接收私钥。

`.env.test` 提供 `WORK_ORDERS_E2E_PLATFORM_DATABASE_URL` 或 `TEST_DATABASE_URL`；可另外提供 `WORK_ORDERS_E2E_BUSINESS_DATABASE_URL`，未给时在同一已授权测试实例中创建另一座独立业务库。维护账号需能创建／删除本次自有测试数据库。两座库都使用随机名称，测试中不复用维护 URL 所指数据库保存平台或工单数据。Nginx 需要可用的 `mime.types`，特殊路径可通过 `NGINX_MIME_TYPES` 指定；跨平台缓存依赖场景可显式提供匹配 esbuild wrapper 版本的 `ESBUILD_BINARY_PATH`。

视觉检查可启用 `WORK_ORDERS_E2E_VISUAL_QA=1`，截图保存到 Git 忽略的 `target/phase11-runtime/qa-<本次随机目录名>/`；其中 ownership.json 记录本次自有目录。服务、数据库、浏览器及临时文件照常清理，截图保留至审查人员明确确认后，仅删除该次已核对归属的 QA 目录，不因计时结束而丢弃截图。Linux 缺少中文字体时，可用 `WORK_ORDERS_E2E_FONT_DIR` 指向已有本地字体目录；测试生成自有 fontconfig 配置与缓存，浏览器 profile、缓存和下载也放在自有临时目录。不要把测试字体或 runtime cache 打包为业务前端资源。

联调测试包含受保护请求的节奏控制，应用 token 通过真实接口续签。当前每客户端内省默认配额为 600 次/分钟，另有 IP、失败认证和并发门禁。缺少配置后的跳过不视为通过。

历史验收结果统一见[架构文档](../../docs/architecture.md#validation)，目标环境需要重新运行验证，配置解析不能代替镜像构建、容器启动或真实 HTTPS 验收。


## Phase 28 正式验收（2026-10-19）

工单／门店镜像与统一外部 URL 接入已完成本阶段隔离验收：同一 839 项源码输入下，完整 fast 63 步、1083 项通过，0 失败／跳过；真实六镜像 registry push／候选清除／固定 digest pull 和 Compose 联合检查 14/14 通过，Chromium／Chrome／Edge 各 9/9 通过。来源、实际命令、旧失败及五项原验收映射见[Phase 28 验证报告](../../docs/validation/phase-28.md)。

平台仍只登记、预检与激活公开 URL；digest 不变性由 Rust CLI／CI 权威持久 inventory 负责。两 API 按 Cargo.lock 在隔离 Linux 编译、核验 ELF 后 runtime 装箱；默认 API Dockerfile 的源码 builder 未执行。同一 daemon 的 pull 前确认本轮候选镜像身份已清除，基础层缓存可能仍在；本结果不声称跨机器空缓存。远端 Actions、外部 registry 发行与生产部署未执行。前端回退须保留旧版本路由与资源，API／数据库独立升级。
