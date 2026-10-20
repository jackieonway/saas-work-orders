# 独立工单前端

Vue 3.5.43 业务应用，应用编码为 `work-orders`。运行在默认 opaque iframe 中，接收 MicroProps 2.0，通过真实 `/work-orders-api` 完成工单操作。没有浏览器模拟数据、平台 session 存储或客户端密钥。

## 构建

使用 Node **24.18.0**、npm **11.16.0**。本目录独立锁定 Vue **3.5.43** 与 Vite **8.3.4**；在本目录执行 `npm ci`，然后构建：

```sh
npm run build -- --version 1.0.0 --allowed-host-origin https://platform.example.com
```

宿主来源必须由部署方显式配置，支持重复 `--allowed-host-origin`。也可使用 `WORK_ORDERS_ALLOWED_HOST_ORIGINS`（逗号分隔）、`WORK_ORDERS_VERSION`、`WORK_ORDERS_API_BASE`。业务 API 默认 `/work-orders-api`，可用 `--api-base` 指定独立 HTTPS 地址。远程宿主与绝对 API 地址必须使用 HTTPS，HTTP 仅允许字面 `localhost`、`127.0.0.1`、`[::1]` 回环地址用于本机开发和隔离验收。不会从消息、referrer 或 iframe origin 猜测可信宿主。

`vite.config.mjs` 使用 `base: './'` 与标准 `index.html` → `src/main.mjs` 入口。薄main同步登记 `MicroIframe.connect` 监听，先接住iframe加载时唯一的MessagePort，再由lifecycle shell异步加载真实业务chunk并调用导出的 `createLifecycle(bridge)`；Vue仍通过模块导入。shell在加载后复验当前signal，避免取消／销毁后的迟到mount；首次加载被旧上下文取消时，新鲜update仍可完成第一次实际mount，unmount保持幂等。登记SDK已有的异步生命周期／取消协议和vendor字节不变，构建仍产生真实ESM动态chunk及带摘要文件名的CSS／JavaScript。源码构建只使用本目录依赖，不读取宿主node_modules、全局window.Vue或远程CDN。

默认产物是 `dist/index.html`，入口及 chunk 资源全部相对于当前不可变版本目录。`--out-dir` 支持系统临时目录内的测试输出；已有输出需有本应用 `build-info.json` 才会覆盖，自定义路径和符号链接在写入前校验。构建不会清理平台已有版本目录或旧 chunk。

`asset-manifest.json` 记录入口、动态导入和 CSS 的实际关系。`build-info.json` 记录应用版本、SDK/契约版本、Vue/Vite/Node版本、锁文件摘要，以及构建输入的逐文件 SHA-256 与总摘要。源码摘要描述实际工作树，不能仅凭应用版本号声称它来自某个 Git 提交。公开 `assets/config.js` 仅含版本、宿主 origin 白名单和业务 API 地址；任何 `VITE_*` 值、client secret、数据库 URL 或平台 session 都不会由构建配置自动注入。

开发可在设置 `WORK_ORDERS_ALLOWED_HOST_ORIGINS` 后执行 `npm run dev`；`npm run preview` 只用于本地检查已构建产物。生产通过静态 Nginx 提供整个版本目录，Node/Vite 不参与生产请求。Nginx 必须对模块、动态 chunk、CSS 和 classic bridge 提供正确 MIME 及匿名 CORS，缺失资源返回真实 404，不得用入口 HTML 吞掉错误。opaque iframe 不增加 `allow-same-origin` 或下载权限。

产物可通过平台管理页面上传 ZIP/tar.gz，或在仓库根目录使用 `scripts/package-microapp.mjs` 生成不可覆盖版本目录和登记清单。命令行打包不会自动登记、预检或激活：

```sh
node scripts/package-microapp.mjs --app work-orders --environment development --version 1.0.0 --input business-apps/work-orders/frontend/dist
```

登记 SDK **1.0.0** 的 `iframe-bridge.js` 与 `micro-sdk.mjs` 固定在 `vendor/`。构建按原字节输出 classic iframe bridge，业务客户端引用同版的模块 SDK；Vue来自锁定的模块依赖，不复制旧 `vue.global.prod.js`。Vue MIT 许可包含在产物中。ESM 与动态 chunk 必须在真实 opaque iframe 中验收匿名 CORS。SDK 升级时应核对共享契约并重新登记前端版本。

