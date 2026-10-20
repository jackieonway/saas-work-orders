# 工单业务 API

当前服务入口统一为独立 Rust API，包含授权、读写/CSV、创建幂等、成员预校验和持久提醒恢复。Phase34已退役Node index，npm start分派锁定的Cargo命令；Node模块只作为CORS、历史协议及迁移兼容测试参考。构建与运行配置见[Rust工单说明](../../../docs/work-orders-rust-baseline.md)和[独立交付](../../../docs/independent-business-delivery.md)，实际验收见[Phase34](../../../docs/validation/phase-34.md)。

Rust 正式启动要求完整的 `PLATFORM_API_ORIGIN`、`PLATFORM_CLIENT_ID` 与 `PLATFORM_CLIENT_SECRET`，应用编码固定 `work-orders`；只加载业务库的 Config／旧健康 router 兼容路径不构成正式启动条件。授权超时默认3000毫秒为内省和context两次调用的总预算，并发默认32、饱和立即503；每份平台JSON最多256 KiB。精确CORS、可选根CA、上下游requestId、分页与CSV限制统一见上述 Rust 说明。

## Rust 写入契约与恢复

以下路径均有 `/work-orders-api` 前缀，使用当前 `work-orders` 应用Bearer和对应 `ticket:<action>` permission、`ticket/<action>` data grant。内省及context先于业务连接；分配变更先非锁预读，再释放连接做平台成员校验，之后进入写事务，锁内不请求外部HTTP。JSON必须为对象和application/json，最多64 KiB；超限413 `REQUEST_LIMIT`，未知／重复字段、非法类型及写接口查询参数400。

| 请求 | Rust 当前输入与结果 |
| --- | --- |
| `POST /tickets` | 必需单个 `Idempotency-Key` 请求头，1–128个 `[A-Za-z0-9._:-]` 字符；正文 `{title,description?,priority?,assigneeUserId?}`；201返回工单，初始OPEN／version=1 |
| `PATCH /tickets/{id}` | `{version,title?,description?,priority?,status?,assigneeUserId?}`，至少一个可修改字段；200返回工单并提升版本 |
| `DELETE /tickets/{id}` | `{version}`；200 `{ok:true}`，软删除并提升版本 |
| `GET /assignees` | 必需action=create／update，可选organizationId／q／page／size；返回 `{records:[{userId,displayName,organizationId}],total,page,size}` |
| `POST /tickets/{id}/remind` | `{idempotencyKey}`；同时要求当前ticket/read和独立notification/publish授权；200返回经校验的五字段发布回执，不修改工单version |

标题trim后1–200个Unicode字符，描述默认空且最多8192字符，优先级默认NORMAL、仅LOW／NORMAL／HIGH；标题和描述拒绝NUL。除处理人外，显式null不当作省略。version为1–2147483646的整数语义JSON数字（`1.0`／`1e0`可表示1），不接受数字字符串。状态允许保持原值、OPEN→IN_PROGRESS、IN_PROGRESS→RESOLVED及RESOLVED→IN_PROGRESS重开，不允许直接OPEN→RESOLVED或退回OPEN。归属、编号、服务器显示名称与时间均不能由客户端覆盖。

Phase19新非空assignee必须是目标组织当前有效直接成员，由服务使用普通应用Bearer调用平台members HTTP核对UUID／组织／名称，再保存名称快照；不查平台SQL、不使用成员放行替身。创建省略／null不分配；update省略或同UUID保留旧分配及名称，不重查历史成员，真正change／clear才要求目录组织scope，非空change须先平台验证，空字符串无效。SELF仍按owner，可修改可见子组织本人旧记录不代表能改变该组织分配。独立提醒使用下一节的发布和恢复协议。

平台 `DATA_GRANT_POLICY.applications[].memberLookups`须显式列ticket/create和ticket/update，默认空、pair必须已在同应用capabilities内，公开capabilities仍为resource／action／scopes，不新增member grant或assign动作。目录organizationId默认当前组织，page默认1且最多1000000、size默认20且最多50；原始q先限制100个Unicode字符再trim，查询最多8192字节，未知／重复／非法编码参数拒绝。平台返回必须匹配所请求组织和分页，单份JSON仍限256 KiB，保持既有TLS／响应／requestId规则。

更新的scope非锁预读释放连接后再做成员校验；BEGIN／FOR UPDATE时复核组织、version、oldAssignee，错配409 `VERSION_CONFLICT`。成员校验TTL从成员HTTP发起时按单调时钟计算 `min(PLATFORM_AUTH_TIMEOUT_MS,3000ms)`，响应和随后DB等待均消耗窗口，不能取得锁后重置。HTTP本身超时为503 `PLATFORM_UNAVAILABLE`，已取得校验结果在提交前过期为503 `MEMBER_VALIDATION_EXPIRED`，同时复验context；COMMIT后才发现期限过期仍用结果未知错误恢复。跨平台库／业务库没有原子事务，最后验证到COMMIT之间仍有撤权或成员失效竞态。

