//! メディアのファイルを少しずつ送る（ファイル全体をメモリに読まない。13-security.md「入口の上限」）。
//!
//! ファイルを読む処理は応答とは別のタスクで動かす。相手が受け取らなくなると応答の本文は読み進められず、本文の中で
//! 時間を見ることができないため。タスクは、相手が受け取らないまま時間が過ぎるか、全体の時間の上限を超えると終わり、
//! ファイルと同時配信の枠を手放す。途中で終わると本文が `Content-Length` に足りないので、接続は切れる。

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::{Body, Bytes};
use hyper::body::{Frame, SizeHint};
use tokio::io::AsyncReadExt as _;
use tokio::sync::{OwnedSemaphorePermit, mpsc};

/// 一度に読む大きさ。
const CHUNK_BYTES: usize = 64 * 1024;

/// 送るときの時間の上限。
#[derive(Debug, Clone, Copy)]
pub struct SendLimits {
    /// 相手が次の分を受け取るまで待つ時間。
    pub stall: Duration,
    /// 1つのファイルを送り終えるまでの時間。
    pub total: Duration,
}

impl SendLimits {
    /// 既定の値。1分割で30秒待つので、遅くても約17kbpsの相手までは送れる。全体の10分は、既定の上限の20MBなら約270kbps。
    pub const DEFAULT: Self = Self {
        stall: Duration::from_secs(30),
        total: Duration::from_secs(600),
    };
}

/// 大きさの決まった本文。
struct FileBody {
    rx: mpsc::Receiver<Bytes>,
    remaining: u64,
}

impl hyper::body::Body for FileBody {
    type Data = Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        match self.rx.poll_recv(cx) {
            Poll::Ready(Some(chunk)) => {
                self.remaining = self.remaining.saturating_sub(chunk.len() as u64);
                Poll::Ready(Some(Ok(Frame::data(chunk))))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.remaining)
    }
}

/// 開いたファイルを送る本文を作る。`permit` は、送り終えるか諦めるまで持ち続ける。
pub fn file_body(
    mut file: tokio::fs::File,
    len: u64,
    permit: OwnedSemaphorePermit,
    limits: SendLimits,
) -> Body {
    let (tx, rx) = mpsc::channel::<Bytes>(1);
    tokio::spawn(async move {
        let _permit = permit;
        let send_all = async {
            let mut left = len;
            while left > 0 {
                let mut buf =
                    vec![0; CHUNK_BYTES.min(usize::try_from(left).unwrap_or(CHUNK_BYTES))];
                match file.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        buf.truncate(n);
                        left -= n as u64;
                    }
                }
                match tokio::time::timeout(limits.stall, tx.send(Bytes::from(buf))).await {
                    Ok(Ok(())) => {}
                    // 相手が切断した、または受け取らない
                    Ok(Err(_)) | Err(_) => break,
                }
            }
        };
        let _ = tokio::time::timeout(limits.total, send_all).await;
    });
    Body::new(FileBody { rx, remaining: len })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use http_body_util::BodyExt as _;
    use tokio::sync::Semaphore;

    use super::*;

    async fn file_of(len: usize) -> (std::path::PathBuf, tokio::fs::File) {
        let path = std::env::temp_dir().join(format!("yomi-serve-{}", uuid::Uuid::new_v4()));
        tokio::fs::write(&path, vec![7u8; len]).await.unwrap();
        let file = tokio::fs::File::open(&path).await.unwrap();
        (path, file)
    }

    #[tokio::test]
    async fn sends_the_whole_file_and_gives_the_slot_back() {
        let len = CHUNK_BYTES * 3 + 5;
        let (path, file) = file_of(len).await;
        let slots = Arc::new(Semaphore::new(1));
        let permit = slots.clone().try_acquire_owned().unwrap();
        let body = file_body(file, len as u64, permit, SendLimits::DEFAULT);
        assert_eq!(
            hyper::body::Body::size_hint(&body).exact(),
            Some(len as u64)
        );
        let got = body.collect().await.unwrap().to_bytes();
        assert_eq!(got.len(), len);
        assert!(got.iter().all(|b| *b == 7));
        tokio::task::yield_now().await;
        let _ = slots.acquire().await.unwrap();
        tokio::fs::remove_file(path).await.unwrap();
    }

    #[tokio::test]
    async fn a_reader_that_stops_reading_loses_the_slot() {
        let len = CHUNK_BYTES * 8;
        let (path, file) = file_of(len).await;
        let slots = Arc::new(Semaphore::new(1));
        let permit = slots.clone().try_acquire_owned().unwrap();
        let limits = SendLimits {
            stall: Duration::from_millis(50),
            total: Duration::from_secs(60),
        };
        // 応答の本文は持ったまま読まない
        let body = file_body(file, len as u64, permit, limits);
        assert_eq!(slots.available_permits(), 0);
        let back = tokio::time::timeout(Duration::from_secs(5), slots.acquire()).await;
        assert!(
            back.is_ok(),
            "the slot must come back while the body is unread"
        );
        drop(body);
        tokio::fs::remove_file(path).await.unwrap();
    }

    #[tokio::test]
    async fn a_slow_reader_stops_at_the_total_limit() {
        let len = CHUNK_BYTES * 8;
        let (path, file) = file_of(len).await;
        let slots = Arc::new(Semaphore::new(1));
        let permit = slots.clone().try_acquire_owned().unwrap();
        let limits = SendLimits {
            stall: Duration::from_secs(60),
            total: Duration::from_millis(50),
        };
        let body = file_body(file, len as u64, permit, limits);
        let back = tokio::time::timeout(Duration::from_secs(5), slots.acquire()).await;
        assert!(back.is_ok(), "the slot must come back at the total limit");
        drop(body);
        tokio::fs::remove_file(path).await.unwrap();
    }
}
