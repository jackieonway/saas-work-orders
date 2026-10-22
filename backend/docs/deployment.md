# 工单 Docker / Compose 部署

## 1. 构建镜像

在本仓库根执行；来源、版本和路由替换为实际值：

```sh
docker build -t saas-work-orders-api:local backend
docker build --build-arg APP_CODE=work-orders --build-arg APP_VERSION=1.0.0 --build-arg HOST_ORIGIN=https://platform.example.com --build-arg API_BASE=/work-orders-api -t saas-work-orders-web:local frontend
```

上下文是两个实际工程目录。将审阅镜像发布到自己的 registry，记录不可变摘要，Compose 运行时使用该摘要；当前 CI 只构建，不自动发布或上线。

## 2. 运行配置

将 backend/compose.env.example 复制为源码之外的私有文件，例如 /srv/saas/private/work-orders.env。填写 POSTGRES_IMAGE、WORK_ORDERS_API_IMAGE、WORK_ORDERS_FRONTEND_IMAGE、WORK_ORDERS_POSTGRES_PASSWORD、WORK_ORDERS_DATABASE_URL、PLATFORM_API_ORIGIN、PLATFORM_CLIENT_ID、PLATFORM_CLIENT_SECRET。

数据库 URL 使用容器 DNS work-orders-postgres:5432，数据库名 work_orders，账号与 Compose 一致；密码做 URL 编码。API 与平台通过真实 HTTPS 交互，不能用容器 localhost 指向主机平台。

EDGE_NETWORK_NAME 对应预先创建的受控私网，例如 platform-edge。Compose 不自动创建该外部网络，也不启动平台。可选私有指标使用 WORK_ORDERS_METRICS_TOKEN；额外 CA、授权模式或预算参数若未在 Compose 透传，需显式补充服务环境/override，不能仅写宿主变量后推断生效。

## 3. 启动与检查

进入 backend 执行：

```sh
docker compose --env-file /srv/saas/private/work-orders.env -f compose.yml config --quiet
docker compose --env-file /srv/saas/private/work-orders.env -f compose.yml up -d
docker compose --env-file /srv/saas/private/work-orders.env -f compose.yml ps
docker compose --env-file /srv/saas/private/work-orders.env -f compose.yml logs --tail 100 work-orders-api
```

Compose 管理 work-orders-postgres、work-orders-api、work-orders-web。API 默认仅绑定主机回环 3004，静态前端回环 3013；共享 edge 网络可供平台网关通过容器服务名访问。

## 4. HTTPS 与应用登记

在平台网关配置 workOrders 上游为 work-orders-api:3004；静态版本路由使用 work-orders-web:8080，并加入 allowedServices。平台登记入口须匹配真实版本路径与镜像 APP_VERSION；CSP 和来源要允许实际连接。

旧版本 URL 不能都指向会被覆盖的新镜像；支持回退时需保留独立旧静态服务或版本存储。配置完成后实际验证 HTTPS、资源 MIME/404、普通用户业务操作、切组织和撤权。

## 5. 升级与恢复

升级前备份并记录当前镜像、配置、迁移历史及在途未知写。后端不得重写已执行 SQL/checksum；旧镜像必须兼容当前数据库。前端回退与数据库恢复分别处理，不删除业务数据卷作为升级步骤。

恢复到隔离新库后再做业务验收。三库协调恢复工具由平台 ops 维护，其固定材料格式需要单独对齐，不因使用 Docker 自动可用。接口并发和未知结果规则见 [API](../API.md)。