创建对验证后DTO的固定字段序列计算SHA-256：title trim、description默认空、priority默认NORMAL、assignee省略／null归一，非空UUID规范化；等价默认写法和JSON键顺序不改变摘要，服务器名称不进hash。receipt唯一域tenant／org／user／create／key，工单／计数器／原201同事务保存。同域同键同正文重放原结果、异正文409；已commit重放仍重新验证当前create授权，但不重查历史member，不因旧成员现已失效而把创建历史误当新分配。切组织或失权不能取旧域结果；编辑或软删不改receipt，当前状态另查GET。

| 情况 | 错误与客户端动作 |
| --- | --- |
| 记录版本已变化 | 409 `VERSION_CONFLICT`；读取最新记录后重新审阅修改 |
| COMMIT前授权过期 | 401；当前事务回滚，重新取得授权后再决定操作 |
| 创建COMMIT报错、提交后context／成员校验到期或15秒创建截止 | 503 `CREATE_RESULT_UNKNOWN`；可能已提交，以原键／正文及当前有效凭据对账，不换键 |
| PATCH／DELETE的COMMIT报错、提交后期限到期或15秒写截止 | 503 `WRITE_RESULT_UNKNOWN`；读回当前详情／版本或确认不可见，不自动重试 |

15秒请求总预算包含授权、读取正文、pool等待及事务；连接获取、SQL语句和锁等待仍分别受配置预算约束。成功／失败／预检均保留既有CORS和requestId规则，允许的预检header新增 `Idempotency-Key`。receipt没有自动清理任务，不把客户端取消或503解释成确定未提交。

Phase18新增 [0002](migrations/0002_create_idempotency.sql)，Phase20新增 [0003](migrations/0003_reminder_intents.sql)，0001／0002原字节保持；合法Node／SQLx迁移前缀接管后形成0001／0002／0003双历史，异常历史／结构拒绝。原Phase19及更早严格基线不认识0003，不能直接回退旧二进制；保留工单、receipt、不可变提醒意图及平台publication，核对在途结果并采用兼容构建。Phase19到18的历史成员降级限制、平台旧parser不识别memberLookups的配置处理，均见Rust说明；不能删表、改checksum或清除迁移历史绕过校验。

## Rust 独立提醒与结果恢复

提醒正文仅 `{idempotencyKey}`，键为1–128个 `[A-Za-z0-9._:-]` ASCII字符，与创建header和requestId分别保存。一次当前授权上下文同时要求 `ticket:read`／`ticket/read` 和 `notification:publish`／`notification/publish`，并将两类grant与tenant、组织硬上限分别落实到工单读取和接收目标；每次恢复仍检查当前授权与未软删除工单。收件人为处理人，无处理人时为创建人，组织为工单组织；不让客户端提供接收人，也不因提醒保存工单或提升version。

短事务在scope内以 `FOR SHARE` 取快照，并按tenant／发布者organization／可信application UUID／发布user／key写入不可变intent。intent保存工单、有效收件人、完整固定INFO正文与hash，不保存token、成功缓存或重试队列；无TTL或级联删除。同域同键换工单、换有效目标或改变完整正文409，即使最初平台发布从未提交。固定文本只含不可变编号和详情链接，标题／状态编辑不改变事件正文。确认意图COMMIT并释放连接／锁后，每个传入请求最多用当前应用Bearer向正式 `POST /api/apps/work-orders/notifications` 发出一次POST；不成员预查、不直写平台通知表、不自动补发。

本地意图COMMIT未确认，或总deadline在尝试POST之前耗尽，返回503 `REMINDER_PREPARATION_UNKNOWN`，明确本次零次平台POST。下一次若存在intent必须遵循它；确无持久intent时才能按当前快照首次准备，未提交的准备不保证固定旧接收人。尝试POST后超时、网络、5xx、无效200或期限过期返回503 `REMINDER_RESULT_UNKNOWN`，可能已发布；保留同键和已固定的完整正文，用当前凭据显式恢复，不自动换键重发。明确400／401／403、404 `RECIPIENT_NOT_FOUND` 和409 `IDEMPOTENCY_CONFLICT`分别处理；意图损坏503 `DATABASE_UNAVAILABLE`且停止发布。

200仅返回UUID `publicationId`、`recipientCount:1`、`deliveryCount:1`、布尔 `duplicate` 和RFC3339 `createdAt`。以接收者真实平台session在当前组织的 `GET /api/notifications` 核对publicationId；实时服务未运行或无Toast不等于持久发布失败，删除收件箱消息后重放不会补建delivery。业务意图与平台发布不具跨库原子性；本地提交后工单可继续变化，平台200校验成功后不因再次读取工单而改判失败。

