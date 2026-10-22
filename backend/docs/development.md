# 工单开发指南

## 前置条件

安装 Rust 1.97.0、Node 24.18.0、npm 11.16.0，准备专属 PostgreSQL 数据库/账号和已经运行的平台。平台需创建机密客户端，绑定 work-orders 应用，并按本项目 manifests 配置菜单、功能权限、数据授权及成员查找能力。

## 启动 API

进入本仓库 backend，在 PowerShell 中注入配置；替换所有尖括号值：

```powershell
$env:NODE_ENV='development'
$env:WORK_ORDERS_DATABASE_URL='postgresql://<账号>:<URL编码密码>@127.0.0.1:5432/work_orders'
$env:WORK_ORDERS_HOST='127.0.0.1'
$env:WORK_ORDERS_PORT='3004'
$env:WORK_ORDERS_ALLOWED_ORIGINS='null'
$env:PLATFORM_API_ORIGIN='http://127.0.0.1:3000'
$env:PLATFORM_CLIENT_ID='<平台生成的客户端ID>'
$env:PLATFORM_CLIENT_SECRET='<私有客户端密钥>'
$env:PLATFORM_AUTHORIZATION_MODE='combined'
cargo run --locked
```

程序不自动读取 .env。完整选项见 [配置模板](../config.rust.env.example)。开发平台必须能从 API 进程访问，客户端绑定、准确应用和授权缺一不可。

GET /work-orders-api/health/live 只说明进程可响应，/health/ready 校验数据库和迁移；它们不证明业务用户权限正确。

## 启动前端

另开终端进入 frontend，执行 [前端 README](../../frontend/README.md)中的安装和环境命令。默认端口 3013，通过平台登记的入口加载。推荐使用绝对开发 API 地址，避免依赖平台宿主没有对齐的代理。

opaque iframe 请求来源为字面 null，后端按模板允许；这不是任意来源通配。若更换运行模式或普通浏览器直连 API，需明确配置对应准确来源。

## 验证

在仓库根执行：

```sh
node .github/scripts/check-inputs.mjs
node --test .github/tests/repository-structure.mjs
```

在 frontend 执行 npm test、npm run build；在 backend 执行 cargo check --locked --all-targets、cargo test --locked --lib。数据库专项测试按源码声明在隔离库显式运行，不把忽略项计为通过。

最后用普通用户验证列表、详情、合法与越权写入、成员目录、切组织和撤权。前端静态测试不能替代平台/数据库联调。业务错误和恢复步骤见 [API 契约](../API.md)。
