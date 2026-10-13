//! 配信APIの回数制限（13-security.md「GraphQL」、Q94, Q100, Q120, Q153）。
//!
//! APIキーごとと、ワークスペースごとに、1分あたりの回数を数える。数え方のキーには必ずワークスペースIDを含めるので、
//! あるワークスペースが上限に達しても、他のワークスペースは制限されない（INV-TEN-03）。状態はプロセスのメモリに持つ。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use uuid::Uuid;

const WINDOW: Duration = Duration::from_secs(60);
/// メモリに持つ記録の数の上限の目安。超えたら、期限の切れた記録を捨てる。
const PRUNE_THRESHOLD: usize = 50_000;

/// 何を数えるか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Subject {
    Key { workspace: Uuid, key: Uuid },
    Workspace(Uuid),
}

#[derive(Default)]
pub struct Limiter {
    state: Mutex<HashMap<Subject, (Instant, u32)>>,
}

impl Limiter {
    /// 1回を数える。上限を超えていれば、次に試せるまでの秒数を返す（数えない）。
    pub fn check(&self, subject: Subject, per_minute: u32) -> Result<(), u64> {
        let now = Instant::now();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.len() >= PRUNE_THRESHOLD {
            state.retain(|_, (start, _)| now.duration_since(*start) < WINDOW);
        }
        let entry = state.entry(subject).or_insert((now, 0));
        if now.duration_since(entry.0) >= WINDOW {
            *entry = (now, 0);
        }
        if entry.1 >= per_minute {
            return Err(WINDOW
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
}