旧Node迁移runner也会执行0003，但旧提醒handler不使用intent，不能提供上述目标固定和准备恢复保证。Phase20不改前端恢复、不部署切流，完整状态与采用边界见 [Rust工单说明](../../../docs/work-orders-rust-baseline.md)。

## Rust启动与迁移

先准备专属业务数据库/账号及正常验证HTTPS的平台机密客户端，通过系统或服务环境显式注入配置。Rust服务不自动读取.env。使用Cargo.lock构建并运行Rust二进制；npm start也只调用cargo run --locked --manifest-path Cargo.toml。数据库升级与回退按上方Rust契约执行。

以下保留Node参考模块的历史接口说明，仅用于兼容测试；旧index与Node部署已经退役。Node迁移oracle仍用于验证历史摘要接管，不作为业务服务运行入口。

## Node参考API契约（仅测试）

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

每种操作要求 `ticket:<action>` 和当前直接组织成员的 `ticket/<action>` 独立 grant。所有范围均叠加可信上下文 `organizationScopeIds` 的组织上限：当前组织及其有效下级组织，禁止访问上级和平级组织。SELF 同时匹配 `owner_user_id=当前用户` 和组织上限，处理人身份不扩大范围；ORG 只匹配当前组织；ORG_SUBTREE 使用平台有效组织 ID 集合；非平台根的 TENANT 由平台收窄为 ORG_SUBTREE。只有平台明确返回 `organizationScopeIds:null` 的实际平台根上下文可以在本租户内不加组织上限；旧上下文缺少该字段时只开放当前组织。缺权限或 grant 返回 403，越界资源与不存在资源都返回 404。

处理人经平台最小应用成员辅助接口校验，是工单所属组织的有效直接成员，且接收组织有当前应用授权。候选目录也要求 create/update 对应操作、grant 和组织上限。SELF 目录只开放当前组织；当前组织下级的本人旧工单可按 SELF 修改其它字段，同值处理人可以保留，更换处理人须获得目标组织范围授权或切换到该组织。上级和平级组织的工单即使属于本人也不能访问。成员名称由服务器验证，客户端不能覆盖。

更新和删除使用行锁与乐观版本，旧版本返回 409；删除后的记录保留在业务库，但列表、详情、修改、提醒和导出均排除。首版没有自动清理任务，正式业务保留期另行确定。

## Node 兼容 CSV 与提醒

CSV 包含人类可读编号、组织和人员名称，状态与优先级使用中文。所有字段按 CSV 规则引用；公式前导字符和控制前导字符加单引号中和。数据超过 1000 条返回 400，超过 4 MiB 返回 413，不返回截断的部分导出。iframe 通过宿主已授权 CSV 下载 broker 保存文件。

旧Node提醒先要求 `ticket:read` 与 `ticket/read` 访问工单，再要求 `notification:publish` 与 `notification/publish`。目标为处理人，未分派时为创建人，接收组织为工单组织；平台发布接口再次验证原应用token、范围和首次接收资格。提醒编号、内容和链接固定，不随标题编辑变化。HTTP网络或5xx结果未知时保留同一键，不自动生成新键；平台已有publication时，同键不同目标返回409。但旧handler不写0003 intent，若此前平台未提交，无法固定那次旧目标，不具上述Rust保证。提醒不重复保存工单，也不引入机器身份。

## 浏览器与错误

默认只为已存在的 opaque iframe 提供 `Origin: null` CORS；允许 Authorization/Content-Type/X-Request-Id 及 GET/POST/PATCH/DELETE 预检，向浏览器暴露响应 X-Request-Id，不使用 cookies 或 credentials。CORS 是传输配置，服务端身份仍由逐请求令牌和授权验证确定。额外 origin 必须逐个配置精确 origin。

错误 JSON 为 `{code,message}`：输入 400、凭证失效 401、操作或数据授权不足 403、不可见资源 404、版本或提醒键冲突 409、容量 413、平台依赖失败 503。错误和日志不输出 token、客户端密钥或数据库连接 URL。

## 测试

`npm test` 包括配置与真实 HTTP/独立 PostgreSQL 验收。真实业务数据库测试读取 `TEST_DATABASE_URL`，只创建并删除自身 UUID 随机库。平台 HTTP fixture 用于精确负向协议测试；完整平台 main、独立平台/业务数据库、真实登录、Socket.IO 和 iframe 联合测试位于上一级 `tests/platform-e2e.mjs`。

Phase 30 授权链、受控成员协议、分页上限、预算参数与显式 legacy 回滚见[应用授权协议](../../../docs/application-authorization-contract.md)。
