mod api;
mod config;
mod judge;
mod notice;

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::api::{Me, Tg, Update};
use crate::config::Config;
use crate::judge::judge;
use crate::notice::Notifier;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .init();

    let config = Config::from_env()?;

    // 只读子命令：probe 看原始流量，check 只跑自检
    match std::env::args().nth(1).as_deref() {
        Some("probe") => return probe(&config).await,
        Some("check") => return check(&config).await,
        _ => {}
    }

    let tg = Tg::new(&config.token, config.debug_raw);

    let me = tg.get_me().await.context("getMe 失败，请检查 BOT_TOKEN")?;
    log::info!(
        "已连接 @{}（id={}）",
        me.user.username.as_deref().unwrap_or("?"),
        me.user.id
    );

    self_check(&tg, &config, &me).await;

    match &config.allowed_chats {
        Some(chats) => log::info!("ALLOWED_CHATS: {:?}", chats),
        None => log::info!("ALLOWED_CHATS 未配置，处理所有群"),
    }
    if config.allowed_bots.is_empty() {
        log::info!("白名单: 空");
    } else {
        log::info!("白名单: {}", config.allowed_bots);
    }
    log::info!(
        "NOTICE={}，TTL={}s",
        config.notice,
        config.notice_ttl.as_secs()
    );

    let notifier = Arc::new(Notifier::new(tg.clone(), config.notice_ttl));

    // bot 上挂着 webhook 时 getUpdates 会一直 409
    match tg.delete_webhook().await {
        Ok(()) => log::info!("webhook 已清理，使用 long polling"),
        Err(err) => log::warn!("deleteWebhook 失败: {err}"),
    }

    log::info!("long polling 已启动（timeout=30s）");

    let mut offset = 0i64;
    let mut backoff = 1u64;
    let mut polls = 0u64;
    let mut handled = 0u64;

    loop {
        match tg.get_updates(offset, 30).await {
            Ok(updates) => {
                backoff = 1;
                polls += 1;

                if updates.is_empty() {
                    log::debug!("poll #{polls}: 0 updates（offset={offset}）");
                    if polls.is_multiple_of(10) {
                        log::info!("heartbeat polls={polls} handled={handled}");
                    }
                    continue;
                }

                log::info!("poll #{polls}: {} updates", updates.len());

                for update in updates {
                    offset = update.update_id + 1;
                    handled += 1;
                    handle(&tg, &config, &notifier, update).await;
                }
            }
            Err(err) if err.is_conflict() => {
                log::error!("getUpdates 409：update 流被其他实例占用，10s 后重试");
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
            Err(err) => {
                let wait = err.retry_after().unwrap_or(backoff);
                log::warn!("getUpdates 失败: {err}；{wait}s 后重试");
                tokio::time::sleep(Duration::from_secs(wait)).await;
                backoff = (backoff * 2).min(30);
            }
        }
    }
}

/// 启动自检：确认能收到消息、能删消息
async fn self_check(tg: &Tg, config: &Config, me: &Me) {
    log::info!("=== 自检 ===");

    if me.can_join_groups {
        log::info!("can_join_groups=true");
    } else {
        log::error!("can_join_groups=false，无法加入群组");
    }

    if me.can_read_all_group_messages {
        log::info!("can_read_all_group_messages=true（隐私模式关闭）");
    } else {
        log::warn!("can_read_all_group_messages=false（隐私模式开启），仅接收命令与提及");
        log::warn!("关闭方式：BotFather /setprivacy → Disable，然后将 bot 移出群并重新加入");
    }

    let Some(chats) = &config.allowed_chats else {
        log::info!("ALLOWED_CHATS 未配置，跳过群权限检查");
        log::info!("=== 自检结束 ===");
        return;
    };

    for chat_id in chats {
        match tg.get_chat_member(*chat_id, me.user.id).await {
            Ok(member) => match member.status.as_str() {
                "creator" => log::info!("chat={chat_id} status=creator"),
                "administrator" if member.can_delete_messages => {
                    log::info!("chat={chat_id} status=administrator can_delete_messages=true");
                }
                "administrator" => {
                    log::error!("chat={chat_id} status=administrator can_delete_messages=false");
                }
                other => log::error!("chat={chat_id} status={other}（需要 administrator）"),
            },
            Err(err) => log::error!("chat={chat_id} getChatMember 失败: {err}"),
        }
    }

    log::info!("=== 自检结束 ===");
}

