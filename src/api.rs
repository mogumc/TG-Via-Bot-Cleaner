//! 直接对接 Telegram Bot API 的极简客户端。
//!
//! 为什么不用强类型 SDK：Bot API 迭代很快（当前 10.3），而 teloxide 只支持到 9.1，
//! 遇到它不认识的新 update（比如游客模式的 `guest_message`）会整条丢弃，消息就
//! 静悄悄地消失了。这里所有结构都是**宽松解析**——只取需要的字段，其余一律忽略，
//! 所以 Bot API 以后加多少新字段都不影响收消息。

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Deserialize;

const API: &str = "https://api.telegram.org/bot";

#[derive(Clone)]
pub struct Tg {
    client: reqwest::Client,
    token: String,
    /// 打印每次请求的原始响应，用来确认 Telegram 到底推了什么
    debug_raw: bool,
}

#[derive(Debug)]
pub enum CallError {
    /// Telegram 返回 ok=false
    Api { code: Option<i32>, description: String, retry_after: Option<u64> },
    Http(reqwest::Error),
    Parse(String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api { code, description, .. } => {
                write!(f, "Telegram 返回 {code:?}: {description}")
            }
            Self::Http(err) => write!(f, "网络错误: {err}"),
            Self::Parse(err) => write!(f, "响应解析失败: {err}"),
        }
    }
}

impl std::error::Error for CallError {}

