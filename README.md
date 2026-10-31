# TG-Via-Bot-Cleaner

清理群里的**游客模式机器人消息**

---

## 使用前的必做配置

**1. 把 bot 加进群，设为管理员，勾上「删除消息」**

不加管理员，`deleteMessage` 会报 `not enough rights`；非管理员也收不到群里的全部消息。

**2. 关掉 BotFather 的隐私模式**

给 @BotFather 发 `/setprivacy` → 选你的 bot → `Disable`。管理员身份可绕过该限制，但仍需关闭，避免撤销管理员后收不到消息。

**3. 开启接收Bot消息**

使用 @BotFather 小程序 → 选你的 bot → Bot Settings -> 启用 Bot to Bot Communication Mode

---

## 配置

| 变量 | 说明 | 默认 |
| --- | --- | --- |
| `BOT_TOKEN` | BotFather 给的 token | 必填 |
| `DEBUG_RAW` | 打印原始响应，排查用 | `false` |
| `NOTICE` | 删除后是否回提示 | `true` |
| `NOTICE_TTL_SECS` | 提示几秒后撤回，`0` 不撤 | `30` |
| `ALLOWED_BOTS` | 白名单机器人 | 空 |
| `ALLOWED_CHATS` | 只处理这些群，同时用于启动时逐群验权限 | 全部 |
| `RUST_LOG` | 排查用 `debug` | `info` |

---

## 运行

```bash
cp .env.example .env
cargo run --release
```

首次运行前执行 `cargo run --release -- probe`，核对 `trigger` 与 `caller` 字段是否符合预期，再启动主程序。

---

## Docker

镜像在推送 `v*` tag 时由 GitHub Actions 构建，推到 GHCR，同时提供 `amd64` / `arm64`，合并为 manifest list。

```bash
docker run -d --name tg-cleaner \
  --restart unless-stopped \
  --read-only \
  --env-file .env \
  ghcr.io/<owner>/tg-via-bot-cleaner:latest
```

---

## 已知限制

- 高频删除可能撞限流，代码里对 `retry_after` 做了等待后重试一次。建议自行部署个人机器人而不是使用公共部署的机器人。
