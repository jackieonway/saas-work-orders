# 工单前端

本工程使用 Vue 3.5.43、Vite 8.3.4 和平台 SDK 2.0.0，向 qiankun 提供 HTML + classic UMD 微应用。`src/qiankun-entry.mjs` 是唯一正式构建入口，`src/app.mjs` 实现工单页面，`src/client.mjs` 负责带当前应用上下文的业务请求，`src/intents.mjs` 保存当前应用存续期间的写入恢复信息。

## 安装与开发

使用 Node.js 24.22.1、npm 11.16.0，在本目录执行。平台、API 和权限的前置配置见[开发指南](../backend/docs/development.md)。

```powershell
npm ci --ignore-scripts --no-audit --no-fund
$env:WORK_ORDERS_ALLOWED_HOST_ORIGINS='http://127.0.0.1:3000'
$env:WORK_ORDERS_API_BASE='http://127.0.0.1:3004/work-orders-api'
npm run dev
```

默认入口是 `http://127.0.0.1:3013/index.html`。`dev` 先生成生产形态的 UMD，再监听源码构建并用静态预览服务提供文件；改动后刷新平台页面生效。该流程没有原生 ESM 热更新。直接打开入口不会挂载业务页面，业务身份必须来自已登记的平台宿主。

`WORK_ORDERS_ALLOWED_HOST_ORIGINS` 是微应用允许运行的平台页面来源，多个来源用逗号分隔，值必须是无路径、无尾斜杠的精确 origin。`WORK_ORDERS_API_BASE` 是业务 API 根地址，默认 `/work-orders-api`；相对地址在平台页面来源下请求，因此平台需要提供对应代理。远程宿主与 API 仅接受 HTTPS，开发可使用回环 HTTP。前端这些配置与后端的 `WORK_ORDERS_ALLOWED_ORIGINS` 是不同配置项，跨域 API 调用需要两边共同配置。

## 宿主接入契约

在平台将应用 code 登记为 `work-orders`，基础路由登记为 `/micro/work-orders`，入口类型为 `HTML`，运行模式为 `TRUSTED`。登记示例位于 [bootstrap.manifest.json](../backend/manifests/bootstrap.manifest.json)，生产必须替换其开发入口 URL。

qiankun 加载 HTML 后调用 `bootstrap`、`mount`、`update`、`unmount`。宿主的 props 形状为：

```js
{
  platform: microProps,
  services: {
    downloadCsv({ resource, filename, content }) { /* 宿主校验授权后下载 */ }
  }
}
```

`platform` 必须符合 SDK 2.0 的 `MicroProps`，包含正确应用 code/basePath、用户、租户、组织、应用令牌、令牌及上下文到期时间、`contextEpoch`、`contextVersion`、权限、数据授权和有效的 `AbortSignal`。qiankun 注入的 `name`、`container` 等元数据由生命周期适配层处理，不传入严格 SDK 契约。`mount` 在 `props.container` 内定位 `[data-work-orders-mount]`，缺失上下文或容器时拒绝挂载；已取消的上下文不挂载。

页面从 `routePath` 读取当前路由，并通过 `onNavigate` 请求平台导航。支持列表 `/micro/work-orders`、创建 `/micro/work-orders/ticket-create` 和详情 `/micro/work-orders/tickets/:id`。后续路由、主题和授权更新通过 `update` 传入，不依赖 iframe 消息或子应用自行改写浏览器历史。CSV 先由 API 校验 `ticket:export`，再交给宿主下载服务；缺少服务会报错，不生成替代的本地下载通道。

qiankun 的 `TRUSTED` 模式在宿主页面中执行代码。宿主必须信任发布者并配置自己的 CSP、静态资源来源与 API 连接来源。样式、主题变量和对话框限定在 `data-work-orders-root` 内，避免污染宿主全局样式。

## 上下文失效与写入恢复

SDK 请求使用捕获的应用令牌并省略 Cookie。身份、租户、组织、令牌或有效授权改变后，客户端取消旧请求并拒绝迟到响应；401 会立即使当前上下文失效。单纯主题或路由更新可保留同一授权生命周期内的请求。

创建、修改、删除和提醒均不自动重试未知结果。原始幂等事件和草稿仅存在当前应用内存，按应用、租户、组织和用户隔离；刷新、关闭或 `unmount` 会丢失本地恢复信息。409 与各类结果未知的处理规则见 [API 契约](../backend/API.md)。

## 测试、构建与发布

```powershell
npm test
npm run build -- --version 2.0.1 --allowed-host-origin https://platform.example.com --api-base /work-orders-api
npm run release:check -- dist
$env:WORK_ORDERS_ALLOWED_HOST_ORIGINS='https://platform.example.com'
npm run preview
```

`npm test` 运行页面、客户端、授权范围、写入事件、qiankun 生命周期、真实 UMD 构建及静态交付测试。它不启动真实平台和 PostgreSQL，也不能证明生产网关与浏览器联调成功。

构建只向浏览器写入版本、宿主来源白名单和 API 地址，关闭任意 `VITE_*` 注入，默认版本取自 package.json 的 2.0.1。`dist/` 包含 HTML、单个 `assets/micro-app.umd.js`、合并 CSS、Vue 许可证、`asset-manifest.json`、`build-info.json` 与 `release-files.json`。构建信息记录 qiankun、契约 2.0、SDK 版本、源码和锁文件摘要；不产生动态 JS 分块。UMD 初始化结束后重新声明生命周期对象，供 qiankun 正确发现导出。

每次构建后自动门禁检查实际产物：核对四个 UMD 生命周期、SDK 2.0.0、HTML/CSS 与资源清单、版本/公开配置、源码与逐文件摘要，并拒绝旧 iframe/ESM/SDK 1 产物、缺失/变更资源或陈旧构建。`release:check` 可显式复核指定目录，CI 同时执行。门禁执行 VM 时禁止网络访问，只做本仓可信代码的格式与生命周期验收，不作为运行任意第三方代码的安全沙箱。

当前已生成 `2.0.1-qiankun` 本地候选，可用 `npm run release:check -- deploy/micro-apps/work-orders/versions/2.0.1-qiankun/html` 复核。需要生成下一份候选时，按部署指南给 `release:candidate` 选择未使用的新版本；工具在 `deploy/micro-apps/work-orders/versions/` 中只新增目录，拒绝覆盖。旧 `2.0.0-qiankun` 和原 `dist/` 保留，生产 HTTPS 构建与本地候选的区别见部署指南。

完整发布 `dist/`，为版本使用不可变 URL；缺失 JS/CSS 应返回 404，不能回退 HTML。构建仅覆盖自己已有的 `dist/`，自定义 `--out-dir` 只允许系统临时目录内的受控路径。生产静态镜像、缓存、跨域读取和网关步骤见[部署指南](../backend/docs/deployment.md)。

固定 SDK 位于 `vendor/micro-sdk/`，其 `SOURCE.json` 记录来源提交、`workingTree: true` 和逐文件摘要，不能把这一副本描述为仅由提交内容复现的干净发布。构建不读取相邻 SDK 仓库。`vendor/vue.global.prod.js` 属于保留的固定材料，正式 UMD 使用 npm 锁定的 Vue 依赖。旧 iframe bridge、自定义 ESM 生命周期入口和双模式构建已从当前前端源码路径移除。
