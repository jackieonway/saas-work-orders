# 工单 Rust API

正式服务是 Rust 1.97.0 的 `work-orders-api`，使用 Axum、SQLx/PostgreSQL 和 reqwest。监听地址默认 `0.0.0.0:3004`，业务前缀 `/work-orders-api`。数据库和数据库账号属于工单业务，平台仅通过 HTTP 提供身份、授权、受控成员目录和通知发布能力。

## 启动与配置

按[开发指南](docs/development.md)准备数据库和平台配置后，在本目录运行：

```powershell
cargo run --locked
```

独立迁移命令为 `cargo run --locked --bin work-orders-migrate`，导入前初始化可使用 `cargo run --locked --bin work-orders-migrate -- --import`。两种方式都只读取 `WORK_ORDERS_DATABASE_URL`，连接已创建的数据库、执行并核验当前全新基线，然后关闭连接退出；不启动 HTTP，不要求平台或 CORS 配置，不创建数据库或角色，`--import` 不导入数据。工单没有 seed，`--seed` 会被拒绝；`--seed --import` 明确互斥，未知或重复参数也会在连接前失败。

迁移命令拒绝 `PG*` 环境变量，避免驱动继承 URL 之外的端口、密码、证书或会话设置；数据库 TLS 仍通过 `WORK_ORDERS_DATABASE_URL` 的受支持参数配置。连接获取限时 5 秒，迁移限时 5 分钟，关闭连接限时 10 秒。先停用业务进程，由导入流程保持独占；该命令不提供数据导入互斥锁。默认 `cargo run --locked` 仍启动 `work-orders-api`。

迁移命令不读取默认 pgpass 文件。新库 owner 使用 `NOLOGIN` 时，可由本机 postgres peer 会话通过 URL 显式指定 owner，例如 `postgresql://postgres@localhost:5432/new_work_orders_db?host=/run/postgresql&options[role]=new_work_orders_owner`。`host` 必须是规范绝对 Unix socket 目录，`options[role]` 仅接受 1–63 字节的小写字母、数字、下划线标识符且不能以数字开头；该委托形式要求 `postgres@localhost`、无密码并显式给出 socket。其他启动 options、未知或重复 URL 参数均拒绝。正常 API 的 URL 参数合同不变。

程序只读进程环境，不自动加载 `.env`、`config.rust.env.example` 或 Compose 的 env 文件。`WORK_ORDERS_DATABASE_URL` 是唯一数据库入口，不回退到平台的 `DATABASE_URL`。`WORK_ORDERS_ALLOWED_ORIGINS` 必须显式配置准确的宿主来源。运行受保护 HTTP 应用还需要完整的 `PLATFORM_API_ORIGIN`、`PLATFORM_CLIENT_ID`、`PLATFORM_CLIENT_SECRET`（独立 `APP_RESOURCE` 服务凭据）；数据库工具可单独构造无平台客户端配置，但这不代表正式 API 支持匿名业务请求。

正常 API 的 `WORK_ORDERS_DATABASE_INIT_MODE` 缺省或为 `seed` 时只核验预先建立的当前 `0001` 基线，不建表、不迁移、不执行 DDL。全新安装先由独立 `work-orders-migrate` 在空库完成基线，再向受限 LOGIN 运行账号交接；缺少或不匹配的迁移历史、checksum 或结构指纹均拒绝启动。`import` 会在连接前拒绝启动，须使用独立迁移命令。已经完成受信离线导入的目标可使用 `imported`：必须同时提供 `WORK_ORDERS_EXPECTED_DATABASE`、`WORK_ORDERS_IMPORT_MARKER`、`WORK_ORDERS_EXPECTED_SCHEMA_SHA256`，其他模式携带其中任一项均拒绝。标记必须精确为 `saas-uuid-import/1/<规范小写 UUIDv7 RFC batch UUID>/<64位小写十六进制摘要>`，schema SHA256 必须来自同批认证导入证明中的完整有序 `schema_material` JSON，而不是自行对当前库重新签认。

