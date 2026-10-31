# tg-via-bot-cleaner

清理群里的**游客模式机器人消息**，并把「谁召唤的、发了什么」落盘留证，方便事后人工核验再决定要不要踢人。

Rust 写的单文件二进制，长轮询，不需要公网 IP。

---

## 判定逻辑

两种来源，都靠「谁按的发送键」区分：

| 触发方式 | `from` | 关键字段 | 处理 |
| --- | --- | --- | --- |
| **游客模式**（机器人不在群里，被 @ 召唤） | 机器人 | `guest_bot_caller_user` = 召唤者 | 删除 + 记录召唤者 |
| **inline 模式**（人在输入框选内容自己发） | 真人 | `via_bot` = 被召唤的机器人 | 删除 + 记录发送人 |
| 群内常驻机器人自己发言 | 机器人 | 无 `guest_bot_caller_user` | 跳过 |
| 普通用户消息 | 真人 | 无 `via_bot` | 跳过 |
| 频道自动转发 / 匿名管理员 / 频道身份 | — | `sender_chat` 有值 | 跳过 |

第三行是关键：群内常驻机器人发消息时 `from` 也是机器人，但**没有 caller 字段**，正好和游客模式区分开。所以不需要维护群成员列表，也不用查 `getChatMember`。

白名单命中的机器人一律放行（`ALLOWED_BOTS`，支持 `@xxxbot` 和数字 ID 混写）。

---

## 启动自检

启动时校验入群权限、隐私模式、群内删除权限，异常项直接输出到控制台：

```
=== 自检 ===
can_join_groups=true
can_read_all_group_messages=true（隐私模式关闭）
chat=-1001234567890 status=administrator can_delete_messages=true
chat=-1009876543210 status=administrator can_delete_messages=false
=== 自检结束 ===
```

单独跑自检（不进轮询）：

```bash
cargo run --release -- check
```

只调用 `getMe` 和 `getChatMember`，不读写任何消息。群权限检查依赖 `ALLOWED_CHATS`。

---

## 日志

每条命中输出一行，字段固定：

```
chat=-1001234567890 msg=8841 trigger=guest bot=@ad_bot#900 caller=@alice#7021234567 deleted=true text=低价代充…
```

`trigger` 为 `guest`（游客模式）或 `inline`；`caller` 后面是数字 ID，改用户名不影响可追溯性。

需要留档直接重定向：

```bash
cargo run --release >> bot.log 2>&1
```

---

## 排查：到底有没有收到消息

内置一个**只读**诊断命令，不删任何东西，可以放心在线上环境跑：

```bash
cargo run --release -- probe
```

它会打印 `getMe` 的全部字段和一次 `getUpdates` 的原始返回。三个关键字段：

| 字段 | 期望 | 说明 |
| --- | --- | --- |
| `can_read_all_group_messages` | `true` | 隐私模式已关闭 |
| `can_join_groups` | `true` | 允许被拉进群 |
| `supports_guest_queries` | 无所谓 | 这是**本 bot 自己**能不能当访客机器人，跟清理别人无关 |

拿到空数组就是当时没有待处理更新——去群里发一条消息，再跑一次 `probe`。

### 测游客模式要拿别人的 bot 测

`@nobotmessage_bot` 这类清理用的 bot 自己 `supports_guest_queries` / `supports_inline_queries` 都是 `false`，**拿它自己召唤永远不会产生 guest 消息**。要测就得找一个在 BotFather 里开了 Guest Mode 的第三方 bot，在群里 `@它` 一下，再看 `probe` 有没有收到。

### 实时看原始流量

```bash
DEBUG_RAW=true RUST_LOG=debug cargo run --release
```

每次 `getUpdates` 的原始响应都会打出来，包括 SDK 会丢掉的部分。看到 `guest_message` 说明路子对了；只看到 `message` 说明这条走的是另一条通道；什么都没有说明 bot 压根没收到。

---

## 使用前的必做配置

**1. 把 bot 加进群，设为管理员，勾上「删除消息」**

不加管理员，`deleteMessage` 会报 `not enough rights`；非管理员也收不到群里的全部消息。

**2. 关掉 BotFather 的隐私模式**

给 @BotFather 发 `/setprivacy` → 选你的 bot → `Disable`。管理员身份可绕过该限制，但仍需关闭，避免撤销管理员后收不到消息。

**3. 关掉隐私模式后，把 bot 移出群再拉回来**

隐私模式的变更只对「新加入」生效，已经在群里的 bot 不会更新。

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

- 基础镜像是 `alpine:3.21`，内含一个 `musl` 目标编译的二进制，约 13MB（alpine 本身同为 musl，静态或动态链接都能直接跑）
- 以非 root 用户 `cleaner` 运行，工作目录 `/app`
- 不监听端口、不写任何文件，没有 healthcheck，可以加 `--read-only`

子命令直接追加在镜像名之后：

```bash
docker run --rm --env-file .env ghcr.io/<owner>/tg-via-bot-cleaner:latest check
```

构建流程：

```
test（clippy + test）→ build（matrix: amd64/arm64 交叉编译 musl）→ 打包 push → 合并 manifest
```

二进制先交叉编译再打包，多架构镜像不需要在 QEMU 里重跑一遍构建。手动触发：

```bash
gh workflow run docker.yml -f tag_name=v0.2.0
```

---

## 已知限制

- **群内其他 bot 能否看到游客模式消息，官方文档没写明。** Guest Bots 一节只说被召唤的 bot 能「在不是成员的聊天里发一条回复」，没提这条消息对群里其他 bot 是否可见。如果实测发现收不到，`DEBUG_RAW` 会显示得清清楚楚——这种情况下 bot 层面就没有办法了，只能靠管理员手动处理。
- **记录只打控制台，不落盘。** 想留档就重定向输出，自己配轮转。
- **删除需要管理员权限和时效。** 过旧的消息 Telegram 会拒绝删除，失败只记 `warn`。
- 高频删除可能撞限流，代码里对 `retry_after` 做了等待后重试一次。
