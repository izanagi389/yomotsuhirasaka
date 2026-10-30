//! 配信APIの回数制限（13-security.md「GraphQL」、Q94, Q100, Q120, Q153）と、回答APIの回数制限（「チャットボットへの悪用」、Q116）。
//!
//! APIキーごとと、ワークスペースごとに、1分あたりの回数を数える。数え方のキーには必ずワークスペースIDを含めるので、
//! あるワークスペースが上限に達しても、他のワークスペースは制限されない（INV-TEN-03）。状態はプロセスのメモリに持つ。
//! 回答は、さらにAPIキーごとの回答の回数（1分あたり）と、訪問者の識別子ごとの回数（1分あたりと1日あたり）を数える
//! （INV-QA-14）。訪問者の識別子は、ワークスペースIDと合わせたハッシュだけを持つ（識別子そのものは持たない）。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::config::AnswerConfig;

const MINUTE: Duration = Duration::from_secs(60);
const DAY: Duration = Duration::from_secs(24 * 60 * 60);
/// メモリに持つ記録の数の上限の目安。超えたら、期限の切れた記録を捨てる。
const PRUNE_THRESHOLD: usize = 50_000;

/// 何を数えるか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Subject {
    Key {
        workspace: Uuid,
        key: Uuid,
    },
    Workspace(Uuid),
    /// APIキーごとの回答の回数（1分あたり）。
    AnswerKey {
        workspace: Uuid,
        key: Uuid,
    },
    /// 訪問者ごとの回答の回数（`day` なら1日あたり、でなければ1分あたり）。`visitor` は識別子のハッシュ。
    Visitor {
        workspace: Uuid,
        visitor: [u8; 32],
        day: bool,
    },
}

impl Subject {
    fn window(&self) -> Duration {
        match self {
            Self::Visitor { day: true, .. } => DAY,
            _ => MINUTE,
        }
    }
}

/// 回答で、どの上限に達したか（次に試せるまでの秒数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerLimited {
    Key(u64),
    Visitor(u64),
}

#[derive(Default)]
pub struct Limiter {
    state: Mutex<HashMap<Subject, (Instant, u32)>>,
}

impl Limiter {
    /// 1回を数える（区切りは数えるものの期間。1分か1日）。上限を超えていれば、次に試せるまでの秒数を返す（数えない）。
    pub fn check(&self, subject: Subject, limit: u32) -> Result<(), u64> {
        let now = Instant::now();
        let window = subject.window();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.len() >= PRUNE_THRESHOLD {
            state.retain(|s, (start, _)| now.duration_since(*start) < s.window());
        }
        let entry = state.entry(subject).or_insert((now, 0));
        if now.duration_since(entry.0) >= window {
            *entry = (now, 0);
        }
        if entry.1 >= limit {
            return Err(window
                .saturating_sub(now.duration_since(entry.0))
                .as_secs()
                .max(1));
        }
        entry.1 += 1;
        Ok(())
    }

    /// キーとワークスペースの両方を数える。キーで止まったときは、ワークスペースの分を数えない。
    pub fn check_request(
        &self,
        workspace: Uuid,
        key: Uuid,
        key_limit: u32,
        workspace_limit: u32,
    ) -> Result<(), u64> {
        self.check(Subject::Key { workspace, key }, key_limit)?;
        self.check(Subject::Workspace(workspace), workspace_limit)
    }

    /// 回答の回数を数える（キー → 訪問者の1分 → 訪問者の1日の順。止まったところから先は数えない）。
    /// `visitor` は中継サーバーが渡した訪問者の識別子（なければ、キーの分だけを数える）。
    pub fn check_answer(
        &self,
        workspace: Uuid,
        key: Uuid,
        visitor: Option<&str>,
        limits: &AnswerConfig,
    ) -> Result<(), AnswerLimited> {
        self.check(
            Subject::AnswerKey { workspace, key },
            limits.key_requests_per_minute,
        )
        .map_err(AnswerLimited::Key)?;
        let Some(visitor) = visitor else {
            return Ok(());
        };
        let hash: [u8; 32] = Sha256::new()
            .chain_update(workspace.as_bytes())
            .chain_update(visitor.as_bytes())
            .finalize()
            .into();
        for (day, limit) in [
            (false, limits.visitor_requests_per_minute),
            (true, limits.visitor_requests_per_day),
        ] {
            self.check(
                Subject::Visitor {
                    workspace,
                    visitor: hash,
                    day,
                },
                limit,
            )
            .map_err(AnswerLimited::Visitor)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_workspaces_are_counted_separately() {
        let l = Limiter::default();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let (ka, kb) = (Uuid::new_v4(), Uuid::new_v4());
        for _ in 0..3 {
            assert!(l.check_request(a, ka, 3, 100).is_ok());
        }
        assert!(l.check_request(a, ka, 3, 100).is_err());
        // 他のワークスペースのキーは止まらない
        assert!(l.check_request(b, kb, 3, 100).is_ok());
        // ワークスペースの上限は、キーをまたいで数える
        let k2 = Uuid::new_v4();
        let l = Limiter::default();
        assert!(l.check_request(a, ka, 10, 2).is_ok());
        assert!(l.check_request(a, k2, 10, 2).is_ok());
        assert!(l.check_request(a, k2, 10, 2).is_err());
        assert!(l.check_request(b, kb, 10, 2).is_ok());
    }

    #[test]
    fn answers_are_counted_per_key_and_per_visitor() {
        let limits = AnswerConfig {
            key_requests_per_minute: 5,
            visitor_requests_per_minute: 2,
            visitor_requests_per_day: 3,
        };
        let l = Limiter::default();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let (ka, kb) = (Uuid::new_v4(), Uuid::new_v4());
        // 訪問者の1分の上限
        assert!(l.check_answer(a, ka, Some("v1"), &limits).is_ok());
        assert!(l.check_answer(a, ka, Some("v1"), &limits).is_ok());
        assert!(
            matches!(l.check_answer(a, ka, Some("v1"), &limits), Err(AnswerLimited::Visitor(s)) if s >= 1)
        );
        // 別の訪問者・識別子なし・別のワークスペースの同じ識別子は止まらない
        assert!(l.check_answer(a, ka, Some("v2"), &limits).is_ok());
        assert!(l.check_answer(a, ka, None, &limits).is_ok());
        assert!(l.check_answer(b, kb, Some("v1"), &limits).is_ok());
        // キーの上限（訪問者が違っても、キーで数える）。ここまでで ka は5回
        assert!(matches!(
            l.check_answer(a, ka, Some("v3"), &limits),
            Err(AnswerLimited::Key(_))
        ));
    }

    #[test]
    fn visitors_have_a_daily_limit() {
        let limits = AnswerConfig {
            key_requests_per_minute: 100,
            visitor_requests_per_minute: 100,
            visitor_requests_per_day: 2,
        };
        let l = Limiter::default();
        let (ws, key) = (Uuid::new_v4(), Uuid::new_v4());
        assert!(l.check_answer(ws, key, Some("v"), &limits).is_ok());
        assert!(l.check_answer(ws, key, Some("v"), &limits).is_ok());
        // 1日の上限: 次に試せるまでは、1分より長い
        match l.check_answer(ws, key, Some("v"), &limits) {
            Err(AnswerLimited::Visitor(s)) => assert!(s > 60, "{s}"),
            other => panic!("{other:?}"),
        }
    }
}