/// 只读自检：检查完就退出，不进入轮询，也不碰任何消息
async fn check(config: &Config) -> Result<()> {
    let tg = Tg::new(&config.token, false);
    let me = tg.get_me().await.context("getMe 失败，请检查 BOT_TOKEN")?;
    log::info!(
        "已连接 @{}（id={}）",
        me.user.username.as_deref().unwrap_or("?"),
        me.user.id
    );

    self_check(&tg, config, &me).await;
    Ok(())
}

/// 只读诊断：把 getMe 和一次 getUpdates 的原始返回打出来。
/// 不删除、不发送任何东西，可以放心在线上环境跑。
async fn probe(config: &Config) -> Result<()> {
    let tg = Tg::new(&config.token, false);

    match tg.get_me_raw().await {
        Ok(raw) => println!("== getMe ==\n{}\n", serde_json::to_string_pretty(&raw)?),
        Err(err) => return Err(err).context("getMe 失败"),
    }

    println!("== getUpdates（只拉一次，不处理） ==");
    match tg.get_updates_raw(0, 0).await {
        Ok(raw) => {
            let count = raw.as_array().map(|arr| arr.len()).unwrap_or(0);
            println!("{}", serde_json::to_string_pretty(&raw)?);
            println!("\n共 {count} 条更新");
            if count == 0 {
                println!("没有待处理的更新。到群里发一条消息（或召唤一个机器人）再跑一次。");
            }
        }
        Err(err) => return Err(err).context("getUpdates 失败"),
    }

    Ok(())
}

async fn handle(tg: &Tg, config: &Config, notifier: &Notifier, update: Update) {
    if config.debug_raw {
        log::debug!(
            "update {}：message={} guest_message={}",
            update.update_id,
            update.message.is_some(),
            update.guest_message.is_some()
        );
    }

    // guest_message 是「机器人被召唤」那条通道，普通群里收到的是 message。
    // 两条都过一遍，防止不同场景走不同字段。
    let Some(msg) = update.message.or(update.guest_message) else {
        return;
    };

    let Some(hit) = judge(&msg, config) else {
        return;
    };

    let deleted = delete(tg, msg.chat.id, msg.message_id).await;

    log::info!(
        "chat={} msg={} trigger={} bot={}#{} caller={}#{} deleted={} text={}",
        msg.chat.id,
        msg.message_id,
        hit.trigger.label(),
        hit.bot.display(),
        hit.bot.id,
        hit.caller.display(),
        hit.caller.id,
        deleted,
        msg.preview()
    );

    if deleted && config.notice {
        notifier.record(msg.chat.id, &hit);
    }
}

async fn delete(tg: &Tg, chat_id: i64, message_id: i32) -> bool {
    match tg.delete_message(chat_id, message_id).await {
        Ok(()) => true,
        Err(err) if err.is_message_gone() => {
            log::debug!("chat={chat_id} msg={message_id}: 消息已不存在");
            false
        }
        Err(err) => {
            let Some(wait) = err.retry_after() else {
                log::warn!("deleteMessage 失败 chat={chat_id} msg={message_id}: {err}");
                return false;
            };

            log::warn!("deleteMessage 429，{wait}s 后重试 chat={chat_id} msg={message_id}");
            tokio::time::sleep(Duration::from_secs(wait)).await;

            match tg.delete_message(chat_id, message_id).await {
                Ok(()) => true,
                Err(err) => {
                    log::warn!("deleteMessage 重试失败 chat={chat_id} msg={message_id}: {err}");
                    false
                }
            }
        }
    }
}
