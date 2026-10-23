# 本地开发与验证

本指南在独立 `saas-work-orders` 仓库内执行命令。前端由平台 qiankun 宿主加载，后端运行 Rust，PostgreSQL 使用工单专属数据库。先阅读[项目入口](../../README.md)确认目录与接口边界。

## 准备工具与平台登记

安装 Node.js 24.22.1、npm 11.16.0、Rust 1.97.0，并准备可创建工单业务表的专属 PostgreSQL 账号。确认 `node --version` 与 `npm --version` 使用同一套 Node 环境；Windows 的 PATH 可能优先选中其他软件附带的旧 Node。

平台需先运行，并完成以下登记。`manifests` 是人工配置参考，不是自动执行或自动授权脚本。

1. 按 [bootstrap.manifest.json](../manifests/bootstrap.manifest.json)登记 `work-orders`，基础路径 `/micro/work-orders`，入口 `http://127.0.0.1:3013/index.html`，类型 `HTML`、模式 `TRUSTED`。
2. 创建机密客户端并绑定该应用。把平台生成的 client ID 和只显示一次的 secret 保存到后端私有配置；客户端显示名称不保证等于 client ID。
3. 创建工单菜单、`ticket:read/create/update/delete/export` 与 `notification:publish` 功能权限。为测试用户分配应用、菜单和对应资源动作的数据授权；可见菜单不等于有业务权限。
4. 需要分配处理人时，确保平台受控成员目录可以查询所需组织的有效直接成员。需要发送提醒时，配置通知发布授权与平台通知能力。

[grants.manifest.example.json](../manifests/grants.manifest.example.json)中的 UUID 是占位符，`targets` 默认空，不会授予用户权限。

## 启动 Rust API

在 `backend/` 中执行，替换尖括号内容。程序不读取环境模板文件，环境变量必须进入实际 Rust 进程。

```powershell
$env:NODE_ENV='development'
$env:WORK_ORDERS_DATABASE_URL='postgresql://<账号>:<URL编码密码>@127.0.0.1:5432/work_orders'
$env:WORK_ORDERS_HOST='127.0.0.1'
$env:WORK_ORDERS_PORT='3004'
$env:WORK_ORDERS_ALLOWED_ORIGINS='http://127.0.0.1:3000'
$env:PLATFORM_API_ORIGIN='http://127.0.0.1:3000'
$env:PLATFORM_CLIENT_ID='<平台生成的客户端ID>'
$env:PLATFORM_CLIENT_SECRET='<后端私有密钥>'
$env:PLATFORM_AUTHORIZATION_MODE='combined'
cargo run --locked
```

按真实平台地址替换 `3000`。`localhost` 与 `127.0.0.1` 是不同来源。开发仅允许回环 HTTP，远程连接使用 HTTPS；内网主机名的 HTTP 也不会自动放行。API 不接受平台数据库 URL 作为业务库。

启动过程接管或验证合法迁移历史，再提供受保护 API。`GET /work-orders-api/health/live` 只验证进程；`/health/ready` 或 `/health` 检查业务库与迁移，不验证用户的全部权限。

## 启动 qiankun 前端

另开终端进入 `frontend/`：

```powershell
npm ci --ignore-scripts --no-audit --no-fund
$env:WORK_ORDERS_ALLOWED_HOST_ORIGINS='http://127.0.0.1:3000'
$env:WORK_ORDERS_API_BASE='http://127.0.0.1:3004/work-orders-api'
npm run dev
```

开发服务监听 `127.0.0.1:3013`，提供与发布相同的 qiankun UMD。源码修改后刷新平台宿主页。通过平台菜单或 `/micro/work-orders` 进入；直接打开 `3013/index.html` 只会获取入口文件，没有平台身份不会挂载。

前端 `WORK_ORDERS_ALLOWED_HOST_ORIGINS` 控制宿主白名单，后端 `WORK_ORDERS_ALLOWED_ORIGINS` 控制业务 API 请求来源。qiankun 在平台页面中运行，请求的 `Origin` 是平台 origin。Rust 与 Node 配置均要求显式来源，拒绝缺失、空值、`null` 与通配符；上述设置与平台真实地址必须一致。

若平台已经配置 `/work-orders-api` 代理，可把前端 API 地址改成 `/work-orders-api`。代理请求仍可能转发 `Origin`，API 的来源白名单继续设置为真实平台来源。浏览器请求不带平台 Cookie，由 SDK 使用当前应用 token。

