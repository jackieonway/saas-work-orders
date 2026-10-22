# 工单项目文档

本仓库交付 Vue 微应用、Rust API、数据库迁移和独立 Compose。构建不读取平台源码；运行时使用平台身份、授权和受控成员目录。

| 目标 | 入口 |
| --- | --- |
| 在本地启动前后端 | [开发指南](development.md) |
| 构建镜像和部署 | [部署指南](deployment.md) |
| 了解前端命令 | [前端说明](../../frontend/README.md) |
| 了解后端结构和配置 | [后端说明](../README.md) |
| 调用接口或处理未知结果 | [API 契约](../API.md) |
| 平台登记和授权 | [应用清单](../manifests/bootstrap.manifest.json)、[授权示例](../manifests/grants.manifest.example.json) |

应用 code 固定为 work-orders。数据库与账号独立，机密客户端凭据只放后端。npm ci 安装依赖，npm run build 构建前端，cargo build --release --locked 构建 API。

仓库 .github 只验证和构建自身，不部署平台或其他业务。SDK 与指标协议来源在 backend/dependencies.lock.json，升级需要审阅兼容性并更新摘要。
