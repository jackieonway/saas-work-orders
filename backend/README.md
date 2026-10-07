# 工单业务 API

Node 24 ESM 服务，使用自己的 PostgreSQL 数据库。平台用户、组织、角色、数据授权和通知均通过固定平台 HTTPS origin 查询；业务服务不读取平台数据库或平台 JWT 密钥。

## 启动与迁移

1. 为业务服务创建独立数据库及业务数据库账号。
2. 从 `config.env.example` 配置数据库连接、平台 origin 和绑定 `work-orders` 的机密客户端凭据。实际环境文件和客户端密钥不入 Git。
3. 执行 `npm ci`，然后使用 Node 24 的 `node --env-file=<实际配置文件> index.mjs`。

启动先在事务中按顺序执行 `migrations/*.sql`，记录 SHA-256。迁移失败或已执行迁移被修改时停止启动；不跳过错误、不重建数据表。`GET /work-orders-api/health` 检查工单与计数器真实所需 schema 可查询、平台保护上下文路由可达。全部 UP 才返回 200，否则 503；健康检查无需登录。`/work-orders-api/health/live` 独立表示进程存活，业务授权服务故障时受保护请求仍返回 503。

生产平台 origin 必须为精确 HTTPS origin。开发模式只额外允许 loopback HTTP，不允许任意内部 DNS HTTP、重定向或来自浏览器的 origin 覆盖。所有授权 HTTP 请求禁止跟随重定向，默认单次超时 3 秒，响应最多 256 KiB。每个业务请求重新进行机密客户端内省和当前应用上下文查询，不缓存正向授权。

生产还必须设置独立随机 `METRICS_TOKEN`，至少 32 字符，不能是占位值。私有 `/work-orders-api/metrics` 只接受该精确 Bearer，网关拒绝公开指标。正常 HTTPS 信任链或 `NODE_EXTRA_CA_CERTS` 加载本机测试 CA；设置 `NODE_TLS_REJECT_UNAUTHORIZED=0` 会拒绝启动。

HTTP 生成／保留合法 UUID `X-Request-Id`，在响应及平台调用中传递。请求 JSON 日志使用静态路由，仅在应用内省、上下文和 grant 验证通过后记录租户／组织；不记录用户 ID、URL/query、请求体、token、机密客户端或数据库秘密。指标 labels 不包含任何业务身份。配置与本地告警见[可观测性文档](../../../docs/observability.md)。

Docker 构建上下文为本目录。镜像只依赖这里的源码、迁移与 `pg`，不引用宿主仓库其它目录。默认内部端口 3004；网关保留 `/work-orders-api/*` 前缀。

## API 契约

所有受保护接口使用 `Authorization: Bearer <work-orders 应用 token>`。平台 session、其它应用 token 和旧 OAuth 服务 token 无法作为业务身份。客户端不能覆盖租户、组织、归属人、编号、显示名称或时间。

| 请求 | 输入 / 输出 |
| --- | --- |
| `GET /tickets` | `page`、`size`、`q`、`status`、`priority`、`organizationId`；返回 `{records,total,page,size}`，默认 20 条、最多 100 条 |
| `GET /tickets/{id}` | 返回当前范围内的未删除工单 |
| `POST /tickets` | `{title,description?,priority?,assigneeUserId?}`；201；创建为 OPEN、version=1 |
| `PATCH /tickets/{id}` | `{version,title?,description?,priority?,status?,assigneeUserId?}`；200；每次成功更新提升版本 |
| `DELETE /tickets/{id}` | JSON `{version}`；200 `{ok:true}`；软删除并提升版本 |
| `GET /tickets/export.csv` | 相同筛选，独立 `ticket:export` 与 `ticket/export`；最多 1000 条、4 MiB，超限明确拒绝并提示筛选 |
| `GET /assignees` | `action=create\|update`、`organizationId`、`q`、`page`、`size`；最多 50 条，返回 `{records:[{userId,displayName,organizationId}],total,page,size}` |
| `POST /tickets/{id}/remind` | `{idempotencyKey}`；200 返回平台发布结果，独立手动提醒，不改工单或版本 |

