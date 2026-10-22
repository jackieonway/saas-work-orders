# 工单 API 契约

服务前缀 /work-orders-api，业务身份为 work-orders。路由在 src/http.rs，DTO 与领域规则在 src 下各模块，数据库增量历史在 migrations。

## 通用要求

受保护请求使用当前应用 Authorization: Bearer，不使用 Cookie。后端逐请求验证应用身份、用户/租户/组织、期限、permission 与资源动作数据授权，所有查询与写入都与租户、组织硬上限相交；处理人不会扩大 SELF owner 范围。

请求/响应携带 X-Request-Id；业务前端使用合法 UUID。默认 opaque iframe 来源为字面 null，精确 CORS 不开放 credentials。错误不能回显 token、SQL、连接或上游敏感正文。

## 路由

| 方法与路径 | 用途 |
| --- | --- |
| GET /health/live | 进程存活 |
| GET /health/ready、/health | 数据库与迁移就绪 |
| GET /tickets | 按当前范围筛选和分页 |
| GET /tickets/export.csv | 相同授权范围内的 CSV |
| GET /tickets/{id} | 当前范围内详情 |
| POST /tickets | 幂等创建 |
| PATCH /tickets/{id} | 带当前 version 更新 |
| DELETE /tickets/{id} | JSON 正文带 version 软删除 |
| GET /assignees | create/update 用途的受控成员目录 |
| POST /tickets/{id}/remind | 持久提醒意图与平台通知发布 |
| GET /internal/metrics | 显式启用后的私有指标 |

表中路径均加服务前缀。读取 ticket/read，写入分别要求对应 create/update/delete；精确功能菜单、资源动作和可用范围由平台登记，不能只凭可见按钮放行。

## 幂等创建

POST /tickets 使用单个 Idempotency-Key 请求头，1–128 个 ASCII 字母、数字或 ._:-。正文含 title、可选 description、priority、assigneeUserId；后端按当前规则规范化后固定摘要。

同租户/组织/用户/创建域下，同键同正文返回原 201 历史结果，同键不同正文 409。重放仍重新检查当前创建授权，不能凭旧键获取其他身份或组织的记录。原创建响应不证明工单此后未被修改、删除或处理人仍有效。

CREATE_RESULT_UNKNOWN 或网络/协议未知时，保留首次键与完整正文，用户明确恢复原事件；不能换键重新创建。前端事件只保存在仍存活 iframe 内存，刷新或销毁不保证保留。

## 版本写入与成员

PATCH/DELETE 使用当前 version。冲突 409 时读取最新详情，由用户决定如何继续，不静默覆盖。WRITE_RESULT_UNKNOWN 时先读回版本/状态，可见性 404 不能单独证明删除已提交。

创建非空处理人、真正变更分配需要受控成员校验。先在业务范围内非锁定读取并释放连接，再查平台成员，短事务锁定后复核 version、组织与原值。省略或保留同一成员按当前协议保存快照，不用历史目录结果代替新成员验证。

成员有效窗口从 HTTP 发起开始计 min(PLATFORM_AUTH_TIMEOUT_MS,3000ms)，不在取得锁后续期。提交前过期回滚并报 MEMBER_VALIDATION_EXPIRED；提交后才确认期限过期时按结果未知处理。平台库与业务库不具备跨库原子性。

## 提醒与恢复

提醒正文仅含 idempotencyKey，独立于创建 header。当前授权同时要求 ticket/read 与 notification/publish，目标固定为处理人或创建人。后端先保存不可变提醒意图、确认事务并释放锁，再调用平台正式发布接口，不改工单 version。

REMINDER_PREPARATION_UNKNOWN 表示本次未尝试平台发布，仍须按原事件核对准备结果；REMINDER_RESULT_UNKNOWN 表示发布结果未知，复用原键和固定内容显式恢复。换目标/内容冲突需先读取确认，不能换键补发可能已落库的通知。

成功回执校验 publicationId、计数、duplicate 和 createdAt 后，再从平台权威收件箱核对。实时服务未运行不代表持久发布失败，删除后的收件箱消息不因重放重新出现。

## 迁移与运维

启动验证既有 Node SHA-256 与 SQLx 历史、实际 schema、receipt 和 reminder intent，不重放已执行迁移。不改 checksum、删 intent 或倒退 schema 使旧二进制启动；回退使用兼容当前历史的构建。普通健康检查不验证所有用户权限，具体运行见 [开发](docs/development.md)和[部署](docs/deployment.md)。
