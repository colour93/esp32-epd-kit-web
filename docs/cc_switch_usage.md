# CC Switch 今日用量源

EPD Agent 内置 `ccswitch.usage` 单实例 producer。实例 ID 为 `cc-switch`，发布资源 `ccswitch/metrics`，schema 为 `generic.metrics/v1`。该源不可配置、参与设备自动同步，每 300 秒刷新一次，资源 TTL 为 900 秒。

## 数据库与只读保证

Windows 默认读取 `%USERPROFILE%\.cc-switch\cc-switch.db`。可通过 `CC_SWITCH_DB` 指向其它数据库文件，例如：

```powershell
$env:CC_SWITCH_DB = 'D:\Data\cc-switch.db'
```

Agent 在打开前确认文件已存在，并使用 SQLite `READ_ONLY` 标志连接。它不会创建数据库、表或 migration，也不会执行任何写语句。数据库读取在 blocking worker 中进行，不阻塞 Agent 的异步循环。

## 统计口径

今日边界使用 SQLite 主机本地时区：

```sql
SELECT
  COUNT(*),
  COALESCE(SUM(
    COALESCE(input_tokens, 0) +
    COALESCE(output_tokens, 0) +
    COALESCE(cache_read_tokens, 0) +
    COALESCE(cache_creation_tokens, 0)
  ), 0)
FROM proxy_request_logs
WHERE date(created_at, 'unixepoch', 'localtime') = date('now', 'localtime');
```

统计直接来自 `proxy_request_logs`，不使用可能滞后的 `usage_daily_rollups`。Token 总量按十进制单位显示并保留至多一位小数，例如 `999`、`1K`、`64.5K`、`1.3M`；指标描述同时显示今日请求数。

## 健康状态

- `ready`：查询和资源发布成功；
- `missing`：数据库文件或 `proxy_request_logs` 表不存在；
- `degraded`：数据库无法只读打开、schema 不兼容、查询失败或资源发布失败。

失败时只更新 source 健康状态，不发布空资源或删除已有资源。上一次成功的资源因此会继续保留，直到新的成功快照覆盖它或设备端 TTL 到期。