impl CallError {
    /// 限流时返回建议等待秒数
    pub fn retry_after(&self) -> Option<u64> {
        match self {
            Self::Api { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// 消息已经被删掉了（我们和别的管理员同时动手时会出现）
    pub fn is_message_gone(&self) -> bool {
        match self {
            Self::Api { description, .. } => description.contains("message to delete not found"),
            _ => false,
        }
    }

    /// getUpdates 冲突：还有 webhook 挂着，或者另一个实例在抢着拉
    pub fn is_conflict(&self) -> bool {
        match self {
            Self::Api { description, .. } => {
                description.contains("terminated by other getUpdates")
                    || description.contains("webhook is active")
                    || description.contains("can't use getUpdates")
            }
            _ => false,
        }
    }
}

#[derive(Deserialize)]
struct ApiResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
    error_code: Option<i32>,
    parameters: Option<ResponseParameters>,
}

#[derive(Deserialize)]
struct ResponseParameters {
    retry_after: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Update {
    pub update_id: i64,
    #[serde(default)]
    pub message: Option<Message>,
    /// Bot API 10.0 的游客模式：机器人被召唤后收到的是这条通道
    #[serde(default)]
    pub guest_message: Option<Message>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    pub message_id: i32,
    pub chat: Chat,
    #[serde(default)]
    pub from: Option<User>,
    /// inline 模式下被召唤的机器人
    #[serde(default)]
    pub via_bot: Option<User>,
    /// 游客模式消息里记录「是谁召唤的我」——溯源的关键
    #[serde(default)]
    pub guest_bot_caller_user: Option<User>,
    #[serde(default)]
    pub sender_chat: Option<Chat>,
    #[serde(default)]
    pub is_automatic_forward: bool,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub caption: Option<String>,
}

impl Message {
    pub fn is_group(&self) -> bool {
        matches!(self.chat.kind.as_str(), "group" | "supergroup")
    }

    /// 日志/证据里用的一句话摘要
    pub fn preview(&self) -> String {
        let Some(text) = self.text.as_deref().or(self.caption.as_deref()) else {
            return "<非文本消息>".to_string();
        };

        let flat = text.replace(['\n', '\r'], " ");
        let mut out: String = flat.chars().take(120).collect();
        if flat.chars().count() > 120 {
            out.push('…');
        }
        out
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Chat {
    pub id: i64,
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: u64,
    #[serde(default)]
    pub is_bot: bool,
    #[serde(default)]
    pub first_name: String,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
}

/// getMe 的返回：除了用户信息，还带了下面这些能力开关
#[derive(Debug, Clone, Deserialize)]
pub struct Me {
    #[serde(flatten)]
    pub user: User,
    #[serde(default)]
    pub can_join_groups: bool,
    /// 隐私模式是否关闭。true 表示能读到群里的全部消息
    #[serde(default)]
    pub can_read_all_group_messages: bool,
}

/// getChatMember 的返回，只关心身份和删除权限
#[derive(Debug, Clone, Deserialize)]
pub struct ChatMember {
    pub status: String,
    #[serde(default)]
    pub can_delete_messages: bool,
}

impl User {
    pub fn display(&self) -> String {
        match &self.username {
            Some(name) => format!("@{name}"),
            None => format!("{}#{}", self.full_name(), self.id),
        }
    }

    pub fn full_name(&self) -> String {
        match &self.last_name {
            Some(last) => format!("{} {}", self.first_name, last),
            None => self.first_name.clone(),
        }
    }

    /// 匿名管理员 / 频道身份 / Telegram 官方服务号
    pub fn is_special(&self) -> bool {
        matches!(self.id, 1_087_968_824 | 136_817_688 | 777_000)
    }
}

#[derive(Deserialize)]
struct SentMessage {
    message_id: i32,
}

impl Tg {
    pub fn new(token: impl Into<String>, debug_raw: bool) -> Self {
        let client = reqwest::Client::builder()
            // 长轮询会挂 30s，请求超时要留出余量
            .timeout(Duration::from_secs(90))
            .build()
            .expect("构建 HTTP 客户端失败");

        Self { client, token: token.into(), debug_raw }
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        body: &serde_json::Value,
    ) -> Result<T, CallError> {
        let url = format!("{API}{}/{method}", self.token);

        let response = self.client.post(&url).json(body).send().await.map_err(CallError::Http)?;
        let bytes = response.bytes().await.map_err(CallError::Http)?;

        if self.debug_raw {
            log::debug!("{method} 原始响应: {}", snippet(&bytes));
        }

        let parsed: ApiResponse<T> = serde_json::from_slice(&bytes)
            .map_err(|err| CallError::Parse(format!("{err}；原始响应: {}", snippet(&bytes))))?;

        if parsed.ok {
            if let Some(result) = parsed.result {
                return Ok(result);
            }
        }

        Err(CallError::Api {
            code: parsed.error_code,
            description: parsed.description.unwrap_or_else(|| "未知错误".to_string()),
            retry_after: parsed.parameters.and_then(|p| p.retry_after),
        })
    }

    pub async fn get_me(&self) -> Result<Me, CallError> {
        self.call("getMe", &serde_json::json!({})).await
    }

    /// 原始 getMe 结果，诊断时看全部字段
    pub async fn get_me_raw(&self) -> Result<serde_json::Value, CallError> {
        self.call("getMe", &serde_json::json!({})).await
    }

    /// 查某个用户在群里的身份和权限，启动自检用
    pub async fn get_chat_member(
        &self,
        chat_id: i64,
        user_id: u64,
    ) -> Result<ChatMember, CallError> {
        self.call(
            "getChatMember",
            &serde_json::json!({ "chat_id": chat_id, "user_id": user_id }),
        )
        .await
    }

    /// 原始 getUpdates 结果，不解析、不处理——诊断用，绝不删除任何东西
    pub async fn get_updates_raw(
        &self,
        offset: i64,
        timeout: u32,
    ) -> Result<serde_json::Value, CallError> {
        self.call(
            "getUpdates",
            &serde_json::json!({
                "offset": offset,
                "timeout": timeout,
                "allowed_updates": [],
            }),
        )
        .await
    }

    /// 长轮询取更新。
    ///
    /// 不传 `allowed_updates`——传空数组虽然官方说等价于默认，但不同实现理解不一，
    /// 万一被当成「不接收任何类型」就会永远返回空，很难查。不带这个字段最保险。
    pub async fn get_updates(&self, offset: i64, timeout: u32) -> Result<Vec<Update>, CallError> {
        self.call(
            "getUpdates",
            &serde_json::json!({
                "offset": offset,
                "timeout": timeout,
            }),
        )
        .await
    }

    /// 清掉可能存在的 webhook。
    ///
    /// 只要 bot 上还挂着 webhook，`getUpdates` 就会一直返回 409 Conflict，
    /// 表现为「程序在跑但一条消息都收不到」。
    pub async fn delete_webhook(&self) -> Result<(), CallError> {
        let _: serde_json::Value = self
            .call("deleteWebhook", &serde_json::json!({ "drop_pending_updates": false }))
            .await?;
        Ok(())
    }

    pub async fn delete_message(&self, chat_id: i64, message_id: i32) -> Result<(), CallError> {
        let _: serde_json::Value = self
            .call(
                "deleteMessage",
                &serde_json::json!({ "chat_id": chat_id, "message_id": message_id }),
            )
            .await?;
        Ok(())
    }

    pub async fn send_message(&self, chat_id: i64, text: String) -> Result<i32, CallError> {
        let sent: SentMessage = self
            .call(
                "sendMessage",
                &serde_json::json!({
                    "chat_id": chat_id,
                    "text": text,
                    "disable_web_page_preview": true,
                }),
            )
            .await?;
        Ok(sent.message_id)
    }
}

fn snippet(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_fields_do_not_break_parsing() {
        // 模拟 Bot API 以后又加了新字段：只要能解析出需要的部分就行
        let raw = r#"{
            "update_id": 1,
            "guest_message": {
                "message_id": 42,
                "date": 1770000000,
                "chat": {"id": -100123, "type": "supergroup", "title": "t"},
                "from": {"id": 900, "is_bot": true, "first_name": "Ad", "username": "ad_bot"},
                "guest_bot_caller_user": {"id": 7, "is_bot": false, "first_name": "Alice"},
                "guest_query_id": "abc",
                "some_future_field": {"nested": [1,2,3]}
            }
        }"#;

        let update: Update = serde_json::from_str(raw).expect("应当能解析");
        let msg = update.guest_message.expect("应当有 guest_message");

        assert_eq!(msg.message_id, 42);
        assert!(msg.is_group());
        let from = msg.from.as_ref().unwrap();
        assert!(from.is_bot);
        assert_eq!(msg.guest_bot_caller_user.as_ref().unwrap().id, 7);
    }

    #[test]
    fn parses_plain_inline_message() {
        let raw = r#"{
            "update_id": 2,
            "message": {
                "message_id": 43,
                "chat": {"id": -100123, "type": "supergroup"},
                "from": {"id": 8, "is_bot": false, "first_name": "Bob"},
                "via_bot": {"id": 901, "is_bot": true, "first_name": "Gif", "username": "gif"}
            }
        }"#;

        let update: Update = serde_json::from_str(raw).expect("应当能解析");
        let msg = update.message.expect("应当有 message");

        assert!(!msg.from.as_ref().unwrap().is_bot);
        assert_eq!(msg.via_bot.as_ref().unwrap().display(), "@gif");
    }

    #[test]
    fn special_accounts_are_recognised() {
        let anonymous = User {
            id: 1_087_968_824,
            is_bot: true,
            first_name: "Group".into(),
            last_name: None,
            username: Some("GroupAnonymousBot".into()),
        };
        assert!(anonymous.is_special());
    }
}
