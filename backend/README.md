# 工单后端

Rust 1.97.0 独立服务，默认 API 端口 3004，业务前缀 /work-orders-api，使用专属 PostgreSQL 数据库和账号。当前授权默认 combined，通过平台 HTTP 校验，不查询平台用户表。

在本 backend 目录执行：

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo test --locked --lib
cargo build --release --locked
cargo run --locked
```

先按 [config.rust.env.example](config.rust.env.example) 注入实际进程环境，程序不自动读取 .env 或模板。启动时接管/验证本业务迁移，异常历史或 schema 拒绝启动。客户端 secret 不进入前端。

本目录同时是 API Docker 构建上下文：

```sh
docker build -t saas-work-orders-api:local .
```

src 是实现，migrations 是不可变迁移，manifests 是公开登记与授权清单，vendor 是固定协议材料。compose.yml 运行数据库、API 和静态前端，不负责构建源码。

完整步骤见 [开发指南](docs/development.md)、[部署指南](docs/deployment.md)，路由及写入恢复见 [API](API.md)。
