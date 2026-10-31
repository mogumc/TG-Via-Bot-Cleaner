//! 判定：这条消息要不要删，以及是谁召唤的。
//!
//! 两种来源分开处理，都靠「谁按的发送键」来区分：
//!
//! * **游客模式**（Bot API 10.0）：机器人不在群里，被 `@` 召唤后以机器人身份
//!   直接发言。`from` 是机器人，`guest_bot_caller_user` 才是召唤者。
//! * **inline 模式**：人在输入框里选了机器人的内容自己发出去。`from` 是人，
//!   `via_bot` 是被召唤的机器人。
//!
//! 群内常驻机器人自己发消息时，`from` 也是机器人、但没有 `guest_bot_caller_user`，
//! 正好被排除——不需要维护成员列表。

use crate::api::{Message, User};
use crate::config::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Guest,
    Inline,
}

impl Trigger {
    pub fn label(self) -> &'static str {
        match self {
            Self::Guest => "guest",
            Self::Inline => "inline",
        }
    }
}

/// 一条命中规则的记录
#[derive(Debug, Clone)]
pub struct Hit {
    /// 被召唤的机器人
    pub bot: User,
    /// 召唤者。游客模式取 `guest_bot_caller_user`，inline 取消息发送人
    pub caller: User,
    pub trigger: Trigger,
}

pub fn judge(msg: &Message, config: &Config) -> Option<Hit> {
    if !msg.is_group() {
        return None;
    }

    if let Some(allowed) = &config.allowed_chats {
        if !allowed.contains(&msg.chat.id) {
            return None;
        }
    }

    // 频道自动转发、频道身份或匿名管理员发言，都不该动
    if msg.sender_chat.is_some() || msg.is_automatic_forward {
        return None;
    }

    let from = msg.from.as_ref()?;

    if from.is_bot {
        // 机器人身份发的消息：只有带 caller 才是游客模式召唤；
        // 没有 caller 说明是本群常驻机器人在正常发言，放行。
        let caller = msg.guest_bot_caller_user.as_ref()?;

        if from.is_special() || config.allowed_bots.contains(from) {
            return None;
        }

        return Some(Hit {
            bot: from.clone(),
            caller: caller.clone(),
            trigger: Trigger::Guest,
        });
    }

    // 真人发的消息：有 via_bot 才是 inline 召唤
    let via = msg.via_bot.as_ref()?;

    if via.is_special() || config.allowed_bots.contains(via) {
        return None;
    }

    Some(Hit {
        bot: via.clone(),
        caller: from.clone(),
        trigger: Trigger::Inline,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_allow_list;

    fn config() -> Config {
        Config {
            token: String::new(),
            notice: false,
            notice_ttl: std::time::Duration::ZERO,
            allowed_chats: None,
            allowed_bots: parse_allow_list(""),
            debug_raw: false,
        }
    }

    fn msg(json: &str) -> Message {
        serde_json::from_str(json).expect("测试消息应当能解析")
    }

    #[test]
    fn guest_message_is_hit_with_caller() {
        let msg = msg(
            r#"{
                "message_id": 1,
                "chat": {"id": -100, "type": "supergroup"},
                "from": {"id": 900, "is_bot": true, "first_name": "Ad", "username": "ad_bot"},
                "guest_bot_caller_user": {"id": 7, "is_bot": false, "first_name": "Alice", "username": "alice"}
            }"#,
        );

        let hit = judge(&msg, &config()).expect("游客模式消息应当命中");
        assert_eq!(hit.trigger, Trigger::Guest);
        assert_eq!(hit.bot.display(), "@ad_bot");
        assert_eq!(hit.caller.display(), "@alice");
    }

    #[test]
    fn resident_bot_message_is_ignored() {
        // 机器人发的，但没有 caller —— 群内常驻机器人，放行
        let msg = msg(
            r#"{
                "message_id": 2,
                "chat": {"id": -100, "type": "supergroup"},
                "from": {"id": 901, "is_bot": true, "first_name": "Helper"}
            }"#,
        );

        assert!(judge(&msg, &config()).is_none());
    }

    #[test]
    fn inline_message_is_hit_with_sender_as_caller() {
        let msg = msg(
            r#"{
                "message_id": 3,
                "chat": {"id": -100, "type": "supergroup"},
                "from": {"id": 8, "is_bot": false, "first_name": "Bob", "username": "bob"},
                "via_bot": {"id": 902, "is_bot": true, "first_name": "Gif", "username": "gif"}
            }"#,
        );

        let hit = judge(&msg, &config()).expect("inline 消息应当命中");
        assert_eq!(hit.trigger, Trigger::Inline);
        assert_eq!(hit.caller.display(), "@bob");
    }

    #[test]
    fn plain_user_message_is_ignored() {
        let msg = msg(
            r#"{
                "message_id": 4,
                "chat": {"id": -100, "type": "supergroup"},
                "from": {"id": 9, "is_bot": false, "first_name": "Carol"},
                "text": "hi"
            }"#,
        );

        assert!(judge(&msg, &config()).is_none());
    }

    #[test]
    fn whitelisted_bot_is_released() {
        let mut cfg = config();
        cfg.allowed_bots = parse_allow_list("@ad_bot");

        let msg = msg(
            r#"{
                "message_id": 5,
                "chat": {"id": -100, "type": "supergroup"},
                "from": {"id": 900, "is_bot": true, "first_name": "Ad", "username": "ad_bot"},
                "guest_bot_caller_user": {"id": 7, "is_bot": false, "first_name": "Alice"}
            }"#,
        );

        assert!(judge(&msg, &cfg).is_none());
    }

    #[test]
    fn private_chat_is_ignored() {
        let msg = msg(
            r#"{
                "message_id": 6,
                "chat": {"id": 7, "type": "private"},
                "from": {"id": 900, "is_bot": true, "first_name": "Ad"},
                "guest_bot_caller_user": {"id": 7, "is_bot": false, "first_name": "Alice"}
            }"#,
        );

        assert!(judge(&msg, &config()).is_none());
    }
}
