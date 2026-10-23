# 工单 API 契约

以下契约对应正式 Rust 服务，前缀统一为 `/work-orders-api`。路由定义在 `src/http.rs`，DTO 与领域规则分别在 `reads/`、`writes/`、`directory/`、`reminders/`；Node 兼容实现不能替代该接口说明。

## 身份、请求与响应约束

受保护请求使用单个 `Authorization: Bearer <当前应用token>`，应用身份固定为 `work-orders`。不通过 Cookie、前端传入的租户 ID 或客户端显示名称获得业务身份。后端每次重新验证平台 token 与授权上下文，校验应用、用户、租户、组织、功能权限、资源动作数据授权和有效期限。

数据查询与写入始终受租户硬边界、组织上限和动作授权交集限制。`SELF` 按创建人 `ownerUserId` 判断，处理人 `assigneeUserId` 不扩大范围；`ORG`、`ORG_SUBTREE`、`TENANT` 仍受组织上限约束。未知/越界资源按可见性返回 404，不泄露其他租户工单是否存在。

JSON 字段采用 camelCase，正文为 UTF-8 JSON 对象，写入 `Content-Type` 为 `application/json`，上限 64 KiB。未知字段、重复字段和无效类型拒绝；调用者不能填写 `tenantId`、`ownerUserId`、组织/人员显示快照等权威字段。除列表、导出和成员查询外，详情与写入接口不接受查询参数。

请求可传 `X-Request-Id` UUID，服务回传规范小写 UUID；缺失、重复或格式错误会生成新值。错误正文为 `{ "code": "…", "message": "…" }`，不回显令牌、SQL、连接信息或平台敏感响应。响应使用 `Cache-Control: no-store`，业务请求最长 15 秒；写入超时可能代表结果未知。

qiankun 运行在平台页面中，浏览器 API 请求来源为平台的精确 origin。`WORK_ORDERS_ALLOWED_ORIGINS` 必须显式允许该来源，API 不启用 credentials；预检允许 GET/POST/PATCH/DELETE 与 Authorization、Content-Type、X-Request-Id、Idempotency-Key。配置缺失/为空、`null` 或通配符会导致启动配置加载失败；实际请求使用未授权来源或 `null` 返回 403 `ORIGIN_DENIED`。准确来源不会代替应用 Bearer token 校验。

## 路由与权限

下表路径均加 `/work-orders-api` 前缀。

| 方法 / 路径 | 行为 | 授权 |
| --- | --- | --- |
| `GET /health/live` | 进程存活 | 无业务身份 |
| `GET /health/ready`、`GET /health` | 独立业务库、迁移与 schema 就绪 | 无业务身份 |
| `GET /tickets` | 分页工单 | `ticket:read` + ticket/read 数据授权 |
| `GET /tickets/export.csv` | 相同范围的 CSV | `ticket:export` + ticket/export 数据授权 |
| `GET /tickets/{id}` | 工单详情 | `ticket:read` + ticket/read 数据授权 |
| `POST /tickets` | 幂等创建，成功 201 | `ticket:create` + ticket/create 数据授权 |
| `PATCH /tickets/{id}` | 乐观版本更新，成功 200 | `ticket:update` + ticket/update 数据授权 |
| `DELETE /tickets/{id}` | 乐观版本软删除，成功 `{ "ok": true }` | `ticket:delete` + ticket/delete 数据授权 |
| `GET /assignees` | 当前用途的受控成员目录 | 对应 create/update 功能和数据范围 |
| `POST /tickets/{id}/remind` | 固定事件的通知发布回执 | ticket/read 与 notification/publish 两组授权 |
| `GET /internal/metrics` | 显式启用后的私有指标 | 独立指标 token |

健康检查不证明平台授权或任意用户业务权限正常。指标 token 使用 `WORK_ORDERS_METRICS_TOKEN`，不能复用客户端 secret，也不是应用 token。

## 列表、详情与导出

列表参数：`page` 默认 1、范围 1–1000000；`size` 默认 20、范围 1–100；`q` 去除首尾空白后最多 200 字符；`status` 为 `OPEN|IN_PROGRESS|RESOLVED`；`priority` 为 `LOW|NORMAL|HIGH`；`organizationId` 为规范连字符 UUID。查询总长度最多 8192 字节，未知、重复或无效编码参数拒绝。

列表返回 `{ records, total, page, size }`，按 `createdAt` 倒序、`id` 升序排列。`organizationId` 是额外筛选条件，只能缩小授权结果。详情返回单条工单，主要字段包括 `id`、`number`、`title`、`description`、`priority`、`status`、`tenantId`、`organizationId`、`organizationName`、`ownerUserId`、`ownerDisplayName`、`assigneeUserId`、`assigneeDisplayName`、`version`、`createdAt`、`updatedAt`。未分配处理人时两个 assignee 字段均为 null。

CSV 支持相同筛选，拒绝 `page`/`size`，最多 1000 条，超限返回 400 `EXPORT_LIMIT`，须缩小筛选范围。工单软删除后不出现在列表、详情与导出中。前端导出还需要宿主提供已授权 `downloadCsv` 服务。

## 创建与幂等回执

