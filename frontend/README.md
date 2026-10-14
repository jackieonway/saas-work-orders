# 独立工单前端

Vue 3.5.43 业务应用，应用编码为 `work-orders`。运行在默认 opaque iframe 中，接收 MicroProps 2.0，通过真实 `/work-orders-api` 完成工单操作。没有浏览器模拟数据、平台 session 存储或客户端密钥。

## 构建

使用 Node 24。在本目录执行 `npm ci` 安装已锁定的 esbuild 0.25.12，然后执行：

```sh
node scripts/build.mjs --version 1.0.0 --allowed-host-origin https://platform.example.com
```

宿主来源必须由部署方显式配置，支持重复 `--allowed-host-origin`。也可使用 `WORK_ORDERS_ALLOWED_HOST_ORIGINS`（逗号分隔）、`WORK_ORDERS_VERSION`、`WORK_ORDERS_API_BASE`。业务 API 默认 `/work-orders-api`，可用 `--api-base` 指定独立 HTTPS 地址。远程宿主与绝对 API 地址必须使用 HTTPS，HTTP 仅允许字面 `localhost`、`127.0.0.1`、`[::1]` 回环地址用于本机开发和隔离验收。不会从消息、referrer 或 iframe origin 猜测可信宿主。

默认产物是 `dist/index.html`，资源全部为相对路径。`--out-dir` 支持系统临时目录内的测试输出；已有输出需有本应用 `build-info.json` 才会覆盖。仓库开发时可以复用宿主已经安装的同版 esbuild；独立源码构建使用本目录 `npm ci`。

产物可通过平台管理页面上传 ZIP/tar.gz，或在仓库根目录使用 `scripts/package-microapp.mjs` 生成不可覆盖版本目录和登记清单。命令行打包不会自动登记、预检或激活：

```sh
node scripts/package-microapp.mjs --app work-orders --environment development --version 1.0.0 --input business-apps/work-orders/frontend/dist
```

Vue 和 iframe SDK 固定在 `vendor/`，构建为 classic script；不要求 iframe 的 ES module CORS，也不需要运行时模板编译。Vue MIT 许可包含在产物中。SDK 升级时应核对共享契约并重新登记前端版本。

## 业务操作

- 列表支持状态和优先级筛选、分页及刷新。可见范围使用“我创建的工单”“当前组织”“当前组织及下级组织”“当前租户”说明。
- 创建只填写标题、描述、优先级与可选处理人；处理人目录支持姓名搜索及每页 50 人的翻页，租户、组织和创建人由服务端确定。
- 编辑携带原修订号。HTTP 409 会保留用户输入并显示最新内容，需明确选择新修订后再保存。处理人不会扩大 SELF 范围；本人记录也必须位于平台允许的组织范围内，处理人调整还需要对应组织范围授权。
- 编辑仅显示当前状态及允许的下一状态：待处理→处理中→已解决，已解决可重新处理。删除前明确确认，删除后从列表、详情和导出中移除，业务库保留历史。
- CSV 从真实 export API 拉取并限制为 4 MiB，交给宿主 `downloadCsv` broker。iframe 保留 `allow-scripts allow-forms`，不增加 `allow-same-origin` 或下载沙箱权限。
- 详情中的提醒是独立 POST `/tickets/:id/remind`，不会再次保存工单。每张工单保留自己的未知结果事件键，往返其他工单后重试仍沿用该键；提醒对象变化导致 409 时，先刷新当前处理人，再由用户明确发起新提醒。

所有操作按钮同时检查对应功能权限和独立数据 grant。后端逐请求内省、当前上下文及资源归属检查是授权依据。组织切换、续期、到期时中止旧请求并清除旧服务端数据；同一身份的编辑草稿保留，切换身份则清空。

业务 API 返回 401 时立即取消当前请求并隐藏服务端视图。列表读取 403 清除列表及关联快照，详情读取 403／404 清除该记录；候选成员 403 仅禁改处理人。写入 403 保留输入并暂停该操作，等待平台提供新上下文。普通网络失败、400 或 409 不触发认证失效。

## 验证

`npm test` 验证前端权限表达、取消与迟到响应、HTTP 409、CSV BOM/容量和独立提醒 DTO。这些是客户端单元测试，不是平台或数据库验收。

真实 Vue iframe、平台接口、独立业务接口、两份 PostgreSQL 数据库、Socket.IO、CSV 下载及版本切换由 `../tests/platform-e2e.mjs` 的联合流程验收。未经该真实服务流程通过，不能将单元测试视作试点完成证据。
