# 工单前端

Vue / Vite 独立微应用，固定 SDK 1.0.0，默认开发端口 3013。页面由已登记的平台宿主提供上下文，没有独立登录或浏览器端客户端 secret。

在本 frontend 目录执行：

```powershell
npm ci
$env:WORK_ORDERS_ALLOWED_HOST_ORIGINS='http://127.0.0.1:3000'
$env:WORK_ORDERS_API_BASE='http://127.0.0.1:3004/work-orders-api'
npm run dev
```

先按 [开发指南](../backend/docs/development.md)配置平台和 API。没有宿主握手时显示等待连接，不构造业务身份。

## 测试与构建

保留上述公开环境配置，再执行：

```sh
npm test
npm run build -- --version 1.0.0 --allowed-host-origin https://platform.example.com --api-base /work-orders-api
npm run preview
```

npm ci 只安装依赖，build 生成 dist 及构建元数据。公开参数仅允许版本、准确宿主来源和 API 地址，远程地址需要 HTTPS，回环开发可用 HTTP。preview 也需对应配置。

发布完整 dist，使用固定版本 URL、相对资源和真实缺资源 404。切身份/组织、失权及到期后取消旧请求并隔离旧数据；写入冲突与未知结果按 [API](../backend/API.md)显式恢复。

镜像从本目录构建，APP_CODE、APP_VERSION、HOST_ORIGIN、API_BASE 都需要真实值，完整命令见 [部署指南](../backend/docs/deployment.md)。
