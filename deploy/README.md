# 工单应用：平台初始化、授权与独立发布

这里的 Node 24 管理工具只使用现有平台 API。通用上传与部署流程见[架构文档](../../../docs/architecture.md#releases)，本文件说明该命令行工具的参数、授权草案和兼容发布流程。平台应用身份固定为 `work-orders`，运行方式固定为 **IFRAME + HTML**；宿主通过 iframe bridge 和平台 SDK 提供短期应用 token。工具不会创建 TRUSTED 应用、读取平台 JWT 签名密钥、重建宿主或默认分配任何租户／组织／角色／成员授权。

## 1. 准备管理员上下文和机密客户端

管理员在平台租户上下文执行应用管理操作，至少需要 `system:application`、`system:menu` 和 `system:oauth`。工具使用已有平台 session Bearer；它不自动登录。将以下值通过进程环境或 secret store 注入，避免把凭据写入命令参数、清单、前端构建或版本目录：

| 环境变量 | 用途 |
| --- | --- |
| `PLATFORM_API_ORIGIN` | 平台精确 origin，例如 `https://saas.example.com`；不能带路径、query、fragment 或用户名／密码 |
| `PLATFORM_ADMIN_BEARER` | 具有上述权限的当前平台 session token，仅管理员工具读取 |
| `PLATFORM_CLIENT_ID` | 平台生成的实际机密客户端 ID；业务 API 也使用该 ID |
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

生产和其它远程环境使用 HTTPS，Node 的正常 TLS 证书验证保持开启。开发 HTTP 仅接受字面回环来源 `localhost`、`127.0.0.1`、`[::1]`，且每条命令必须同时带 `--environment development --allow-loopback-http`；仅改环境名称不会允许 HTTP。

## 2. 检查并执行应用 bootstrap

在仓库根目录执行。默认命令和没有 `--apply` 的写命令只输出本地计划，不发起网络请求：

```powershell
node business-apps/work-orders/deploy/platform-admin.mjs --help
node business-apps/work-orders/deploy/platform-admin.mjs
node business-apps/work-orders/deploy/platform-admin.mjs bootstrap --environment production
```

先审阅 [bootstrap.manifest.json](bootstrap.manifest.json)。它定义唯一应用、默认空授权、一个可见的 `/micro/work-orders` 根菜单，以及其下六个不可见 BUTTON 权限节点：

| 资源 | 操作权限 | 独立数据授权 |
| --- | --- | --- |
| `ticket` | `ticket:read/create/update/delete/export` 五项分别配置 | 同名 `read/create/update/delete/export` action，分别显式选 scope |
| `notification` | `notification:publish` | `publish` action，单独显式选 scope |

根菜单没有操作权限；授权入口菜单不会隐含 `ticket:read`。所有按钮（包括提醒）使用 `visible:false`，操作权限仍由平台和业务服务检验。创建应用时使用 `autoCreateMenu:false`，避免平台自动菜单和本工具菜单重复。

准备好环境后查询和执行：

```powershell
node business-apps/work-orders/deploy/platform-admin.mjs inspect --environment production
node business-apps/work-orders/deploy/platform-admin.mjs bootstrap --environment production --apply
node business-apps/work-orders/deploy/platform-admin.mjs inspect --environment production
```

开发回环示例：

```powershell
node business-apps/work-orders/deploy/platform-admin.mjs inspect --environment development --allow-loopback-http
node business-apps/work-orders/deploy/platform-admin.mjs bootstrap --environment development --allow-loopback-http --apply
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

工具不自动写授权。复制 [grants.manifest.example.json](grants.manifest.example.json)，用 bootstrap 输出替换应用／菜单 UUID，再按批准对象添加 `targets`。模板 UUID 刻意无效，必须先完成核对；`targets:[]` 意味着没有授权操作，也没有 SQL 写入。

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

只增加批准的 action；不隐含更新、删除、导出或提醒。发送提醒须另外声明 `notification:publish` 权限和 `notification/publish` 数据 grant。scope 只能是 `SELF | ORG | ORG_SUBTREE | TENANT`；工单 SELF 以创建人／归属人为准，处理人身份不扩大范围。功能权限和数据范围互相独立，未给对应 grant 时业务 API 拒绝。

生成可审阅计划和 SQL 草案：

```powershell
node business-apps/work-orders/deploy/platform-admin.mjs grants-plan --manifest path/to/approved-grants.json --sql-out path/to/work-orders-grants-review.sql
```

该命令不读取网络配置、不连接 PostgreSQL、不提交授权；`--sql-out` 只用独占创建写本地文件，已有文件拒绝覆盖。输出的 path/body 是**管理员明确清单的审核产物**，不会执行。输入不接受未知字段、凭据、其他应用编码、未批准权限、重复成员或冲突 action/scope。

管理员依次完成以下操作：

1. 核对组织／角色所属租户、角色所属组织、用户已有该组织直接 membership；相关对象启用且未过期。`GET /api/users/{userId}/memberships` 不返回 membership UUID，DBA 必须按 `userId + organizationId + tenantId` 查得当前 UUID。不存在时停止部署授权并单独安排成员管理。
2. 记录租户／组织授权、角色菜单／用户集合和该 membership 的现有数据 grants，协调同范围管理员暂停并发修改。专用工单角色优先；给共享角色增加菜单会改变其所有成员的权限，须审阅完整成员受众。
3. 在平台控制台核对计划内菜单都属于 `work-orders`。先提交计划中的租户 `PUT /api/tenants/{id}/authorizations`，再提交组织对应 PUT，body 仅包括工单应用和选中的根菜单／按钮，`authorized:true`。它们按显式项目逐项处理，缺省项不会撤销；但事务末尾的范围一致性清理可能清除存量不一致授权，应前后对比。
4. 角色 `GET /api/roles/{id}/menus` 和 `/users` 后，将批准新增 ID 与当前完整集合取并集再 PUT；这两个 PUT 是**全量替换**，没有 ETag／版本号，不能只传新工单 ID 或使用过期快照。提交后重新 GET，核对所有原有值保留。
5. 独立数据 grants 当前没有公共管理写 API。DBA 审阅生成 SQL：它锁定并核对精确 app/menu/tenant/org/role/user/direct membership，仅插入该应用批准的 resource/action/scope，`ON CONFLICT DO NOTHING`；既有同 action 不同 scope 报错，不自动扩权或覆盖。草案默认以 `ROLLBACK` 结束；在确认后由 DBA 显式改最终语句为 `COMMIT`，在受控事务中执行并保留操作审计。不要把 SQL 内容混入数据库凭据文件。
6. 用真实非 super 用户进入指定组织，获取工单应用 token，查询 `/api/apps/work-orders/context`，检查 user/tenant/org、六项中的批准权限和独立 grants；随后验证业务接口及拒绝场景，并对比其它应用授权快照。

**不要把成员维护混入自动部署授权。** 当前 `PUT /api/users/{id}/memberships` 对保留项采用差量维护，保持有效 membership UUID；实际删除成员仍会级联删除其数据 grants 并撤销关联会话。缺少成员关系时先由管理员完成成员管理，再重新核对 membership UUID 和批准的数据授权，不能直接沿用旧快照。

父组织角色权限可能继承到子组织；子组织仍须有自己的应用／菜单授权，独立 data grant 只来自当前组织直接 membership，不随角色继承。

## 4. 发布工单前端版本

前端单独构建，复用仓库打包器；不重建宿主。下面的 `--input` 指向实际构建产物，发布目录由管理员作为不可变静态归档部署：

```powershell
node scripts/package-microapp.mjs --app work-orders --environment production --version 1.0.0 --input path/to/work-orders-dist
```

默认生成 `deploy/micro-apps/work-orders/versions/1.0.0/` 和其中的 `release.json`，入口为 `/sub-apps/work-orders/versions/1.0.0/index.html`。已有版本目录不能覆盖，变更使用新版本号；不要发布 `.env`、secret、source token 或其它敏感文件。

将版本目录复制到网关对应的不可变目录，保留旧版本；确认部署支持 iframe。标准 Vue/Vite 模块在 opaque iframe origin 中匿名获取资源，需要 `Access-Control-Allow-Origin: *` 或 `null`，入口的 CSP frame-ancestors / X-Frame-Options 必须允许宿主。平台代理预检使用服务器配置的精确 origin allowlist；CLI 的配置不能扩大该列表，详细规则见 [应用独立发布 API](../../../docs/architecture.md#release-contract)。

每一步都是独立命令：

```powershell
node business-apps/work-orders/deploy/platform-admin.mjs register --environment production --release-file deploy/micro-apps/work-orders/versions/1.0.0/release.json
node business-apps/work-orders/deploy/platform-admin.mjs register --environment production --release-file deploy/micro-apps/work-orders/versions/1.0.0/release.json --apply
node business-apps/work-orders/deploy/platform-admin.mjs releases --environment production
node business-apps/work-orders/deploy/platform-admin.mjs preflight --environment production --release-id <发布UUID> --apply
node business-apps/work-orders/deploy/platform-admin.mjs activate --environment production --release-id <发布UUID> --apply
node business-apps/work-orders/deploy/platform-admin.mjs deployments --environment production
```

工具校验 release.json 属于工单版本目录、HTML 入口、声明环境、固定版本资源、无凭据 URL，且生产 URL 使用 HTTPS。同环境同版本已存在时返回 `VERSION_EXISTS`，不会更新／删除旧版本。登记只得到 DRAFT，不会自动预检或启用。

平台预检 HTTP 200 但 `validationStatus=FAILED` 时命令失败为 `PREFLIGHT_FAILED`；原诊断已在平台保存，管理员在发布管理页面查看。本工具不回显包含网络 URL 的原诊断正文。`activate` 要求目标已 PASSED，且平台会再次真实预检。启用、回滚不改变 app、OAuth、权限和 data grants。

发布 `1.0.1` 时重复打包、静态部署、登记、预检，再显式启用；部署指针保留上一版本。显式回滚：

```powershell
node business-apps/work-orders/deploy/platform-admin.mjs rollback --environment production
node business-apps/work-orders/deploy/platform-admin.mjs rollback --environment production --apply
node business-apps/work-orders/deploy/platform-admin.mjs deployments --environment production
```

没有上一版本时返回 `ROLLBACK_UNAVAILABLE`。平台会重新预检上一版本，失败或并发变化保留原部署并返回错误。保留旧版本目录；宿主刷新 profile 后按新版本 ID 卸载并挂载，已有实例在刷新前仍使用自己的固定版本入口。

## 5. 失败与安全测试

HTTP 请求固定在管理员配置的 origin，使用 Node 原生 HTTP(S)，不采用环境代理、不跟随 redirect、不自动重试；单次请求总超时 35 秒（覆盖平台 30 秒预检及响应体），JSON 响应上限 1 MiB。错误只打印固定 `category` 和可选 HTTP `status`，不打印 URL、参数、Bearer、secret、正文或异常堆栈。

常见错误：`INVALID_ORIGIN`／`HTTPS_REQUIRED` 为 origin 配置问题；`HTTP_ERROR` 的 401/403 要重新检查会话与管理权限；`APPLICATION_CONFLICT`、`CLIENT_OWNERSHIP_CONFLICT`、`MENU_CONFLICT` 要人工核对归属，不能通过自动重写消除；`REQUEST_TIMEOUT` 表示结果可能已提交，先查看实际状态再决定重试。

```powershell
node --test business-apps/work-orders/deploy/tests/platform-admin.mjs business-apps/work-orders/deploy/tests/grants.mjs
```

测试使用本地 HTTP fixture、CLI 子进程及离线 grant 输入检查，覆盖管理工具协议、冲突拒绝和 SQL 范围。它们不使用真实平台／PostgreSQL，也不执行生成 SQL；真实授权、业务数据和版本切换需运行工单端到端测试。测试数量与结果以当前运行输出为准。
