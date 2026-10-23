# 构建、部署与恢复

工单独立部署 API、静态前端和专属 PostgreSQL。平台及其网关已经存在；本仓库 Compose 不启动平台、不创建机密客户端，也不自动配置应用授权。

## 构建不可变镜像

在仓库根执行，替换版本、宿主和公开 API 地址：

```powershell
docker build -t saas-work-orders-api:2.0.1 backend
docker build --build-arg APP_CODE=work-orders --build-arg APP_VERSION=2.0.1 --build-arg HOST_ORIGIN=https://platform.example.com --build-arg API_BASE=/work-orders-api -t saas-work-orders-web:2.0.1 frontend
```

后端 Docker 使用 Rust 1.97.0 构建，运行镜像只含 Rust 二进制与系统运行依赖；前端使用 Node 24.22.1 构建、非 root nginx 在 8080 提供 `dist/`。`APP_CODE`、`APP_VERSION`、`HOST_ORIGIN`、`API_BASE` 是前端构建公开输入，任何客户端 secret、数据库连接或平台会话都不得传入这些参数。静态镜像渲染器要求 HTTPS，回环 HTTP 只用于本地 Vite 开发构建。

生产发布使用自己审核的基础镜像及 registry，记录源码版本、构建记录和镜像摘要。Compose 镜像变量填写不可变摘要。当前 CI 只测试和构建镜像，没有自动推送、自动登记入口或上线步骤。

## 不可变本地候选与产物门禁

生产静态构建沿用 `deploy/build-static.mjs` 的四字段输入和 HTTPS 校验。候选工具先在受控临时目录构建并运行门禁，再独占创建新版本目录，完成校验后最后写入 `candidate.json`；同名版本即使只有空目录也拒绝覆盖。工具不配置平台入口、不修改现有 nginx、不启动服务、不修改数据库或容器。

本轮已生成的本地候选使用明确的回环 HTTP 宿主，不能作为生产 HTTPS 部署包。在 `frontend/` 中复核当前候选：

```powershell
npm run release:check -- deploy/micro-apps/work-orders/versions/2.0.1-qiankun/html
```

目录为 `frontend/deploy/micro-apps/work-orders/versions/2.0.1-qiankun/`，包含 `html/`、`public-config.json` 与 `candidate.json`。`--local` 只允许回环 HTTP 宿主及本机 API 地址/路径，不生成生产 `nginx.conf`；候选元数据标记 `localCandidate: true`、`deployed: false`。该目录由 Git 忽略，需要单独保留或打包交付。

生成下一份候选时必须使用尚未存在的新版本，不要重新生成上面的已有版本。下面的 PowerShell 示例附加本机时间作为新版本后缀；若发生同名，工具仍会拒绝覆盖：

```powershell
$newVersion='2.0.1-qiankun-' + (Get-Date -Format 'yyyyMMddHHmmss')
npm run release:candidate -- --local --version $newVersion --host-origin http://127.0.0.1:3000 --api-base /work-orders-api
```

旧 `2.0.0-qiankun` 候选保留原字节；由于前端 package 与工具链已升级，它不是当前源码的验收候选，不能以修改旧包或改摘要的方式让门禁通过。

生产使用已确认的真实 HTTPS 宿主和新的不可变版本，移除 `--local` 再生成候选，此时复用原静态渲染器输出 `nginx.conf`。示例域名 `platform.example.com` 仅表示待替换参数，不是本次已确认的部署地址。

构建自动运行 `release:check`，CI 也显式检查实际 `dist/`。门禁验证 HTML、UMD、CSS、固定 SDK 2.0.0、版本/公开配置、当前源码摘要、资源清单和逐文件 SHA-256，并在禁用网络的 VM 中核验四个生命周期与 qiankun 最后全局导出；拒绝旧 ESM/iframe/SDK 1、缺失或多余资源、变更文件和陈旧源码记录。已有旧 `dist/` 未被候选工具覆盖，缺少新清单的旧产物会被门禁拒绝；不能为通过检查改旧包字节或把旧包重新命名为新版本。

## 运行配置与 qiankun 来源

将 `backend/compose.env.example` 复制到源码之外的私有文件，例如 `/srv/saas/private/work-orders.env`，填写：

| 配置 | 用途 |
| --- | --- |
| `POSTGRES_IMAGE` | 已批准的 PostgreSQL 镜像摘要 |
| `WORK_ORDERS_API_IMAGE` / `WORK_ORDERS_FRONTEND_IMAGE` | 上一步构建并发布的镜像摘要 |
| `WORK_ORDERS_POSTGRES_PASSWORD` | 工单独立数据库口令 |
| `WORK_ORDERS_DATABASE_URL` | `postgresql://work_orders:<URL编码密码>@work-orders-postgres:5432/work_orders` |
| `WORK_ORDERS_ALLOWED_ORIGINS` | 平台页面的准确 HTTPS origin，多个用逗号分隔 |
| `PLATFORM_API_ORIGIN` | API 容器可访问的真实平台 HTTPS origin |
| `PLATFORM_CLIENT_ID` / `PLATFORM_CLIENT_SECRET` | 已绑定 work-orders 的机密客户端凭据 |
| `EDGE_NETWORK_NAME` | 预先创建、可与平台网关通信的受控网络 |
| `WORK_ORDERS_METRICS_TOKEN` | 可选的独立私有指标 token |