## 常规检查

在仓库根运行独立输入与构建上下文检查：

```powershell
node .github/scripts/check-inputs.mjs
node --test .github/tests/repository-structure.mjs
```

在 `frontend/` 运行：

```powershell
npm test
npm run build -- --allowed-host-origin https://platform.example.com --api-base /work-orders-api
```

在 `backend/` 运行：

```powershell
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo test --locked --lib
cargo clippy --locked --all-targets -- -D warnings
```

CI 使用相同的 Node/Rust 版本，执行前端测试、实际产物门禁、输入检查、Node 兼容测试、Rust 格式/check/lib test/clippy，并构建前后端镜像；授权单元测试同时验证默认 Rust 监听与显式 Node 监听模式。现有工作流没有执行数据库专项测试，也没有发布镜像或部署生产。构建依赖摘要变化必须审阅来源并更新锁定记录，不能仅为消除检查错误重写摘要。

某些 Windows 主机限制 Rust 测试可执行文件接受本机 TCP 连接，会使授权 HTTP fixture 超时。已确认这一环境时，可在当前终端显式选择专有 Node HTTP 监听：

```powershell
$env:WORK_ORDERS_TEST_NODE=(Get-Command node).Source
cargo test --locked --lib -- --test-threads=1
Remove-Item Env:WORK_ORDERS_TEST_NODE
```

该测试辅助进程只监听随机回环端口，用 stdin/stdout 把请求转到原 Axum Router，再由正式 reqwest 客户端接收真实 HTTP 响应；授权逻辑、状态、响应体上限、期限与断言不变。串行运行避免多个辅助进程在繁忙 Windows 主机上争用短超时预算。不修改防火墙、代理、生产代码或系统环境，也不自动切换监听方式。测试结束关闭自有子进程。

## 隔离数据库专项测试

`backend/tests/read_contracts.rs`、`write_contracts.rs`、`member_contracts.rs`、`reminder_contracts.rs`、`migrations.rs` 包含真实 PostgreSQL + 受控平台 HTTP fixture 测试，均有默认忽略项。它们不是对真实平台的端到端测试。

测试要求显式的 `CI_TEST_POSTGRES_OWNER=ci-<标识>`，数据库 URL 主机必须精确为 `<owner>-postgres`，初始数据库为 `/postgres`；账号要有创建和清理随机测试库的权限。配置专属、可丢弃的测试实例与匹配 DNS 后，按具体测试目标运行，例如：

```powershell
$env:CI_TEST_POSTGRES_OWNER='ci-work-orders'
$env:TEST_DATABASE_URL='postgresql://<测试管理员>:<URL编码密码>@ci-work-orders-postgres:5432/postgres'
cargo test --locked --test read_contracts -- --ignored --test-threads=1
cargo test --locked --test write_contracts -- --ignored --test-threads=1
cargo test --locked --test member_contracts -- --ignored --test-threads=1
cargo test --locked --test reminder_contracts -- --ignored --test-threads=1
```

迁移专项还会调用保留的 Node 迁移驱动，先在 `backend/` 用当前 Node 执行 `npm ci --ignore-scripts --no-audit --no-fund`，再运行 `cargo test --locked --test migrations -- --ignored --test-threads=1`。不要把业务库或平台库作为测试管理员入口，不要去掉测试的归属校验。

保留 Node 兼容测试的命令是 `npm test`，对应 `node --test tests/*.test.mjs`；先在 `backend/` 安装锁定依赖。其中 API 测试可读取 `WORK_ORDERS_TEST_DATABASE_URL`、`TEST_DATABASE_URL` 或仓库根被忽略的 `.env.test`。它们测试历史 Node 模块，不证明 Rust 正式服务通过。未配置数据库而跳过的用例要单独记录。

## 平台联调验收

使用普通用户分别验证列表、详情、创建、分配处理人、合法状态转换、带版本更新、软删除、CSV 和提醒。再验证没有功能权限、没有数据授权、跨租户、组织上限、切组织、撤权、令牌更新/过期和卸载后的迟到响应。只有 `create` 权限的用户应能进入独立创建路由，不能据此读取列表。

模拟写入丢失响应时，按 [API 契约](../API.md)恢复原始事件；不要改键补发。构建与单元测试不能代替这一步，联调记录应注明平台、入口版本、网关配置、用户授权和数据库测试范围。