`imported` 先核对 URL 解码后的库名，再以同一只读 Repeatable Read 事务核对实际库名、数据库 COMMENT、完整 catalog SHA256、正式迁移历史与 readiness 约束。运行账号必须是 URL 明确指定的 LOGIN、非高权角色，不能拥有库或用户 schema/对象，不能继承或切换到 owner/高权角色，也不能通过任何可切换角色获得库或 schema CREATE 或数据库 TEMPORARY。校验通过才启动 HTTP，整个分支不执行迁移或 seed；平台客户端和 CORS 等正常 HTTP 配置仍然必需。该模式只验证已完成的导入，不执行导入或数据库授权修改。

配置字段与默认值由 `src/config.rs` 定义，[config.rust.env.example](config.rust.env.example)提供参考。qiankun 的 API 请求来自真实平台页面 origin，配置不得使用 `null`、通配符、带路径或尾斜杠的 URL；缺失/空值会在配置加载时失败。`NODE_ENV=production` 是 Rust 继续兼容的环境开关，要求平台与准确来源使用 HTTPS；服务本身不需要 Node 运行时。

| 配置 | 默认与约束 |
| --- | --- |
| `WORK_ORDERS_HOST` / `WORK_ORDERS_PORT` | `0.0.0.0` / `3004`；host 使用数字 IP，生产 port 不得为 0 |
| `WORK_ORDERS_ALLOWED_ORIGINS` | 必填，逗号分隔的准确宿主 origin；生产 HTTPS，开发允许回环 HTTP |
| `WORK_ORDERS_DB_MAX_CONNECTIONS` | 10，范围 1–30 |
| `WORK_ORDERS_DB_ACQUIRE_TIMEOUT_MS` | 5000，范围 100–30000 |
| `WORK_ORDERS_DB_STATEMENT_TIMEOUT_MS` / `WORK_ORDERS_DB_LOCK_TIMEOUT_MS` | 各 5000，范围 100–30000 |
| `WORK_ORDERS_STARTUP_TIMEOUT_MS` | 30000，范围 1000–300000 |
| `WORK_ORDERS_SHUTDOWN_TIMEOUT_MS` | 10000，范围 100–60000 |
| `WORK_ORDERS_HEALTH_TIMEOUT_MS` | 2000，范围 100–10000 |
| `PLATFORM_AUTH_TIMEOUT_MS` / `PLATFORM_AUTH_MAX_CONCURRENT` | 3000 ms / 32；分别为 100–10000 与 1–128 |
| `PLATFORM_AUTHORIZATION_MODE` | 缺省或 `canonical`；旧 `combined`／`legacy` 拒绝启动 |
| `PLATFORM_IDENTITY_ISSUER` | 外部 Access Token 必须匹配的准确 HTTPS issuer；缺省时仅内部 app_session 可用，不从平台 API origin 推断 |
| `PLATFORM_RESOURCE_SCOPE_BINDINGS` | 来自审核部署配置的 JSON 对象，将 `ticket:read/export/create/update/delete` 及可选 `notification:publish` 映射到准确 OAuth scope；缺少当前操作映射即拒绝外部 token；重复键、未知操作和非法 scope 拒绝启动 |
| `PLATFORM_CA_CERT_FILE` | 可选 PEM 证书包，最多 1 MiB，追加信任根并继续验证 TLS |
| `WORK_ORDERS_METRICS_TOKEN` | 可选，16–1024 个可见 ASCII 字符，不能与机密客户端 secret 相同 |

数据库 TLS 需按环境显式设置 URL 参数，例如 `sslmode=verify-full` 与可信证书路径；生产 HTTP 模式不会自动把 PostgreSQL 的默认 `prefer` 改为强制 TLS。

## 请求授权与存储边界

每次业务请求使用独立服务 Basic 凭据，将原 token、业务 `resource=ticket` 和准确 action 放在 POST Form 中调用 `/api/apps/work-orders/authorize`；只接受 `resource-authorization/1`，没有旧内省／context 运行路径或自动回退。外部 `access_token` 必须匹配显式 identity issuer、`work-orders` 资源和 audience，以及该操作的审核 scope 映射；内部 `app_session` 必须匹配固定内部 issuer、work-orders audience 和 `app:access`。USER 只从私有 `principal` 读取员工、成员、账号代际及组织，公开 opaque `sub` 不解析为 UUID。公开 context 1.0 和 SDK 3.0.0 合同保持原版本。

