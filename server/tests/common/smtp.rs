//! 結合テストの部品: テストの中で動かす擬似SMTPサーバー。

use std::sync::{Arc, Mutex};

use cms_server::config::{SmtpConfig, SmtpSecurity};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

/// 擬似SMTPサーバーのふるまい。
#[derive(Default)]
pub struct Smtp {
    /// 受け取ったメール（DATAの中身）。
    pub received: Vec<String>,
    /// この回数だけ、宛先の段階で一時的な失敗（451）を返す。
    pub transient_failures: u32,
    /// 宛先の段階で恒久的な失敗（550）を返す。
    pub reject: bool,
}

pub async fn fake_smtp() -> (u16, Arc<Mutex<Smtp>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = Arc::new(Mutex::new(Smtp::default()));
    let shared = state.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let state = shared.clone();
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                write.write_all(b"220 fake ESMTP\r\n").await.unwrap();
                let mut data: Option<String> = None;
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Some(buf) = data.as_mut() {
                        if line == "." {
                            state.lock().unwrap().received.push(data.take().unwrap());
                            write.write_all(b"250 queued\r\n").await.unwrap();
                        } else {
                            buf.push_str(&line);
                            buf.push('\n');
                        }
                        continue;
                    }
                    let upper = line.to_ascii_uppercase();
                    let reply: &[u8] = if upper.starts_with("EHLO") || upper.starts_with("HELO") {
                        b"250 fake\r\n"
                    } else if upper.starts_with("RCPT") {
                        let mut s = state.lock().unwrap();
                        if s.reject {
                            b"550 5.1.1 no such user\r\n"
                        } else if s.transient_failures > 0 {
                            s.transient_failures -= 1;
                            b"451 4.3.0 try again later\r\n"
                        } else {
                            b"250 ok\r\n"
                        }
                    } else if upper.starts_with("DATA") {
                        data = Some(String::new());
                        b"354 go ahead\r\n"
                    } else if upper.starts_with("QUIT") {
                        write.write_all(b"221 bye\r\n").await.unwrap();
                        break;
                    } else {
                        b"250 ok\r\n"
                    };
                    write.write_all(reply).await.unwrap();
                }
            });
        }
    });
    (port, state)
}

pub fn smtp_config(port: u16) -> SmtpConfig {
    SmtpConfig {
        host: "127.0.0.1".into(),
        port: Some(port),
        security: SmtpSecurity::None,
        username: None,
        password: None,
        from: "CMS <cms@example.test>".into(),
    }
}

/// 受け取ったメールの本文を読める形にする（lettre は非ASCIIの本文を base64 か quoted-printable で送る）。
pub fn decoded_body(raw: &str) -> String {
    use base64::Engine as _;
    let (head, body) = raw.split_once("\n\n").unwrap_or((raw, ""));
    let head = head.to_ascii_lowercase();
    if head.contains("content-transfer-encoding: base64") {
        let joined: String = body.split_whitespace().collect();
        String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(joined)
                .expect("base64 body"),
        )
        .expect("utf-8 body")
    } else if head.contains("content-transfer-encoding: quoted-printable") {
        decode_quoted_printable(body)
    } else {
        body.to_owned()
    }
}

/// quoted-printable を戻す（行末の `=` は改行なしでつなぐ、`=XX` は1バイト）。
fn decode_quoted_printable(body: &str) -> String {
    let joined = body.replace("=\n", "");
    let bytes = joined.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        if let (b'=', Some(b)) = (bytes[i], hex) {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).expect("utf-8 body")
}