## 业务操作

- 创建使用独立路径 `/micro/work-orders/ticket-create`；详情使用 `/micro/work-orders/tickets/:id`。两者需要各自显式授权的 MENU，根入口和详情读权限不包含创建入口。旧 `/tickets/new` 只按详情标识 `new` 处理，不打开创建表单；创建按钮导航与直达入口使用同一个新路径。平台菜单、bootstrap 和管理员工具按此规范同步，采用新规范需要新的前端发布版本，旧创建路由版本不能直接作为兼容回滚对象。
- 列表支持状态和优先级筛选、分页及刷新。可见范围使用“我创建的工单”“当前组织”“当前组织及下级组织”“当前租户”说明。
- 创建只填写标题、描述、优先级与可选处理人；处理人目录支持姓名搜索及每页 50 人的翻页，租户、组织和创建人由服务端确定。
- 编辑携带原修订号。HTTP 409 会保留用户输入并显示最新内容，需明确选择新修订后再保存。处理人不会扩大 SELF 范围；本人记录也必须位于平台允许的组织范围内，处理人调整还需要对应组织范围授权。
- 编辑仅显示当前状态及允许的下一状态：待处理→处理中→已解决，已解决可重新处理。删除前明确确认，删除后从列表、详情和导出中移除，业务库保留历史。
- CSV 从真实 export API 拉取并限制为 4 MiB，交给宿主 `downloadCsv` broker。iframe 保留 `allow-scripts allow-forms`，不增加 `allow-same-origin` 或下载沙箱权限。
- 创建使用Rust要求的 `Idempotency-Key` header，首次提交固定一次逻辑事件的键和完整DTO；字段在发送及结果未知期间锁定，用户通过“恢复原事件”显式发送同键同正文。只有预期状态与完整合法成功DTO通过校验才结束事件，畸形2xx、网络、协议或5xx均保留为未知结果，不自动换键重建工单。
- 详情提醒为独立POST `/tickets/:id/remind`，正文仅 `{idempotencyKey}`，不会再次保存工单。每张工单保留自己的未知事件键，往返其他工单后显式重试仍沿用该键；PREPARATION_UNKNOWN明确本次零平台POST，RESULT_UNKNOWN可能已经发布。409时先刷新并确认当前目标，再由用户明确发起新提醒，不能为补Toast自动换键。

所有操作按钮同时检查功能权限和独立data grant，后端逐请求内省／当前上下文及资源归属检查仍为授权依据。组织切换、真实授权替换和到期中止旧请求并清除可见服务器数据；同一业务身份的合法草稿可保留，切换身份隐藏旧域并只恢复新域自己的内存状态，不显示其他域草稿或复用其事件键。

同 epoch、凭据、到期时间、身份、权限/grant/菜单/组织上限、发布版本和宿主 signal 均未改变的主题/路由更新复用授权 controller。纯主题变化不取消保存、不清空缓存、不重新拉取列表，也不能清除服务端 403 后暂停的操作。路由变化只取消离开的列表/详情读取并启动相关读取，独立保存继续执行；迟到详情不能回写新路由。保存后若用户已离开原路由，只报告结果，不覆盖当前详情。

真实授权替换立即清除列表、详情、处理人目录、删除确认和编辑器的服务端快照；同一身份可保留用户草稿及最小资源范围标识，重新取得有读取权限的工单前暂停保存。撤回 read 或 write grant 会关闭不再可访问的编辑器，撤销成员身份/宿主 signal 会停用整个视图。当前 contextVersion 相同不能掩盖权限缩减，前端检查也不能代替后端逐请求授权。

业务 API 返回 401 时立即取消当前请求并隐藏服务端视图。列表读取 403 清除列表及关联快照，详情读取 403／404 清除该记录；候选成员 403 仅禁改处理人。写入 403 保留输入并暂停该操作，等待平台提供新上下文。普通网络失败、400 或 409 不触发认证失效。

<a id="recovery-lifetime"></a>
## 内存事件与恢复生命周期

`IntentStore`和草稿只存在于当前仍存活iframe的模块内存，按appCode／tenantId／organizationId／userId分区，不保存token、完整MicroProps或平台session，不使用opaque localStorage，也不新增SDK持久化协议。跨路由、同业务身份的授权续期及切到其他组织再返回，可以恢复相应域内保留的合法草稿和未知事件。刷新、关闭页面、销毁iframe或更换发布实例不保证保留这些内存数据；创建／提醒结果未知时请保持页面，直至使用当前有效授权显式核对完成。后端0003提醒intent和创建receipt是独立持久保证，不能把它们描述成浏览器存储。