上述请求均以 `/work-orders-api` 为前缀。JSON 请求体上限 64 KiB，拒绝未知字段。标题 trim 后 1～200 个 Unicode 字符；描述最多 8192 字符；优先级为 LOW/NORMAL/HIGH。状态只能 OPEN→IN_PROGRESS→RESOLVED，或 RESOLVED→IN_PROGRESS 重开，允许保留相同状态。

工单记录：

```ts
{
  id, number, title, description, priority, status, version, createdAt, updatedAt,
  tenantId, organizationId, organizationName,
  ownerUserId, ownerDisplayName,
  assigneeUserId: string | null, assigneeDisplayName: string | null
}
```

编号由服务器在租户内生成。组织、创建人和处理人显示名称来自经验证的平台响应并保存快照，读取不要求用户拥有用户目录管理权限。

## 授权与处理人

每种操作要求 `ticket:<action>` 和当前直接组织成员的 `ticket/<action>` 独立 grant。SELF 只匹配本租户 `owner_user_id=当前用户`，处理人身份不扩大范围；ORG 只匹配当前组织；ORG_SUBTREE 使用平台有效组织 ID 集合；TENANT 仍强制本租户过滤。缺权限或 grant 返回 403，越界资源与不存在资源都返回 404。

处理人经平台最小应用成员辅助接口校验，是工单所属组织的有效直接成员，且接收组织有当前应用授权。候选目录也要求 create/update 对应操作和 grant。SELF 目录只开放当前组织；用户在其它当前组织进入后，仍可按 SELF 修改其本人的旧组织工单其它字段，同值处理人可以保留，更换处理人须获得目标组织范围授权或切换到该组织。成员名称由服务器验证，客户端不能覆盖。

更新和删除使用行锁与乐观版本，旧版本返回 409；删除后的记录保留在业务库，但列表、详情、修改、提醒和导出均排除。首版没有自动清理任务，正式业务保留期另行确定。

## CSV 与提醒

CSV 包含人类可读编号、组织和人员名称，状态与优先级使用中文。所有字段按 CSV 规则引用；公式前导字符和控制前导字符加单引号中和。数据超过 1000 条返回 400，超过 4 MiB 返回 413，不返回截断的部分导出。iframe 通过宿主已授权 CSV 下载 broker 保存文件。

提醒先要求 `ticket:read` 与 `ticket/read` 访问工单，再要求 `notification:publish` 与 `notification/publish`。目标为处理人，未分派时为创建人，接收组织为工单组织；平台发布接口再次验证原应用 token、范围和接收资格。提醒编号、内容和链接固定，不随标题编辑变化。HTTP 网络或 5xx 结果未知时必须用同一键重试，不自动生成新键；若处理人改变后同键不同目标，平台返回 409，用户应发起新的独立提醒。提醒不重复保存工单，也不引入机器身份。

## 浏览器与错误

默认只为已存在的 opaque iframe 提供 `Origin: null` CORS；允许 Authorization/Content-Type 及 GET/POST/PATCH/DELETE 预检，不使用 cookies 或 credentials。CORS 是传输配置，服务端身份仍由逐请求令牌和授权验证确定。额外 origin 必须逐个配置精确 origin。

错误 JSON 为 `{code,message}`：输入 400、凭证失效 401、操作或数据授权不足 403、不可见资源 404、版本或提醒键冲突 409、容量 413、平台依赖失败 503。错误和日志不输出 token、客户端密钥或数据库连接 URL。

## 测试

`npm test` 包括配置与真实 HTTP/独立 PostgreSQL 验收。真实业务数据库测试读取 `TEST_DATABASE_URL`，只创建并删除自身 UUID 随机库。平台 HTTP fixture 用于精确负向协议测试；完整平台 main、独立平台/业务数据库、真实登录、Socket.IO 和 iframe 联合测试位于上一级 `tests/platform-e2e.mjs`。
