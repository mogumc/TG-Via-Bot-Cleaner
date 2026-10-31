//! 删除后回一条提示，说明是谁召唤的、删了什么。
//!
//! 广告通常一次 `@` 好几个机器人连发，逐条回复等于二次刷屏，所以同一个召唤者在
//! 窗口内的多次删除只合成一条提示，发出去以后 `ttl` 秒自动撤回。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crate::api::Tg;
use crate::judge::Hit;

/// 合并窗口：第一次删除后等这么久，窗口内的都算一批
pub const WINDOW: Duration = Duration::from_secs(5);

struct Pending {
    caller_name: String,
    /// 去重后的机器人名，同一个人反复召唤同一个机器人时不重复列出
    bots: HashSet<String>,
    count: usize,
    scheduled: bool,
}

struct Inner {
    tg: Tg,
    pending: Mutex<HashMap<(i64, u64), Pending>>,
    ttl: Duration,
}

pub struct Notifier {
    inner: Arc<Inner>,
}

impl Notifier {
    pub fn new(tg: Tg, ttl: Duration) -> Self {
        Self {
            inner: Arc::new(Inner { tg, pending: Mutex::new(HashMap::new()), ttl }),
        }
    }

    /// 记一次删除。同一召唤者在窗口内多次调用只会产生一条提示。
    pub fn record(&self, chat_id: i64, hit: &Hit) {
        let key = (chat_id, hit.caller.id);
        let caller_name = hit.caller.display();
        let bot_name = hit.bot.display();

        let is_first = {
            let mut pending = self.inner.pending.lock().unwrap_or_else(PoisonError::into_inner);
            let entry = pending.entry(key).or_insert_with(|| Pending {
                caller_name,
                bots: HashSet::new(),
                count: 0,
                scheduled: false,
            });

            entry.bots.insert(bot_name);
            entry.count += 1;

            let first = !entry.scheduled;
            entry.scheduled = true;
            first
        };

        if !is_first {
            return;
        }

        let inner = self.inner.clone();
        tokio::spawn(async move {
            tokio::time::sleep(WINDOW).await;
            let batch = {
                let mut pending = inner.pending.lock().unwrap_or_else(PoisonError::into_inner);
                pending.remove(&key)
            };

            if let Some(batch) = batch {
                inner.send_notice(chat_id, batch).await;
            }
        });
    }
}

impl Inner {
    async fn send_notice(&self, chat_id: i64, batch: Pending) {
        let mut bots: Vec<String> = batch.bots.into_iter().collect();
        bots.sort();
        let bots = bots.join("、");

        let text = if batch.count > 1 {
            format!(
                "已删除 {} 通过 {} 发送的 {} 条消息",
                batch.caller_name, bots, batch.count
            )
        } else {
            format!("已删除 {} 通过 {} 发送的消息", batch.caller_name, bots)
        };

        let message_id = match self.tg.send_message(chat_id, text).await {
            Ok(id) => id,
            Err(err) => {
                // 群可能禁了机器人发言，不影响删除本身
                log::debug!("发送删除提示失败 chat={chat_id}: {err}");
                return;
            }
        };

        if self.ttl.is_zero() {
            return;
        }

        // 提示本身也自动消失，别在群里留一堆公告
        tokio::time::sleep(self.ttl).await;
        if let Err(err) = self.tg.delete_message(chat_id, message_id).await {
            log::debug!("撤回删除提示失败 chat={chat_id}: {err}");
        }
    }
}