创建未知后，即使后续401／403也不能自动丢掉原事件换键。恢复前重新取得当前域有效create权限及grant，只发送首次固定的header键／完整正文；预期201和完整工单DTO校验成功后才结束事件。提醒未知恢复同样只用原JSON键；严格校验200的publicationId、两个值为1的计数、布尔duplicate与RFC3339时间，畸形成功JSON或错误2xx状态归协议未知，不能提示已发送或清事件。503业务code只读取有界8 KiB白名单来区分准备／发布unknown，远端正文不作为提示输出，也未改变登记SDK协议。

编辑409保留草稿，显示最新详情，由用户选择“使用最新版本继续编辑”后再保存；WRITE_RESULT_UNKNOWN先选择“读取当前详情核对”，不自动重发PATCH。状态变更与删除未知也先核对当前状态／可见性；删除的未确认状态按工单保留，不能靠切到另一工单、续期或核对另一条记录解除它。读取到404只说明当前不可见，不能单凭该结果断言删除提交或自动再次删除。恢复操作仍需当前授权。

## 验证

`npm test` 验证前端权限表达、取消与迟到响应、HTTP 409、CSV BOM/容量和独立提醒 DTO，并执行独立 Vite 构建、相对 ESM/动态 chunk、源码/SDK 元数据、秘密环境哨兵不泄露、输出目录保护，以及版本前缀下的静态 HTTP 与真实 404 回归。这些客户端及构建测试不能替代真实平台、数据库或 opaque iframe 验收。

Phase21真实浏览器入口为 `../../../tests/phase21-work-orders-browser.mjs`，由配套services helper提供实际平台、独立Rust工单API、两座自有PostgreSQL、生产宿主／静态Nginx及TLS。其验证清单包括opaque module／动态chunk、Origin:null／无Cookie、普通用户CRUD／成员／CSV／持久提醒、提交后响应丢失的显式恢复、MENU互斥和失权隐藏；实时通知服务可保持未启动，持久收件箱单独核对。实际运行、资源清理和数量由 [Phase21报告](../../../docs/validation/phase-21.md)记录，不预记通过。

旧 `../tests/platform-e2e.mjs` 保留其完整兼容业务／通知／版本切换联合流程，不以旧Node服务或历史证据代替当前Rust前端验收；轻量Phase8竞态回归也保留独立边界。VM／构建HTTP、受控浏览器与真实平台联合结果分别记录，未执行不能计通过。


## 版本化 SDK

`vendor/iframe-bridge.js` 与 `vendor/micro-sdk.mjs` 来自同一个登记 SDK **1.0.0**，保持 context 1.0、MicroProps/iframe 2.0。`WorkOrdersClient` 保留工单列表/写入/提醒/CSV 方法、401 立即清空视图以及迟到响应拒绝规则，通用 HTTP 传输复用该版本的 `requestWithContext`。构建元数据包含 `sdkVersion`。

JSON 与 CSV 都按流限制为 4 MiB，CSV 保留 UTF-8 BOM；请求强制省略 cookies，传入当前应用 Bearer 与 UUID `X-Request-Id`。独立业务 API 的 CORS 必须显式允许请求 `X-Request-Id` 并暴露响应 `X-Request-Id`，参考后端已支持；保持精确 origin 且不开放 credentials。`BusinessError` 继续提供工单 UI 使用的 status/code，并增加 kind/requestId，区分认证、禁止、冲突、网络及协议错误；错误不展示远端响应文本。工单特有的组织硬上限、功能权限与独立 grant 仍由 domain 和业务后端执行。

vendor 是生成产物，请在仓库根执行 `npm run sdk:sync` 更新、执行 `npm run sdk:check` 验证；不要手改 vendor 或独立复制一份桥。源码独立构建所需的 SDK 文件已经随 vendor 提交，不依赖宿主运行时的 node_modules 或平台 storage。新 SDK 必须随新的不可变工单前端版本登记，旧发布目录保留以支持激活和回滚。完整支持能力和兼容策略见 [SDK 接入说明](../../../examples/micro-app-sdk/README.md)。
