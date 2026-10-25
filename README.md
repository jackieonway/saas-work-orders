# 工单管理微应用

工单业务由 Vue 3 前端、Rust API 和独立 PostgreSQL 数据库组成。前端运行在平台的 qiankun 宿主中，平台提供应用令牌、用户与组织上下文、导航和 CSV 下载服务；API 按请求向平台核验身份与授权，再访问自己的业务库。

当前前端发布链路采用 qiankun HTML 入口、classic UMD 生命周期与固定 SDK 3.0.0。Rust、Node 兼容配置和 Compose 均要求显式填写准确的平台页面来源，拒绝 `null`、通配符和缺失配置；业务请求仍逐次校验应用令牌与授权。构建会验证实际 UMD、SDK、资源与摘要，并可生成只新增、不覆盖旧版本的静态候选包，步骤见[部署指南](backend/docs/deployment.md)。

当前本地验收候选是 `frontend/deploy/micro-apps/work-orders/versions/2.0.1-qiankun/`，使用 `http://127.0.0.1:3000` 宿主和 `/work-orders-api`。它是尚未部署的本机候选；旧 `2.0.0-qiankun` 保留原字节。复核当前候选使用 `release:check`，生成后续候选必须选择未使用的新版本，不能覆盖已有目录。

| 阅读目的 | 文档 |
| --- | --- |
| 准备平台、数据库并在本地启动 | [开发指南](backend/docs/development.md) |
| 接入 qiankun、构建前端和理解生命周期 | [前端说明](frontend/README.md) |
| 运行 Rust 服务并理解配置与迁移 | [后端说明](backend/README.md) |
| 调用接口、处理授权与未知写入结果 | [API 契约](backend/API.md) |
| 镜像、静态版本、网关和回退 | [部署指南](backend/docs/deployment.md) |

目录 `frontend/` 与 `backend/` 分别是独立构建上下文，不需要读取相邻平台或 SDK 源码仓库。`backend/manifests/` 提供应用登记与授权示例，`backend/migrations/` 保存业务库的不可变迁移。SDK、共享指标库与部署助手的固定摘要由 `backend/dependencies.lock.json` 记录。

开发工具版本为 Node.js 24.22.1、npm 11.16.0、Rust 1.97.0。前端默认端口 3013，API 默认端口 3004，平台访问路由为 `/micro/work-orders`，业务 API 前缀为 `/work-orders-api`。平台、机密客户端、功能权限和数据授权需要事先准备，直接打开微应用静态入口不会生成登录身份。

后端正式运行入口是 Rust。保留的 Node 模块用于历史兼容性与迁移测试，`backend/index.mjs` 会拒绝作为服务启动。在 `backend/` 执行 `npm test` 会运行实际存在的 Node 测试；Rust 验证、受限机器的测试 HTTP 监听选项和需要隔离数据库的测试边界见开发指南。

[版本与安全更新记录](RELEASE.md)。
