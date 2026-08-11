# EPD Agent 与设备工作台

Rust Agent `0.2.x` 与内嵌 React 工作台，为 ESP32 E-Paper BLE Protocol v4 提供数据 Producer、设备同步和本地管理。

```text
Browser -> 127.0.0.1 HTTP/SSE -> Rust Agent -> BLE v4 -> ESP32
                                      |
                                      +-> Producer Registry
                                           -> ResourcePublisher
                                           -> SyncCoordinator
```

Agent 是唯一 BLE 主机。React 不使用 Web Bluetooth，不直接访问云服务。Codex Producer 通过本机 `codex app-server` 读取现有登录并发布 `codex.rate_limits/v1`。

## 核心模块

- `agent/src/producer.rs`：Producer manifest、context、control 与编译期 Registry；
- `agent/src/publisher.rs`：revision、hash、heartbeat、reconcile 和串行 Resource 写；
- `agent/src/coordinator.rs`：battery auto-sync cycle 与唯一 `system.sync.complete`；
- `agent/src/codex.rs`：Codex app-server 客户端、采集和 schema 投影；
- `agent/src/ble.rs`：v4 扫描、重新配对、分帧、RPC 和重连；
- `agent/src/web.rs`：loopback Axum API、SSE、本地 session 与静态资源；
- `src/App.tsx`：动态 Page/Binding、Resource JSON、Producer、安全和诊断界面。

新增 Producer 或 Page 通常不修改 React：Page 表单来自 `capabilities.pages`，Producer 列表来自 `producers[]`。完整注册规则见固件仓库的[功能组件开发规范](../esp32-epd-kit/docs/feature_component_development.md)。

## 本地 API

主要 v4 路由：

| Route | 用途 |
|---|---|
| `GET /api/v1/snapshot` | Agent、设备、`producers[]` 和日志 |
| `POST /api/v1/producers/{id}/refresh` | 按 Producer ID 刷新 |
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

运行包含内嵌 Web 产物的 Agent：

```bash
bun run dev:agent
```

React 热更新：

```bash
bun run dev
```

Vite 将 `/api` 代理到 `http://127.0.0.1:38473`。需要有效 session 时，使用 Agent 打印的 tokenized URL，并把 origin 替换为 Vite origin。

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

## 本地安全

- listener 固定 loopback；
- 无 CORS；
- mutation 校验 loopback Origin；
- installation secret 保存于用户配置目录的私有文件；
- URL fragment 一次性交换 `HttpOnly; SameSite=Strict` cookie；
- 浏览器无 BLE、Codex stdio 或云凭据访问能力。

协议与数据契约见 [BLE Protocol v4](../esp32-epd-kit/docs/ble_protocol_v4.md)、[v4 架构](../esp32-epd-kit/docs/architecture_v4.md)和 [Codex schema](../esp32-epd-kit/docs/openai_codex_usage.md)。
