# 工单管理发布日志

## 2026-10-23 · Node.js 安全更新

- 前端包 `work-orders-frontend`：`2.0.0` → `2.0.1`；后端 Node 兼容测试包 `work-orders-api`：`1.0.0` → `1.0.1`。
- Node.js 基线由 `24.18.0` 升级至官方安全修复版本 `24.22.1`。前端 `.nvmrc`、`.node-version`、Docker 构建镜像和开发/部署文档统一使用 `24.22.1`；前端与后端 Node 兼容测试包的 package/lock engines 最低版本收紧至 `>=24.22.1 <25`。
- 修复官方 2026 年 10 月安全公告中影响 Node.js 24.x 的 8 项漏洞：CVE-2026-105482、CVE-2026-105483、CVE-2026-106069、CVE-2026-111497、CVE-2026-105481、CVE-2026-106072、CVE-2026-106074、CVE-2026-106073。
- Rust 服务版本、SDK 契约与既有静态发布产物保持原版本和内容；新产物需使用独立版本 URL 构建、验证后再部署。

依据：[Node.js 官方安全公告](https://nodejs.org/en/blog/vulnerability/october-2026-security-releases)、[Node.js v24.22.1 发布说明](https://nodejs.org/en/blog/release/v24.22.1)。

### 验证

- Node.js `24.22.1` + npm `11.19.1`（满足 engines）：前端 `npm test` 共 88 项通过，无失败或跳过。
- 后端兼容测试包 `npm test`：7 项通过；7 项真实 PostgreSQL 用例因未配置 `TEST_DATABASE_URL` 按既有条件跳过，无失败。
- 前端使用默认版本 `2.0.1` 构建到独立临时目录，自动发布门禁与 `node scripts/release-check.mjs <输出目录>` 均通过。
- `node .github/scripts/check-inputs.mjs` 与 `node --test .github/tests/repository-structure.mjs` 通过（结构测试 1 项）；前后端 package/lock 版本与 engines 一致。
- 本次未运行 Docker 镜像构建或真实浏览器集成验收；未发布或激活部署。
