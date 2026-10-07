# 独立工单业务试点

本应用按已批准的[业务提案](../../docs/business-pilot-proposal.md)实现。平台管理用户、租户、组织、操作权限、独立数据授权、应用 token、通知和发布指针；工单服务使用自己的 PostgreSQL 保存实际业务数据。Vue 前端通过默认 opaque iframe 和 MicroProps 2.0 bridge 接入宿主。

## 部署组成

| 组件 | 位置或入口 | 配置边界 |
| --- | --- | --- |
| Vue 业务前端 | `frontend/`，公开版本 `/sub-apps/work-orders/versions/<version>/` | 仅静态资源、精确宿主 origin、业务 API 地址和版本标识 |
| 独立 Node 24 API | `backend/`，`/work-orders-api/` | 独立业务 DB、平台 HTTPS origin、机密客户端 ID/secret |
| 业务 PostgreSQL | 独立数据库和账号 | 不读取平台表，不复用平台数据库 URL |
| 平台 bootstrap/授权/发布工具 | [deploy/README.md](deploy/README.md) | 当前平台管理员 session，明确应用及授权对象，分步执行 |
| 持久通知实时服务 | [notification-ws](../../mini-services/notification-ws/README.md) | 平台 DB 与内部 session 校验，真实收件箱为权威来源 |

客户端不持有 OAuth client secret 或 JWT 签名密钥。业务 API 对每个受保护请求内省应用 token，再读取当前应用上下文；功能权限与 `ticket/<action>` 数据 grant 分别检查。SELF 按创建人／归属人判断，处理人身份不扩大范围。租户和组织归属从可信上下文取得，客户端不能覆盖或搬移。

## 1. 管理员登记与授权

先按[管理员操作手册](deploy/README.md)创建机密客户端、登记 `work-orders` 应用和七个入口／操作菜单。`work-orders-client` 是客户端名称；平台生成的实际 ID 配置为业务服务的 `PLATFORM_CLIENT_ID`。

租户、组织、角色、用户和当前直接 membership 的数据 grant 必须逐项明确。Bootstrap 不默认分配授权或标记 TRUSTED。授权草案仅产出可审阅清单／SQL；已有角色菜单和用户集合的管理 API 是全量替换，提交前需要读取完整当前集合并保留已有值。不要通过重建 membership 补授权，其 UUID 变化会级联删除现有数据 grants。

## 2. 独立业务 API 和数据库

直接部署 Node 服务见[业务 API 配置与操作说明](backend/README.md)。服务自动执行自身业务迁移后启动；平台迁移继续由平台 backend 管理。新建数据库、数据库账号和生产权限需按部署环境明确配置，不向平台库安装业务表。

仓库 Compose 提供两个显式 profile：

- `work-orders`：独立 Node 24 业务 API，只暴露容器内 3004；可连接已经部署的独立业务数据库。
- `work-orders-local-db`：可选独立 PostgreSQL 容器与 `work_orders_pgdata` 卷，仅在需要本地业务库时启用；不发布数据库端口，没有默认演示密码。

将 [compose.env.example](deploy/compose.env.example) 的值配置进根目录 Git 忽略的 `.env.docker` 或受控 secret store。Compose 使用 `WORK_ORDERS_PLATFORM_CLIENT_ID/SECRET` 映射容器内 `PLATFORM_CLIENT_ID/SECRET`；数据库必须显式给出 `WORK_ORDERS_DATABASE_URL`。缺少配置时业务服务拒绝启动。

```powershell
# 已有独立业务数据库。
docker compose --env-file .env.docker --profile work-orders up -d --build work-orders-api
# 使用独立业务 PostgreSQL 容器；DB URL 中的 host 是 work-orders-postgres。
docker compose --env-file .env.docker --profile work-orders --profile work-orders-local-db up -d --build work-orders-postgres work-orders-api
```

生产容器固定 `NODE_ENV=production`，平台 origin 必须是实际 HTTPS 网关，TLS 证书正常验证，不将 `http://backend:3001` 作为生产内省地址。公开宿主、应用版本入口和 API 使用 HTTPS。根 Caddy 示例监听 HTTP：生产需将站点设为已确认域名、移除 `auto_https off`、开放／映射 443，并持久化 Caddy `/data` 与 `/config`；前置 TLS 或 Nginx 部署需要对应证书和 HTTPS 转发设置。`PUBLIC_APP_URL` 单独设置不会自动配置 TLS。

网关将 `/work-orders-api` 和 `/work-orders-api/*` 保留完整前缀转发到业务 API；未知路径返回 API 404，不进入宿主 HTML。Caddy 上游为 `work-orders-api:3004`，Nginx 独立部署示例为 `127.0.0.1:3004`。服务未部署时该路由不可用，业务 profile 不随基础平台默认启动。

Opaque iframe 的业务请求携带字面 `Origin:null` 和短期应用 Bearer，默认业务 CORS 只允许该 origin，不启用浏览器 cookie 或 credentialed CORS。其他精确 origin 需显式配置 `WORK_ORDERS_ALLOWED_ORIGINS`。公开不可变静态版本保留已有匿名 CORS `*`，该规则不扩展到业务 API、OAuth 或 Socket.IO。

## 3. 独立构建、发布与回滚

前端构建自身代码、Vue runtime 和 iframe SDK，不使用 `frontend/public/sub-apps`。明确配置实际宿主 origin，并使用新版本号生成新构建：

