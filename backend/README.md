# 工单 Rust API

正式服务是 Rust 1.97.0 的 `work-orders-api`，使用 Axum、SQLx/PostgreSQL 和 reqwest。监听地址默认 `0.0.0.0:3004`，业务前缀 `/work-orders-api`。数据库和数据库账号属于工单业务，平台仅通过 HTTP 提供身份、授权、受控成员目录和通知发布能力。

## 启动与配置

按[开发指南](docs/development.md)准备数据库和平台配置后，在本目录运行：

```powershell
cargo run --locked
```

程序只读进程环境，不自动加载 `.env`、`config.rust.env.example` 或 Compose 的 env 文件。`WORK_ORDERS_DATABASE_URL` 是唯一数据库入口，不回退到平台的 `DATABASE_URL`。`WORK_ORDERS_ALLOWED_ORIGINS` 必须显式配置准确的宿主来源。运行受保护 HTTP 应用还需要完整的 `PLATFORM_API_ORIGIN`、`PLATFORM_CLIENT_ID`、`PLATFORM_CLIENT_SECRET`；数据库工具可单独构造无平台客户端配置，但这不代表正式 API 支持匿名业务请求。

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
| `PLATFORM_AUTHORIZATION_MODE` | `combined`；可显式选择兼容的 `legacy`，无自动降级 |
| `PLATFORM_CA_CERT_FILE` | 可选 PEM 证书包，最多 1 MiB，追加信任根并继续验证 TLS |
| `WORK_ORDERS_METRICS_TOKEN` | 可选，16–1024 个可见 ASCII 字符，不能与机密客户端 secret 相同 |

数据库 TLS 需按环境显式设置 URL 参数，例如 `sslmode=verify-full` 与可信证书路径；生产 HTTP 模式不会自动把 PostgreSQL 的默认 `prefer` 改为强制 TLS。

## 请求授权与存储边界

每次业务请求重新校验应用令牌和平台上下文，不缓存成功授权。默认 `combined` 调用 `/api/apps/work-orders/authorize`，显式 `legacy` 使用 `/oauth/introspect` 和 `/api/apps/work-orders/context`。这个后端 HTTP 授权协议仍使用 `contractVersion: 1.0`，与前端 SDK/MicroProps 2.0 属于不同接口，不能据此判定前端仍是旧 SDK。

功能权限、资源动作授权、租户边界、组织上限与 token/context 期限同时参与校验。`SELF` 以创建人归属判定，处理人不会扩大可见范围。客户端不传入权威的 tenant/owner 字段，API 也不读取平台用户表或平台 JWT 私钥。平台失败或授权预算饱和返回失败，不绕过授权继续读库。

上游 HTTP 禁止重定向、环境代理和隐式请求重试，单次 JSON 最大 256 KiB。成员校验在释放业务连接后进行，随后用短事务复核并提交；提醒先持久化不可变事件并释放锁，再发布平台通知。[API 契约](API.md)记录并发、成员有效期和未知结果恢复要求。

启动会验证实际 schema、历史 Node SHA-256 迁移记录和 SQLx 迁移历史。兼容既有合法工单库，拒绝不支持的对象、历史缺口或 checksum/schema 漂移。`migrations/0001_work_orders.sql`、`0002_create_idempotency.sql`、`0003_reminder_intents.sql` 是不可变增量，不能为了启动旧程序删除记录或篡改已执行 SQL。

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

## 保留的 Node 兼容材料

`auth.mjs`、`db.mjs`、`server.mjs`、配置与 Node 测试用于历史行为比较及 Node → Rust 迁移验证。`index.mjs` 明确报告 `NODE_RUNTIME_RETIRED` 并退出；`npm start` 只是转发 `cargo run`。`config.env.example` 用于 Node 兼容测试，保留 `METRICS_TOKEN`、`NODE_EXTRA_CA_CERTS` 等 Node 名称，来源规则与 qiankun 一致；正式 Rust 服务使用 `config.rust.env.example`。

先运行 `npm ci --ignore-scripts --no-audit --no-fund`，再用 `npm test` 执行 `tests/*.test.mjs`。数据库验收测试会按自身条件跳过，不能把跳过计为通过。
