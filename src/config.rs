//! 运行配置，全部来自环境变量（支持 .env）。

use std::collections::HashSet;
use std::env;
use std::fmt;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::api::User;

#[derive(Debug, Clone)]
pub struct Config {
    pub token: String,
    /// 删除后是否在群里回提示
    pub notice: bool,
    /// 提示发出多久后撤回
    pub notice_ttl: Duration,
    /// Some(..) 时只处理这些群
    pub allowed_chats: Option<HashSet<i64>>,
    /// 白名单机器人
    pub allowed_bots: AllowList,
    /// 打印收到的原始 update，用于确认到底有没有收到消息
    pub debug_raw: bool,
}

/// 白名单机器人。支持数字 ID 和 `@username`（不区分大小写）混写。
#[derive(Debug, Clone, Default)]
pub struct AllowList {
    ids: HashSet<u64>,
    usernames: HashSet<String>,
}

impl AllowList {
    pub fn contains(&self, user: &User) -> bool {
        if self.ids.contains(&user.id) {
            return true;
        }
        user.username
            .as_deref()
            .is_some_and(|name| self.usernames.contains(&name.to_ascii_lowercase()))
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty() && self.usernames.is_empty()
    }
}

impl fmt::Display for AllowList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = self.usernames.iter().map(|name| format!("@{name}")).collect();
        parts.extend(self.ids.iter().map(u64::to_string));
        parts.sort();
        write!(f, "{}", parts.join(", "))
    }
}

pub fn parse_allow_list(raw: &str) -> AllowList {
    let mut list = AllowList::default();

    for token in raw.split(|c: char| c == ',' || c.is_whitespace()) {
        let token = token.trim().trim_start_matches('@');
        if token.is_empty() {
            continue;
        }

        match token.parse::<u64>() {
            Ok(id) => {
                list.ids.insert(id);
            }
            Err(_) => {
                list.usernames.insert(token.to_ascii_lowercase());
            }
        }
    }

    list
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let token = env::var("BOT_TOKEN")
            .or_else(|_| env::var("TELOXIDE_TOKEN"))
            .context("缺少环境变量 BOT_TOKEN")?;

        if token.trim().is_empty() {
            anyhow::bail!("BOT_TOKEN 是空的");
        }

        Ok(Self {
            token: token.trim().to_string(),
            notice: env_bool("NOTICE", true),
            notice_ttl: Duration::from_secs(env_u64("NOTICE_TTL_SECS", 30)),
            allowed_chats: env_i64_list("ALLOWED_CHATS"),
            allowed_bots: parse_allow_list(&env::var("ALLOWED_BOTS").unwrap_or_default()),
            debug_raw: env_bool("DEBUG_RAW", false),
        })
    }
}

fn env_bool(key: &str, default: bool) -> bool {
    match env::var(key) {
        Ok(raw) => matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on" | "y"
        ),
        Err(_) => default,
    }
}

fn env_u64(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

fn env_i64_list(key: &str) -> Option<HashSet<i64>> {
    let raw = env::var(key).ok()?;
    let ids: HashSet<i64> = raw
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.trim().parse::<i64>().ok())
        .collect();
    if ids.is_empty() {
        None
    } else {
        Some(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::parse_allow_list;

    #[test]
    fn parses_ids_and_usernames() {
        let list = parse_allow_list("123456, @Gif, group_helper_bot");

        assert!(list.ids.contains(&123456));
        assert!(list.usernames.contains("gif"));
        assert!(list.usernames.contains("group_helper_bot"));
    }

    #[test]
    fn separators_and_prefixes_are_forgiving() {
        let list = parse_allow_list("  1 ,2\n@Foo\t@bar , ");

        assert_eq!(list.ids.len(), 2);
        assert!(list.usernames.contains("foo"));
        assert!(list.usernames.contains("bar"));
    }

    #[test]
    fn empty_whitelist_stays_empty() {
        assert!(parse_allow_list("  ,  ").is_empty());
        assert!(parse_allow_list("").is_empty());
    }
}