`POST /tickets` 必须携带单个 `Idempotency-Key` 请求头，值为 1–128 个 ASCII 字母、数字或 `._:-`。正文示例：

```json
{"title":"检查门店设备","description":"补充背景","priority":"NORMAL","assigneeUserId":null}
```

`title` 去首尾空白后非空、最多 200 字符；`description` 默认空串、最多 8192 字符；`priority` 默认 `NORMAL`；处理人可缺省或 null，非空时必须为有效 UUID。服务端设置创建者、租户、当前组织、人员显示快照、初始状态 `OPEN` 和版本。

同租户、组织、用户和创建动作域中，同键同规范正文返回最初的 201 历史回执；同键不同正文返回 409 `IDEMPOTENCY_CONFLICT`。规范正文会统一标题空白、默认值与 UUID 大小写，因此 JSON 字段顺序不影响摘要。重放仍检查当前创建授权，旧键不能让另一身份或组织读取历史结果。历史创建回执不证明记录此后未修改、未删除，或旧处理人仍有效。

503 `CREATE_RESULT_UNKNOWN`、网络断开或无法确认的成功响应都需要保留首次 key 与完整正文，由用户显式恢复同一事件；不得换键再次创建。当前前端只在当前微应用内存中保存事件，刷新、卸载或关闭页面不保证保留恢复信息。

## 版本更新、删除与处理人

`PATCH` 正文需要 `version` 和至少一个字段：`title`、`description`、`priority`、`status`、`assigneeUserId`。`DELETE` 正文只允许 `{ "version": 1 }`。version 为 1–2147483646 的整数语义 JSON 数字，成功写入递增版本；省略 update 字段保持原值，处理人显式 null 表示清空。

允许的状态变化为 `OPEN → OPEN/IN_PROGRESS`、`IN_PROGRESS → IN_PROGRESS/RESOLVED`、`RESOLVED → RESOLVED/IN_PROGRESS`。不能从 OPEN 直接跳到 RESOLVED。409 `VERSION_CONFLICT` 后读取最新详情，由用户确认并选择新版本继续，不能静默覆盖。

`GET /assignees` 必须传 `action=create|update`；可选 `organizationId`、`page`、`size`、`q`。page 默认 1，最大 1000000；size 默认 20，最大 50；q 在 trim 前最多 100 字符。返回 `{ records, total, page, size }`，成员项为 `{ userId, displayName, organizationId }`，不提供平台完整用户表。

创建非空处理人或真正变更分配时需实时校验成员。服务先完成业务范围内的非锁定读取并释放连接，再查平台有效成员，最后进入短事务锁定并复核 version、组织与原值。省略或保留同一处理人可沿用当前存储快照，不能用历史目录列表替代新成员校验。成员窗口从平台 HTTP 请求开始计 `min(PLATFORM_AUTH_TIMEOUT_MS, 3000ms)`，不因等待数据库锁而续期；提交前过期回滚并返回 `MEMBER_VALIDATION_EXPIRED`，提交后才无法确认有效性时按结果未知处理。平台库与工单库之间没有跨库原子事务。

503 `WRITE_RESULT_UNKNOWN` 或网络/协议未知时，先读取当前详情核对版本与状态，再决定后续操作。读取 404 还可能代表可见性改变，不能单凭它证明删除已提交。

## 提醒与未知发布结果

`POST /tickets/{id}/remind` 正文只含 `{ "idempotencyKey": "原始事件键" }`，键规则与创建相同，但独立于创建请求头。后端同时校验工单读取与通知发布授权，目标固定为当前处理人，未分配时为创建人；类型、标题、内容和工单深层链接由服务生成，调用者不能覆盖收件人或正文。

后端先保存不可变提醒意图，确认事务并释放锁，再调用平台发布接口，不修改工单 version。成功返回 `publicationId`、`recipientCount: 1`、`deliveryCount: 1`、`duplicate`、`createdAt`。客户端仍应从平台权威收件箱核对；实时通知服务未运行不代表持久发布失败，已删除的收件箱消息不会因重放重新出现。

503 `REMINDER_PREPARATION_UNKNOWN` 表示准备阶段未确认、本次尚未尝试平台发布，仍须用原事件核对准备结果；`REMINDER_RESULT_UNKNOWN` 表示已尝试发布而结果未知。两者均要求保留原键和固定内容并显式恢复。目标或内容发生变化可能得到 409 `IDEMPOTENCY_CONFLICT`，先读取确认，不换键补发可能已经落库的通知。

## 错误与运行边界

401 `INVALID_APP_TOKEN` / `INVALID_CONTEXT` 要求停止当前身份请求并等待平台刷新；403 `OPERATION_DENIED` / 数据授权拒绝不能通过隐藏按钮绕过；404 不证明跨租户资源的存在性；409 由用户处理冲突；503 `PLATFORM_UNAVAILABLE` 等故障按失败关闭。所有写入都要结合上文的未知结果分类，不能统一自动重试。

启动迁移、配置和健康检查见[后端说明](README.md)，平台联调与数据库专项测试见[开发指南](docs/development.md)，版本发布和数据库恢复见[部署指南](docs/deployment.md)。迁移历史、创建回执与提醒意图必须保留，不得删历史或修改 checksum 以启动旧构建。
