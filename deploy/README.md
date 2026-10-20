# 工单应用：平台初始化、授权与独立发布

这里使用[独立 Rust 管理 CLI](../../../tools/platform-admin/README.md)，可执行文件为 `saas-platform-admin`，只使用现有平台 API。旧 Node 管理入口与 SQL 草案输出已退役。通用上传与部署流程见[架构文档](../../../docs/architecture.md#releases)，本文件说明该命令行工具的参数、授权计划和兼容发布流程。平台应用身份固定为 `work-orders`，运行方式固定为 **IFRAME + HTML**；宿主通过 iframe bridge 和平台 SDK 提供短期应用 token。工具不会创建 TRUSTED 应用、读取平台 JWT 签名密钥、重建宿主或默认分配任何租户／组织／角色／成员授权。

根Compose默认服务为Rust平台API、PostgreSQL、宿主静态与独立入口Nginx。Rust工单API已有读写／成员／持久提醒，Phase21前端改为独立Vue／Vite消费者，但未执行生产切流；保留的Node工单兼容路线仍须显式包含[遗留override](../../../deploy/compose.legacy.yml)和legacy profile，以及[遗留变量模板](../../../deploy/compose.legacy.env.example)。通知另由Phase23独立Rust profile提供。镜像由运维预先提供固定digest，legacy override不构建镜像、不启动工单数据库、不发布服务端口、不挂载源码或可写持久卷。业务数据库与账号仍需独立配置。

Phase28 推荐镜像路径现由[业务镜像交付说明](../../../deploy/microapps/README.md)、[业务 Compose override](../../../deploy/compose.business.yml)和[公开变量模板](../../../deploy/compose.business.env.example)提供。它独立部署 Rust 工单 API 与 v1/v2 前端静态容器，复用入口 Nginx 的固定版本路由，不需要平台产物卷。该新 override 与同名 Node legacy 服务不能同时合并；通知仍使用自己的独立 override。平台 bootstrap、菜单和 grant 流程保持下文语义，镜像部署不会自动授权，也不表示生产切流已经执行。

本阶段受管 API 验收使用冻结 Linux 源码独立 locked Cargo 编译、核对 ELF/source/inputs/platform/SHA-256 后 runtime 装箱，候选报告明确记录 `verified-prebuilt-ELF`。默认 [Rust Dockerfile](../backend/Dockerfile)保留独立源码 builder；未实际运行默认 builder 不能记为 PASS。静态前端仍用自己的 lockfile 从源码 Docker build，实际 registry pull、业务及浏览器结论须读取本次 Phase28 报告。

下面仅解析配置，不启动服务；两个环境文件都必须位于受控位置并填完相应占位值：

```sh
docker compose --env-file /secure/platform.env --env-file /secure/platform-legacy.env -f docker-compose.yml -f deploy/compose.legacy.yml --profile legacy config --quiet
```

只有独立 Nginx 发布 HTTPS 入口。现有 `/work-orders-api` 原前缀转发到私网 `work-orders-api:3004`；该 legacy override 只保留工单兼容 API，不表示 Rust 工单（Phase 16–20）已切流。通知已由 Phase 23 的[独立 Rust Compose](../../../deploy/compose.notification.yml)提供，`/socket.io/` 转发到私网 `notification-ws:3003`。通知健康探针需要通过公开 HTTPS 平台验证保护路由，入口不能反向等待通知就绪而形成启动死锁；旧通知容器需按原部署清单精确停止，详见[通知切流说明](../../../mini-services/notification-ws/README.md)。

使用新业务 override 时，同一 `/work-orders-api` 前缀明确指向独立 Rust 镜像。操作者先固定 digest pull/start 并检查真实 Docker health，使用 Rust CLI 对 release/artifact 执行持久 `artifact-bind --apply` 与 `artifact-verify`，再保留旧版本路由地推进 Nginx 配置，最后执行带 `--artifact-file`／`--inventory-dir` 的 register、preflight 和 activate。旧六字段 URL release 定义不添加 image/digest/target；digest 一致性由 CLI/CI journal 拒绝冲突，不能声称平台已保存校验它。已部署 URL 管理入口与 ZIP 兼容发布并存，不改变下文授权及幂等要求。

## 1. 准备管理员上下文和机密客户端

管理员在平台租户上下文执行应用管理操作，至少需要 `system:application`、`system:menu` 和 `system:oauth`。工具使用已有平台 session Bearer；它不自动登录。将以下值通过进程环境或 secret store 注入，避免把凭据写入命令参数、清单、前端构建或版本目录：

| 环境变量 | 用途 |
| --- | --- |
| `PLATFORM_API_ORIGIN` | 平台精确 origin，例如 `https://saas.example.com`；不能带路径、query、fragment 或用户名／密码 |
| `PLATFORM_ADMIN_BEARER` | 具有上述权限的当前平台 session token，仅管理员工具读取 |
| `PLATFORM_CLIENT_ID` | 平台生成的实际机密客户端 ID；业务 API 也使用该 ID |
| `PLATFORM_CA_FILE` | 可选的公开 PEM CA 链文件，最大 64 KiB；保持 TLS 及主机名验证，不接受私钥 |
| `PLATFORM_CLIENT_SECRET` | 业务 API 从服务器环境读取；**管理员工具不读取这个变量**，前端不能持有 secret |

先在平台 OAuth 客户端管理界面创建一个客户端：

| 字段 | 值 |
| --- | --- |
| clientName | `work-orders-client` |
| clientType | `CONFIDENTIAL` |
| grantTypes | 仅 `client_credentials` |
| scopes | 仅 `openid` |
| redirectUris | 空列表 |
| accessTokenValidity / refreshTokenValidity | 均为 300 秒 |
| applicationId | 初次创建为 `null`，随后由本工具关联 |

现有 `POST /api/oauth/clients` 不支持指定 `clientId` 或传入 secret，会生成带随机后缀的客户端 ID，并仅在创建响应中返回一次明文 secret。**`work-orders-client` 是客户端名称，不能当作生成的实际 ID。** 管理员立即把一次性 secret 保存到业务服务的 secret store，记录实际 ID 并配置 `PLATFORM_CLIENT_ID`。本工具不创建／轮换客户端，避免丢失或输出这份一次性凭据。

`client_credentials` 是平台创建客户端所需的配置；业务 API 的用户请求仍必须使用宿主取得的应用 token，通过该机密客户端逐请求内省，再读取 `/api/apps/work-orders/context`。服务凭据、平台 session 和其他应用 token 都不能冒充工单用户 token。

生产和其它远程环境使用 HTTPS，Rust HTTP 客户端的正常 TLS 证书验证保持开启。开发 HTTP 仅接受字面回环来源 `localhost`、`127.0.0.1`、`[::1]`，且每条命令必须同时带 `--environment development --allow-loopback-http`；仅改环境名称不会允许 HTTP。

## 2. 检查并执行应用 bootstrap

先在仓库根目录构建独立 crate，再将 `tools/platform-admin/target/debug/saas-platform-admin`（Windows 为 `.exe`）的绝对路径加入受控 PATH，或用该绝对路径替换下文命令。此 crate 使用自己的 Cargo.lock，不要求根 Cargo workspace：

```powershell
cargo build --manifest-path tools/platform-admin/Cargo.toml --locked
```

默认命令和没有 `--apply` 的写命令只输出本地计划，不读取网络凭据、不发起网络请求：

```powershell
saas-platform-admin --help
saas-platform-admin
saas-platform-admin bootstrap --environment production
```

先审阅 [Rust 内嵌的 bootstrap 清单](../../../tools/platform-admin/manifests/bootstrap.manifest.json)和[严格 schema](../../../tools/platform-admin/schemas/bootstrap.schema.json)。本目录的 [bootstrap.manifest.json](bootstrap.manifest.json) 保留为公开兼容 source，并与 Rust 清单同步。CLI 不接收外部 bootstrap 清单。它定义唯一应用、默认空授权和七个稳定 key 的节点：可见的 `/micro/work-orders` 根 MENU、隐藏的详情 MENU、可见的创建 MENU，以及四个不可见 BUTTON：

| key | 类型与路径 | 功能权限与显示 |
| --- | --- | --- |
| `root` | MENU `/micro/work-orders` | 空功能权限，仅授权应用列表入口 |
| `ticket:read` | MENU `/micro/work-orders/tickets/:id` | `ticket:read`，隐藏在导航中，直达详情仍需显式授权 |
| `ticket:create` | MENU `/micro/work-orders/ticket-create` | `ticket:create`，导航中可见 |
| `ticket:update/delete/export`、`notification:publish` | BUTTON，空路径 | 各自独立操作权限，导航中隐藏 |

创建路径不位于 `/tickets/:id` 动态详情路径下；旧 `/micro/work-orders/tickets/new` 不作为创建别名，当前前端只将 `new` 视为详情标识。根菜单和详情读菜单都不隐含创建 MENU 授权。仅选择 `ticket:create` 的授权计划也不会增加详情读 MENU。

| 资源 | 操作权限 | 独立数据授权 |
| --- | --- | --- |
| `ticket` | `ticket:read/create/update/delete/export` 五项分别配置 | 同名 `read/create/update/delete/export` action，分别显式选 scope |
| `notification` | `notification:publish` | `publish` action，单独显式选 scope |

根菜单没有操作权限；授权入口菜单不会隐含 `ticket:read`。所有按钮（包括提醒）使用 `visible:false`；MENU 可达性、功能权限和独立数据 grant 分别检验，菜单授权不补数据 grant。创建应用时使用 `autoCreateMenu:false`，避免平台自动菜单和本工具菜单重复。

已有部署若仍将 `ticket:read/create` 登记为 BUTTON，工具会返回 `MENU_CONFLICT`，授权计划与运行校验也要求新 MENU 的精确类型、路径、可见性和打开方式；工具不改写已有节点。采用新规范前，管理员须在受控窗口核对这两个菜单 UUID、所有授权和角色受众，在平台菜单管理中审阅调整其类型／路径／可见性／`MICRO` 打开方式，保留原 UUID、权限和父节点，再重新 inspect。同步构建、登记并启用采用新创建路径的前端版本；旧前端仍可能把 `/tickets/new` 解释为创建，因此不能仅修改菜单，也不能回滚到未采用此路由规范的旧前端。既有不可变版本保留，但采用和回滚对象需重新核对。这里不提供自动迁移、授权修改或生产执行。

准备好环境后查询和执行：

```powershell
saas-platform-admin inspect --environment production
saas-platform-admin bootstrap --environment production --apply
saas-platform-admin inspect --environment production
```

开发回环示例：

```powershell
saas-platform-admin inspect --environment development --allow-loopback-http
saas-platform-admin bootstrap --environment development --allow-loopback-http --apply
```

Bootstrap 的写入范围：

1. 核对唯一 `work-orders` 应用及环境指定的客户端；客户端名称、类型、授权模式、scope、TTL 和现有关联不符合清单时停止。
2. 应用不存在时创建 IFRAME HTML 应用。应用定义启用，但**没有任何授权**；缺省兼容入口 `/sub-apps/work-orders/unreleased/index.html` 应保持 404，不能放置可变生产内容。
3. 仅把指定、当前未绑定的客户端关联到本应用；已有正确关联保留。客户端属于其他应用、名称重复，或本应用另有关联客户端时拒绝。
4. 核对已有本应用菜单的 UUID、应用、父节点、类型、路径、权限、可见性和打开方式，只创建缺项。重复／错误归属的菜单停止，不删除或重写已有菜单。
5. 输出应用 UUID、客户端记录 UUID 和七个菜单 UUID；不输出 Bearer、secret、平台 origin 或 API 原始正文。

已有应用的运行方式、入口类型和基础路由必须匹配；名称等展示字段不会自动改写。已停用的应用不会被本工具重新启用。可用 `--app-id <已核对的应用UUID>` 额外固定目标身份。

这些 API 没有跨应用／OAuth／菜单事务、条件更新或菜单唯一键，**需要一个受控管理窗口，只运行一份 bootstrap**。工具在关联前再次读客户端，但这不构成原子比较更新。失败可能留下部分已创建项；不会通过删除回滚，避免撤销已有授权。网络写请求不自动重试：先 `inspect` 核对实际结果，再重新运行显式命令；正确已有项会被复用。

## 3. 明确租户、组织、角色、成员和数据范围

Bootstrap 不隐含授权。复制 [grants.manifest.example.json](grants.manifest.example.json)，用 bootstrap 输出替换应用／菜单 UUID，再按批准对象添加 `targets`。模板 UUID 刻意无效，必须先完成核对；`targets:[]` 意味着没有授权操作。

每个 target 必须显式提供下面的完整结构；这些占位符必须替换为真实且核对过的 UUID：

```json
{
  "tenantId": "<租户UUID>",
  "organizationId": "<该租户组织UUID>",
  "roleId": "<该组织专用工单角色UUID>",
  "userId": "<用户UUID>",
  "membershipId": "<用户在该组织现有直接成员关系UUID>",
  "permissions": ["ticket:read", "ticket:create"],
  "dataGrants": [
    {"resource":"ticket","action":"read","scope":"SELF"},
    {"resource":"ticket","action":"create","scope":"SELF"}
  ]
}
```

只增加批准的 action；不隐含更新、删除、导出或提醒。发送提醒须另外声明 `notification:publish` 权限和 `notification/publish` 数据 grant。scope 枚举为 `SELF | ORG | ORG_SUBTREE | TENANT`，实际可授值还必须在平台 `DATA_GRANT_POLICY` 显式登记的本应用 resource/action/scopes 内，并受目标组织 ceiling 约束；scope 不是可以直接按枚举次序比较的大小关系。工单 SELF 以创建人／归属人为准，处理人身份不扩大范围。功能权限和数据范围互相独立，未给对应 grant 时业务 API 拒绝，缺省不授 `TENANT` 或其它 scope。

日常数据授权使用正式管理 API。`DATA_GRANT_POLICY` 由平台运维显式配置应用能力和允许管理的目标租户／组织根范围；平台根身份或管理员自身业务 grant 都不能代替这份边界。`system:data-grant:read` 与 `system:data-grant:write` 是两项独立 BUTTON 权限，宽泛用户／角色／菜单／授权管理权不与它们等价。GET 需要 read；preview 需要 read + write，PATCH 需要 write，两个变更入口还都要求有效平台根上下文。业务用户的成员查询和工单操作不需要 grant 管理权。

全新开发/生产 bootstrap 在既有 `system:authorization` 菜单下登记隐藏 read/write BUTTON，并显式授予平台管理员角色、平台租户和根组织。已有安装不自动添加菜单或补授权，应由授权管理员使用既有菜单和授权 API 显式采用；普通组织管理员不会因此获得写权。配置形状、独立权限与升级步骤见[平台数据 grant 管理契约](../../../docs/data-grant-management.md)。

生成可审阅 API 计划；也可先查看受控执行命令的离线计划：

```powershell
saas-platform-admin grants-plan --manifest path/to/approved-grants.json
saas-platform-admin grants-apply --manifest path/to/approved-grants.json
```

这两个示例不读取网络配置、不发 HTTP 请求、不连接 PostgreSQL、不提交授权。`grants-plan` 输出明确租户/组织请求、角色当前集合并集要求和数据 grant 的 GET→preview→PATCH 顺序，标记必须读取实时版本；输出仅是**管理员明确清单的审核产物**。输入遵循[严格 grants schema](../../../tools/platform-admin/schemas/grants.schema.json)，不接受未知字段、凭据、其他应用编码、未批准权限、重复成员或冲突 action/scope。

批准完整目标、专用角色受众与作用域，并暂停同范围并发管理后，明确执行：

```powershell
saas-platform-admin grants-apply --manifest path/to/approved-grants.json --apply
```

CLI 通过正式 API 核对应用、菜单、租户、组织、角色、用户与已存在的直接 membership，再提交明确租户/组织授权及保留当前值的角色菜单/成员并集。它不维护 membership。数据 grant 逐目标读取实时 opaque version，先 preview 并验证精确差量，再使用同一版本 PATCH。此 CLI 仅增加缺项；已有相同 resource/action 的同 scope 保留，不同 scope 返回 `GRANT_SCOPE_CONFLICT`，scope 更新和撤销另走平台 UI/API 审阅。未列出的 action 保留，没有隐式 scope 或缺省 `TENANT`。平台逐请求验证实际平台根、独立 read/write 权限和 `DATA_GRANT_POLICY`，持有管理员 session 不绕过这些要求。角色全量 PUT 没有 CAS，仍须停并发窗口并审阅共享角色全部成员。

下列步骤说明相同正式 API 的审阅与核对规则，也适用于管理员人工执行。多个管理 API 不构成一个跨目标事务；失败可能已提交部分操作，先读实际授权、角色集合、grant 与审计，再审阅后续动作，不能盲目重跑。

管理员依次完成以下操作：

1. 核对组织／角色所属租户、角色所属组织、用户已有该组织直接 membership；相关对象启用且未过期。通过 `GET /api/users/{userId}/memberships` 读取原有范围内的成员记录及其 `membershipId`，匹配 tenantId/organizationId 后使用稳定 UUID。该发现接口仍需要独立的 `system:user` 权限和原有用户管理范围；grant 管理权不自动包含它。不存在时停止部署授权并单独安排成员管理。
2. 记录租户／组织授权、角色菜单／用户集合和该 membership 的现有数据 grants，协调同范围管理员暂停并发修改。专用工单角色优先；给共享角色增加菜单会改变其所有成员的权限，须审阅完整成员受众。
3. 在平台控制台核对计划内菜单都属于 `work-orders`。先提交计划中的租户 `PUT /api/tenants/{id}/authorizations`，再提交组织对应 PUT，body 仅包括工单应用和选中的根菜单／按钮，`authorized:true`。它们按显式项目逐项处理，缺省项不会撤销；但事务末尾的范围一致性清理可能清除存量不一致授权，应前后对比。
4. 角色 `GET /api/roles/{id}/menus` 和 `/users` 后，将批准新增 ID 与当前完整集合取并集再 PUT；这两个 PUT 是**全量替换**，没有 ETag／版本号，不能只传新工单 ID 或使用过期快照。提交后重新 GET，核对所有原有值保留。
5. 对五 UUID 的精确 target 调用 `GET /api/data-grants`，保存响应的 opaque `version` 和现有 `grants`。将离线模板里 `expectedVersion` 的说明占位替换为这次真实响应的 `version`，先 POST `/api/data-grants/preview` 审阅 before/after/changes，再用相同版本 PATCH `/api/data-grants`。预览不写库、不预留版本；返回 409 时重新 GET 和预览，不能猜版本、假定初始版本或照旧版本强行重试。PATCH 使用差量 upsert/remove，未列出的 action 保留；本计划的 remove 默认为空。
6. 用真实非 super 用户进入指定组织，获取工单应用 token，查询 `/api/apps/work-orders/context`，检查 user/tenant/org、六项中的批准权限和独立 grants；随后验证业务接口及拒绝场景，并对比其它应用授权快照。

**不要把成员维护混入自动部署授权。** 当前 `PUT /api/users/{id}/memberships` 对保留项采用差量维护，保持有效 membership UUID；实际删除成员仍会级联删除其数据 grants 并撤销关联会话。缺少成员关系时先由管理员完成成员管理，再重新核对 membership UUID 和批准的数据授权，不能直接沿用旧快照。

父组织角色权限可能继承到子组织；子组织仍须有自己的应用／菜单授权，独立 data grant 只来自当前组织直接 membership，不随角色继承。

API 模板形状如下；五个 UUID 和版本必须来自已核对的实际目标与实时 GET，不能直接提交占位符：

```text
GET /api/data-grants?tenantId=<tenant UUID>&organizationId=<organization UUID>&userId=<user UUID>&membershipId=<direct membership UUID>&applicationId=<work-orders application UUID>
POST /api/data-grants/preview
PATCH /api/data-grants
```

POST/PATCH 使用同一 body 形状：

```json
{
  "target": {
    "tenantId": "<tenant UUID>",
    "organizationId": "<organization UUID>",
    "userId": "<user UUID>",
    "membershipId": "<direct membership UUID>",
    "applicationId": "<work-orders application UUID>"
  },
  "expectedVersion": "<copy GET /api/data-grants response.version>",
  "upsert": [{"resource":"ticket","action":"read","scope":"SELF"}],
  "remove": []
}
```

写入重新验证平台根、独立权限、direct membership 与目标 tenant/org/user 的一致性、应用登记能力和显式管理范围；审计失败与主写一起回滚。PATCH 后重新 GET 并验证下一次受保护业务请求的实际范围。网络响应丢失不代表回滚，先查当前状态与审计，再决定后续动作。

旧 `grantSql` 和 `--sql-out` 已退役，Rust CLI 拒绝该参数，不生成或执行 SQL。授权统一经过正式管理 API 的策略、独立权限、版本与同事务审计检查。

## 4. 发布工单前端版本

Phase21前端使用本目录之外 `../frontend/` 的独立Vue3.5.43／Vite8.3.4锁依赖；Node24.18.0／npm11.16.0只参与构建／验证，生产只运行静态Nginx。先从仓库根执行独立安装及标准Vite构建，实际宿主origin由部署方明确提供：

```powershell
npm --prefix business-apps/work-orders/frontend ci --ignore-scripts
npm --prefix business-apps/work-orders/frontend run build -- --version 1.0.0 --allowed-host-origin https://saas.example.com --api-base /work-orders-api
```

产物采用 `base:'./'`，标准index／模块入口、真实动态chunk、CSS和classic iframe bridge全部相对固定版本目录。`asset-manifest.json`与 `build-info.json`记录真实资源、应用／SDK／Vue／Vite／Node版本、独立lock及source摘要；公开config仅含版本、精确宿主白名单和API地址，不自动注入VITE_*、secret、数据库URL或平台session。dev／preview只用于本地工具，不能作为生产服务；完整配置见 [前端README](../frontend/README.md)。

前端复用仓库打包器、不重建宿主。下面的 `--input` 指向实际完整构建产物，发布目录由管理员作为不可变静态归档部署：

```powershell
node scripts/package-microapp.mjs --app work-orders --environment production --version 1.0.0 --input path/to/work-orders-dist
```

默认生成 `deploy/micro-apps/work-orders/versions/1.0.0/` 和其中的 `release.json`，入口为 `/sub-apps/work-orders/versions/1.0.0/index.html`。已有版本目录不能覆盖，变更使用新版本号；不要发布 `.env`、secret、source token 或其它敏感文件。

将整个版本目录复制到网关对应不可变目录并保留旧版本／chunk。opaque iframe匿名加载ESM、动态chunk、classic bridge和CSS，需要正确MIME和静态 `Access-Control-Allow-Origin:*` 或null；不存在资源必须真实404，不回落入口HTML。入口CSP frame-ancestors／X-Frame-Options须允许宿主，iframe不增加allow-same-origin或下载权限。静态匿名CORS不扩展到业务API：工单请求保持Origin:null、当前应用Bearer、credentials:omit，由Rust生成精确业务CORS，Nginx透传。平台代理预检使用服务器精确origin allowlist，CLI配置不能扩大它，见 [独立发布API](../../../docs/architecture.md#release-contract)。

每一步都是独立命令：

```powershell
saas-platform-admin register --environment production --release-file deploy/micro-apps/work-orders/versions/1.0.0/release.json
saas-platform-admin register --environment production --release-file deploy/micro-apps/work-orders/versions/1.0.0/release.json --apply
saas-platform-admin releases --environment production
saas-platform-admin preflight --environment production --release-id <发布UUID> --apply
saas-platform-admin activate --environment production --release-id <发布UUID> --apply
saas-platform-admin deployments --environment production
```

工具校验 release.json 属于工单版本目录、HTML 入口、声明环境、固定版本资源、无凭据 URL，且生产 URL 使用 HTTPS。同环境同版本已存在时返回 `VERSION_EXISTS`，不会更新／删除旧版本。登记只得到 DRAFT，不会自动预检或启用。

平台预检 HTTP 200 但 `validationStatus=FAILED` 时命令失败为 `PREFLIGHT_FAILED`；原诊断已在平台保存，管理员在发布管理页面查看。本工具不回显包含网络 URL 的原诊断正文。`activate` 要求目标已 PASSED，且平台会再次真实预检。启用、回滚不改变 app、OAuth、权限和 data grants。

发布 `1.0.1` 时重复打包、静态部署、登记、预检，再显式启用；部署指针保留上一版本。显式回滚：

```powershell
saas-platform-admin rollback --environment production
saas-platform-admin rollback --environment production --apply
saas-platform-admin deployments --environment production
```

没有上一版本时返回 `ROLLBACK_UNAVAILABLE`。平台会重新预检上一版本，失败或并发变化保留原部署并返回错误。保留旧版本目录；宿主刷新profile后按新版本ID卸载并挂载，已有实例在刷新前仍使用自己的固定版本入口。

发布／回退前先核对当前页面未确认的创建、版本写入和提醒。Phase21浏览器草稿／事件只在同一存活iframe内按app／tenant／org／user隔离，跨路由／续期／切组织返回可恢复；刷新、关闭或更换发布实例不保证保留，不用opaque localStorage或新SDK存储能力。未知结果应保持页面直至显式核对完成，合法成功DTO校验前不结束原事件；后端0003持久意图和创建receipt不会随前端版本回退清理。旧前端无创建header／恢复协议，不能当当前Rust行为的等价回退，也不能通过删receipt／intent或改迁移checksum解决。

创建 `/micro/work-orders/ticket-create` 与详情 `/micro/work-orders/tickets/:id` 的MENU仍分别授权；新构建、登记、预检和启用不补权限或grant。Phase21实际opaque模块／chunk、Rust API、普通用户操作及未知结果的浏览器结果由 [Phase21](../../../docs/validation/phase-21.md)单独记录；命令和配置说明不表示本轮已操作生产发布或切流。

## 5. 失败与安全测试

HTTP 请求固定在管理员配置的 origin，使用 Rust reqwest/rustls，不采用环境代理、不跟随 redirect、不自动重试；整条远程命令的所有 HTTP 请求共享默认 35 秒总预算，可通过 `--timeout-ms` 收紧到 10–35000 毫秒；后续请求使用剩余预算，JSON 响应上限 1 MiB。错误只打印固定 `category` 和可选 HTTP `status`，不打印 URL、参数、Bearer、secret、正文或异常堆栈。

常见错误：`INVALID_ORIGIN`／`HTTPS_REQUIRED` 为 origin 配置问题；`HTTP_ERROR` 的 401/403 要重新检查会话与管理权限；`APPLICATION_CONFLICT`、`CLIENT_OWNERSHIP_CONFLICT`、`MENU_CONFLICT` 要人工核对归属，不能通过自动重写消除；`REQUEST_TIMEOUT` 表示结果可能已提交，先查看实际状态再决定重试。

```powershell
cargo test --manifest-path tools/platform-admin/Cargo.toml --locked
```

测试使用受控 HTTP fixture、Rust CLI 子进程及离线 manifest 输入检查，覆盖管理协议、冲突拒绝、正式授权 API 和脱敏输出。它们不替代真实平台／PostgreSQL；真实授权、业务数据和版本切换需运行工单端到端测试，并显式提供预构建 `PLATFORM_ADMIN_BIN`。CLI 不运行 Docker 命令或启动服务，前端构建与打包的 Node 工具继续保留。测试数量与结果以当前运行输出为准。
