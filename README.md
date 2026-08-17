# EPD Agent 与设备工作台

Rust Agent `0.2.x` 与内嵌 React 工作台，为 ESP32 E-Paper BLE Protocol v4 提供数据源、设备同步和本地管理。

```text
Browser -> 127.0.0.1 HTTP/SSE -> Rust Agent -> BLE v4 -> ESP32
                                      |
                                      +-> Producer Registry
                                           -> ResourcePublisher
                                           -> SyncCoordinator
```

Agent 是唯一 BLE 主机。React 不使用 Web Bluetooth，不直接访问云服务。数据源分为两层：`source_types[]` 是静态能力目录，`sources[]` 是具有独立 ID、配置、状态和资源键的实例。Codex 可通过本机 `codex app-server` 读取现有登录，也可用独立 OAuth 多账号直接采集并自动维护 token；CC Switch 内置源从本机数据库只读统计今日 Token；`cli.jmespath` 与 `http.jmespath` 可创建多个实例，把 CLI 或 HTTP JSON 分别投影为通用指标；`platform.balance` 独立查询 DeepSeek 与 Moonshot（Kimi）余额。

## 核心模块

- `agent/src/producer.rs`：Producer manifest、context、control 与编译期 Registry；
- `agent/src/publisher.rs`：revision、hash、heartbeat、reconcile 和串行 Resource 写；
- `agent/src/coordinator.rs`：battery auto-sync cycle 与唯一 `system.sync.complete`；
- `agent/src/codex.rs`：Codex app-server 客户端、采集和 schema 投影；
- `agent/src/codex_oauth.rs`：Codex OAuth、系统凭据、多账号 token 刷新与额度采集；
- `agent/src/ccswitch.rs`：CC Switch 今日 Token 的只读 SQLite 聚合与通用指标发布；
- `agent/src/cli.rs`：多实例 CLI 数据源管理、JMESPath 投影和本机私有配置；
- `agent/src/http.rs`：多实例 HTTP JSON 数据源、网络边界和系统凭据；
- `agent/src/balance.rs`：DeepSeek、Moonshot 等平台余额数据源；
- `agent/src/metrics.rs`：CLI/HTTP 共用的 JMESPath 校验与 `generic.metrics/v1` 投影；
- `agent/src/ble.rs`：v4 扫描、重新配对、分帧、RPC 和重连；
- `agent/src/web.rs`：loopback Axum API、SSE、本地 session 与静态资源；
- `src/App.tsx`：动态 Page/Binding、Resource JSON、数据源类型与实例、安全和诊断界面。

新增数据源类型或 Page 通常不修改 React：Page 表单来自 `capabilities.pages`，类型与实例分别来自 `source_types[]`、`sources[]`。CLI 与 HTTP 实例分别持久化在本机 `cli-sources.json`、`http-sources.json`，发布 `cli/{id}`、`http/{id}`；HTTP 密钥只存系统凭据库。完整注册规则见固件仓库的[功能组件开发规范](../esp32-epd-kit/docs/feature_component_development.md)。

## 本地 API

主要 v4 路由：

| Route | 用途 |
|---|---|
| `GET /api/v1/snapshot` | Agent、设备、`source_types[]`、`sources[]` 和日志 |
| `POST /api/v1/source-types/{id}/refresh` | 刷新某类型的全部实例 |
| `POST /api/v1/sources/{id}/refresh` | 刷新单个数据源实例 |
| `GET /api/v1/source-types/codex.oauth/sources` | 列出 Codex OAuth 账号 |
| `POST /api/v1/source-types/codex.oauth/oauth/start` | 创建 OAuth PKCE 授权会话 |
| `POST /api/v1/source-types/codex.oauth/oauth/complete` | 校验回调 URL、交换 token 并保存账号 |
| `PUT/DELETE /api/v1/source-types/codex.oauth/sources/{id}` | 更新或删除 OAuth 账号 |
| `GET/POST /api/v1/source-types/cli.jmespath/sources` | 列出或创建 CLI 实例 |
| `PUT/DELETE /api/v1/source-types/cli.jmespath/sources/{id}` | 更新或删除 CLI 实例 |
| `GET/POST /api/v1/source-types/http.jmespath/sources` | 列出或创建 HTTP 实例 |
| `PUT/DELETE /api/v1/source-types/http.jmespath/sources/{id}` | 更新或删除 HTTP 实例 |
| `POST /api/v1/source-types/http.jmespath/test` | 测试 HTTP 请求与 JMESPath 投影 |
| `POST /api/v1/device/page` | 提交 PageSettings |
| `GET/PUT/DELETE /api/v1/device/resource` | 读取、owner 发布、删除 Resource |
| `PATCH /api/v1/device/config` | staged patch + commit |
| `POST /api/v1/device/refresh` | 屏幕 auto/full 刷新 |

生产端口为 `38473`；可用 `--port` 或 `EPD_AGENT_PORT` 修改。服务只监听 loopback。

## 开发

```bash
bun install
bun run build
```

开发时先运行 Agent（终端会打印带 token 的 URL）：

```bash
bun run dev:agent
```

再开一个终端运行 Vite，实现 React Fast Refresh/HMR：

```bash
bun run dev
```

Vite 将 `/api`（包括 SSE）代理到 `http://127.0.0.1:38473`。把 Agent 打印 URL 的 origin 从 `http://127.0.0.1:38473` 改为 Vite 显示的 origin（默认 `http://localhost:5173`），保留完整的 `#token=...`；首次访问会建立 session，之后编辑 `src/` 即可热更新。

Agent 单独检查：

```bash
cd agent
cargo check
```

## 打包

```bash
bun run build:agent
cargo install cargo-packager
bun run package:agent
```

构建脚本会运行 Bun production build，并把 `dist/` 嵌入 Rust binary。macOS 使用用户 LaunchAgent，Windows 使用 HKCU Run。
推送 `v` 前缀 tag（如 `v0.2.0`）会构建 macOS arm64 与 Windows x64 二进制和安装包，并上传到对应 GitHub Release。

## 本地安全

- listener 固定 loopback；
- 无 CORS；
- mutation 校验 loopback Origin；
- installation secret 保存于用户配置目录的私有文件；
- URL fragment 一次性交换 `HttpOnly; SameSite=Strict` cookie；
- HTTP source 密钥与 Codex OAuth token 只存系统凭据库，不进入配置文件、浏览器响应或 ESP32；
- 浏览器无 BLE、Codex stdio 或云凭据访问能力。

协议与数据契约见 [BLE Protocol v4](../esp32-epd-kit/docs/ble_protocol_v4.md)、[v4 架构](../esp32-epd-kit/docs/architecture_v4.md)、[Codex schema](../esp32-epd-kit/docs/openai_codex_usage.md)、[HTTP 数据源](../esp32-epd-kit/docs/generic_http.md)和 [CC Switch 今日用量源](docs/cc_switch_usage.md)。

设备通过物理串口执行 `setup` 后会广播 setup 标志 120 秒。Windows 若保留了设备端已经丢失的旧配对，Agent 会在该窗口内首次安全握手失败后清除本机旧记录并重新触发系统配对；正常重连不会清除 bond。