Compose 显式要求 `WORK_ORDERS_ALLOWED_ORIGINS`，缺失/空值在配置展开阶段报错。Rust 与 Node 配置都拒绝缺失、空值、`null`、通配符、带路径或尾斜杠的来源；生产只接受 HTTPS，开发可使用准确回环 HTTP。该值来自平台页面地址，不能从私有 `PLATFORM_API_ORIGIN` 猜测。平台同域 API 代理若继续转发浏览器 `Origin`，API 仍执行相同检查。允许来源只负责 CORS，受保护请求仍要求应用 token 与实时授权。

授权预算、自定义 CA、数据库预算等配置若未在基础 Compose 中透传，须一并添加到 override；证书文件还需只读挂载。`PLATFORM_CA_CERT_FILE` 使用容器路径，不能仅设置宿主变量。`PLATFORM_API_ORIGIN` 不能用容器 `localhost` 指代宿主平台。数据库 TLS 根据实际网络要求显式配置，Rust 生产开关不会强制数据库 TLS。

## 启动与健康检查

下面命令在服务器的 `backend/` 执行，使用同一私有 env 文件；额外配置若使用 override，应每次传入相同的 `-f` 参数：

```sh
docker compose --env-file /srv/saas/private/work-orders.env -f compose.yml config --quiet
docker compose --env-file /srv/saas/private/work-orders.env -f compose.yml up -d
docker compose --env-file /srv/saas/private/work-orders.env -f compose.yml ps
docker compose --env-file /srv/saas/private/work-orders.env -f compose.yml logs --tail 100 work-orders-api
```

服务名为 `work-orders-postgres`、`work-orders-api`、`work-orders-web`。数据库只在内部数据库网络，业务持久数据放 `business-data` 卷。API 与静态前端加入已有 edge 网络，默认只向主机回环暴露 3004 与 3013。API 健康检查访问 `/work-orders-api/health/ready`；静态服务访问 `/healthz`。存活或就绪为成功不代表普通用户业务授权已通过。

API 默认没有开放指标；设置私有 token 后，通过 `/work-orders-api/internal/metrics` 的独立 Bearer token 读取。指标路由由网关/私网控制访问，不用业务应用令牌替代指标凭据。API 日志输出结构化请求信息，可用响应的 `X-Request-Id` 关联排查。

## 网关、静态版本与应用登记

平台网关的工单 API 上游指向 `work-orders-api:3004`，静态上游指向 `work-orders-web:8080`，并按网关自身规则加入允许服务清单。公开静态 URL 应带固定版本，例如 `/micro-static/work-orders/1.0.0/index.html`；网关移除版本路由前缀后再转发到该静态镜像的根目录。登记 `HTML` / `TRUSTED` 入口为最终可访问 URL，应用 basePath 仍为 `/micro/work-orders`。

完整发布 HTML、UMD、CSS、许可证和构建元数据。资源均相对 HTML 入口解析，单个版本中的文件必须来自同一份 `dist/`。缺失资源返回 404，不使用 SPA 回退；HTML 使用 `no-cache`，成功的版本资源使用 immutable 缓存，错误响应使用 `no-store`。

生成的静态 nginx 配置对公开资源提供 `Access-Control-Allow-Origin: *` 且不启用 credentials，供 qiankun fetch HTML/脚本。这里的公开静态资源 CORS 与 API 精确来源授权是两个边界。宿主页面的 CSP 还需允许实际静态入口和业务 API；静态响应的 CSP 不能替宿主完成这一配置。业务 API 路径相对平台 origin 解释时，平台需要提供 `/work-orders-api` 代理。

新版本不能覆盖旧版本 URL。需要回退时为旧版本保留独立静态服务或版本存储，确保 `1.0.0` 地址不会随新镜像变化。发布前核对 `build-info.json` 中的应用、版本、`runtime: qiankun`、`contractVersion: 2.0`、SDK 版本及源码摘要。

## 发布验收与恢复

先验证真实 HTTPS 入口、HTML/JS/CSS MIME、静态跨域读取、资源 404 和 API 预检，再用普通用户验证全部业务动作、深层路由、切组织、撤权、令牌更新和卸载。按照 [API 契约](../API.md)验证并发冲突、未知创建/修改/删除和提醒恢复；单元测试及容器健康状态不能替代平台联调。

升级前备份专属数据库并记录迁移历史、当前镜像摘要、网关版本路由、私有配置版本和未确认写入事件。启动会拒绝不一致的 schema 或迁移 checksum，不要修改已执行 SQL、删除幂等回执/提醒意图或倒退迁移历史以启动旧镜像。

前端回退依靠保留的静态版本和平台入口切换；后端回退要求旧二进制兼容当前数据库。数据库恢复先进入隔离新库，验证租户边界、业务记录和恢复事件后再切流，不删除生产数据卷作为升级步骤。平台统一恢复工具的材料格式与协调流程需要单独对齐，本仓库 Compose 没有自动提供跨服务恢复事务。