MACHINE 使用独立 client／credential／delegation 及租户组织范围，只开放明确批准的 read/export；不生成用户、成员或虚构组织 ID，禁止 SELF、创建、修改、删除、处理人目录与提醒。人员写入在成员／收据预读后、短事务前，用同一个原 token 再次在线核对原主体、代际、投影和数据范围，并只保留更短期限；网络检查不持有业务连接或行锁。它是独立服务间的授权检查点，不是跨 HTTP 与数据库的原子事务。

成员目录使用 `/internal/platform/apps/work-orders/members[/userId]` 的 Basic＋Form POST，接受 `resource-directory/1` 外层及原成员 DTO；原三秒期限从 HTTP 开始计算，服务响应或数据库等待不能延长它。提醒使用同一私有 origin 的 `/internal/platform/apps/work-orders/notifications/{result,publish}`：独立服务 Basic，原 token 和完整原请求只在表单内。外部 USER 还须具备明确 `notification:publish` scope 映射、该 scope、发布权限及独立数据授权；MACHINE 继续拒绝。每次先查原结果，只有准确 404 `NOT_FOUND` 才发布同一请求；响应为 `resource-notification/1`，查询及发布总窗口不超过原三秒。完整权威合同见[平台资源合同](../../saas-plateform/backend/docs/designs/unified-auth/phase-15-resource-capability-contract.md)与[私有通知合同](../../saas-plateform/backend/docs/designs/unified-auth/phase-18-private-notification-contract.md)。

提醒意图在当前 `0001` 中额外保存原 token 摘要和原授权来源摘要，不保存原 token。重试必须匹配这些不可变事实；更换登录会话、业务身份或凭据后使用旧事件键会返回 `409 REMINDER_SOURCE_CHANGED`，不得自动换键补发。先核实原通知结果，再由用户明确发起独立提醒。此结构只用于新库，不接管旧 checksum 或对已有库运行增量 DDL。

功能权限、资源动作授权、租户边界、组织上限与 token/context 期限同时参与校验。`SELF` 以创建人归属判定，处理人不会扩大可见范围。客户端不传入权威的 tenant/owner 字段，API 也不读取平台用户表或平台 JWT 私钥。平台失败或授权预算饱和返回失败，不绕过授权继续读库。

上游 HTTP 禁止重定向、环境代理和隐式请求重试，单次 JSON 最大 256 KiB。成员校验在释放业务连接后进行，随后用短事务复核并提交；提醒先持久化不可变事件并释放锁，再发布平台通知。[API 契约](API.md)记录并发、成员有效期和未知结果恢复要求。

## 代码与验证

`src/http.rs` 定义路由、CORS、请求 ID 和 15 秒请求上限；`authorization/` 验证平台结果；`reads/`、`writes/`、`directory/`、`reminders/` 实现业务规则；`migrations/` 验证和迁移数据库；`telemetry.rs` 提供日志与私有指标。

```powershell
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo test --locked --lib
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
```

`tests/*.rs` 中需要隔离 PostgreSQL 的测试默认忽略，普通单元测试通过不代表这些测试通过。授权单元测试默认由 Rust 拥有 HTTP 监听；若本机阻止 Rust 测试进程接受连接，可显式设置 `WORK_ORDERS_TEST_NODE` 为已安装 Node 的路径，用专有 Node 监听转发到原 Axum 测试处理器。正式 reqwest、授权断言与生产路径不变，程序不会自动降级。详细运行条件见[开发指南](docs/development.md)。本目录可直接构建 API 镜像，Compose 只负责运行已经构建的镜像，见[部署指南](docs/deployment.md)。

## 全新数据库基线

仅使用 `migrations/0001_work_orders.sql` 初始化空库，不再兼容旧升级链或历史迁移记录。SQLx 只记录版本 1，并核对当前结构指纹；漂移或 checksum 不符拒绝启动。不会通过改写迁移历史或旧快照签认现状。UUIDv7、当前身份/不可变约束、幂等与业务审计在首次建表时全部建立。

系统与其他服务的数据归属见[全新安装](../../saas-plateform/backend/docs/fresh-installation.md)。