```powershell
cd business-apps/work-orders/frontend
npm ci --ignore-scripts
node scripts/build.mjs --version 1.0.0 --allowed-host-origin https://saas.example.com --api-base /work-orders-api
cd ../../..
node scripts/package-microapp.mjs --app work-orders --environment production --version 1.0.0 --input business-apps/work-orders/frontend/dist
```

打包器生成 `deploy/micro-apps/work-orders/versions/1.0.0/` 和该版本的 `release.json`。将此完整目录放到只读版本根目录后，按管理员手册分别执行 register、preflight 和 activate。登记只得到 DRAFT；工具不自动激活、不改变 trust 或授权。发布新版本时重新构建并使用新版本目录；已有版本不可覆盖。

回滚使用平台部署的上一版本指针并重新预检。保留旧版本与注册清单，归档／删除只按批准的保留方案由管理员明确执行；本应用没有自动清理版本或软删除工单的作业。工单前端版本变化不重建宿主、不迁移业务数据，也不改变应用身份、OAuth 客户端和授权。

## 4. 验证与运维边界

后端真实 PostgreSQL/HTTP 测试、前端实际浏览器测试、管理工具安全测试与实际平台联合 E2E 分别报告其覆盖范围。网关测试 `tests/micro-release-gateways.mjs` 使用真实 Caddy/Nginx 与明确的传输 fixture，覆盖静态版本、Socket.IO 和工单 API 前缀／正文／查询／404；它不替代实际授权验收。

实际平台联合测试位于 `tests/platform-e2e.mjs`，显式提供最新平台 main、隔离测试数据库连接、Nginx、生产宿主构建和现有 Playwright/Chromium 后运行。测试创建两座随机自有数据库，使用实际平台迁移与登录、业务迁移与 CRUD、通知 PostgreSQL 与 Socket.IO，再由实际 Vue iframe 验证操作和 v1→v2→回滚；测试只关闭／删除本次创建的资源。缺少显式运行配置时明确跳过；已配置但路径无效或运行失败则报告失败。

联合测试在 Linux Node 24 下运行，所有服务进程与浏览器使用同一系统的可执行文件，便于严格跟踪自有 PID。先准备当前宿主的 `frontend/dist`、工单前后端依赖和本机运行时，再从仓库根目录执行：

```bash
export WORK_ORDERS_E2E_BACKEND_BINARY=/absolute/path/to/dante-cloud-backend
export WORK_ORDERS_E2E_HOST_DIST=/absolute/path/to/frontend/dist
export NGINX_BIN=/absolute/path/to/nginx
export PLAYWRIGHT_MODULE=/absolute/path/to/node_modules/playwright
export BROWSER_EXECUTABLE=/absolute/path/to/chromium
node --env-file=.env.test --test business-apps/work-orders/tests/platform-e2e.mjs
```

`.env.test` 提供 `WORK_ORDERS_E2E_PLATFORM_DATABASE_URL` 或 `TEST_DATABASE_URL`；可另外提供 `WORK_ORDERS_E2E_BUSINESS_DATABASE_URL`，未给时在同一已授权测试实例中创建另一座独立业务库。维护账号需能创建／删除本次自有测试数据库。两座库都使用随机名称，测试中不复用维护 URL 所指数据库保存平台或工单数据。Nginx 需要可用的 `mime.types`，特殊路径可通过 `NGINX_MIME_TYPES` 指定；跨平台缓存依赖场景可显式提供匹配 esbuild wrapper 版本的 `ESBUILD_BINARY_PATH`。

视觉检查可启用 `WORK_ORDERS_E2E_VISUAL_QA=1`，截图保存到 Git 忽略的 `target/phase11-runtime/qa-<本次随机目录名>/`；其中 ownership.json 记录本次自有目录。服务、数据库、浏览器及临时文件照常清理，截图保留至审查人员明确确认后，仅删除该次已核对归属的 QA 目录，不因计时结束而丢弃截图。Linux 缺少中文字体时，可用 `WORK_ORDERS_E2E_FONT_DIR` 指向已有本地字体目录；测试生成自有 fontconfig 配置与缓存，浏览器 profile、缓存和下载也放在自有临时目录。不要把测试字体或 runtime cache 打包为业务前端资源。

测试保留平台现有内省每分钟 30 次限制，按约 2.1 秒间隔执行受保护业务请求；应用 token 仍通过真实接口续签。完整范围与浏览器／发布场景需要数分钟。2026-10-07 正常全量联合验收 16/16 通过、0 fail／cancel／skip，100 个宿主文件在 v1→v2→回滚过程中哈希不变；具体版本、组件证据及自有资源清理见[验收记录](../../docs/phase11-validation.md)。缺少配置后的跳过不视为通过。

用户已于 2026-10-02 确认[本机隔离 Linux 原生准生产交付方案](../../docs/delivery-acceptance-proposal.md)：Phase 12 实际验证 HTTPS、健康／指标、故障告警、加密备份与新库恢复；RPO 24 小时／RTO 1 小时为演练目标，不承诺商业 SLA，不自动清理业务／通知／审计／会话数据。正式现网域名与主机、密钥管理接入及保留期在现网部署时另行确认。当前机器没有 Docker daemon；Compose 配置检查不计作容器构建或启动证据，实际容器运行留待 runner。Phase 11 的门禁和完整联合验收结果见[工单试点验收记录](../../docs/phase11-validation.md)。
