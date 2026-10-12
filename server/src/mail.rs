//! メール（SMTP）。送信は必ずジョブで行い、リクエストの処理の中では送らない（設計の原則5）。
//!
//! - 宛先・件名・本文は、ジョブの中身として暗号化して保存する（リセットや招待のリンクを含みうるため）。
//! - SMTPの設定がなければ、メールは積まない。
//! - 送信の失敗は種類だけを記録する。宛先や相手の応答の本文はログに出さない。

use std::time::Duration;

use anyhow::{Result, bail};
use lettre::message::header::ContentType;
use lettre::message::{Mailbox, Message};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::extension::ClientId;
use lettre::{AsyncSmtpTransport, AsyncTransport as _, Tokio1Executor};
use sqlx::PgExecutor;

use crate::config::{SmtpConfig, SmtpSecurity};
use crate::jobs::{self, Outcome};
use crate::keys::KeyRing;

/// ジョブの種類の名前。
pub const JOB_KIND: &str = "mail";
/// 送信の試行の上限（間隔は30秒から2倍ずつ。8回でおよそ1時間半）。
const MAX_ATTEMPTS: i32 = 8;
const SEND_TIMEOUT: Duration = Duration::from_secs(30);

/// 1通のメール（本文はテキストだけ）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mail {
    pub to: String,
    pub subject: String,
    pub body: String,
}

impl Mail {
    /// ジョブの中身の形: 各項目を「4バイトの長さ（ビッグエンディアン）＋UTF-8」で並べる。
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for field in [&self.to, &self.subject, &self.body] {
            let len = u32::try_from(field.len()).unwrap_or(u32::MAX);
            out.extend_from_slice(&len.to_be_bytes());
            out.extend_from_slice(&field.as_bytes()[..len as usize]);
        }
        out
    }

    pub fn decode(mut bytes: &[u8]) -> Result<Self> {
        let mut fields = Vec::with_capacity(3);
        for _ in 0..3 {
            if bytes.len() < 4 {
                bail!("truncated mail payload");
            }
            let len = u32::from_be_bytes(bytes[..4].try_into().expect("4 bytes")) as usize;
            bytes = &bytes[4..];
            if bytes.len() < len {
                bail!("truncated mail payload");
            }
            fields.push(String::from_utf8(bytes[..len].to_vec())?);
            bytes = &bytes[len..];
        }
        if !bytes.is_empty() {
            bail!("trailing bytes in mail payload");
        }
        let body = fields.pop().expect("3 fields");
        let subject = fields.pop().expect("3 fields");
        let to = fields.pop().expect("3 fields");
        Ok(Self { to, subject, body })
    }
}

/// メールをジョブとして積む。SMTPの設定がなければ何もせず false。
pub async fn enqueue<'e>(
    executor: impl PgExecutor<'e>,
    keys: &KeyRing,
    smtp: Option<&SmtpConfig>,
    mail: &Mail,
) -> Result<bool> {
    if smtp.is_none() {
        return Ok(false);
    }
    jobs::enqueue(executor, keys, None, JOB_KIND, &mail.encode(), MAX_ATTEMPTS).await?;
    Ok(true)
}

/// メールのジョブを1つ実行する。
pub async fn deliver(smtp: Option<&SmtpConfig>, hello_name: &str, payload: &[u8]) -> Outcome {
    let Some(smtp) = smtp else {
        // 積んだ後でSMTPの設定が外された
        return Outcome::Fail("smtp_not_configured");
    };
    let Ok(mail) = Mail::decode(payload) else {
        return Outcome::Fail("invalid_payload");
    };
    let message = match build_message(smtp, &mail) {
        Ok(m) => m,
        Err(_) => return Outcome::Fail("invalid_message"),
    };
    let transport = match transport(smtp, hello_name) {
        Ok(t) => t,
        Err(_) => return Outcome::Fail("invalid_smtp_settings"),
    };
    match transport.send(message).await {
        Ok(_) => Outcome::Done,
        Err(e) if e.is_permanent() => Outcome::Fail("smtp_permanent_error"),
        Err(e) if e.is_timeout() => Outcome::Retry("smtp_timeout"),
        Err(e) if e.is_tls() => Outcome::Retry("smtp_tls_error"),
        Err(e) if e.is_transient() => Outcome::Retry("smtp_transient_error"),
        Err(_) => Outcome::Retry("smtp_connection_error"),
    }
}

fn build_message(smtp: &SmtpConfig, mail: &Mail) -> Result<Message> {
    let from: Mailbox = smtp.from.parse()?;
    let to: Mailbox = mail.to.parse()?;
    Ok(Message::builder()
        .from(from)
        .to(to)
        .subject(mail.subject.clone())
        .header(ContentType::TEXT_PLAIN)
        .body(mail.body.clone())?)
}

fn transport(smtp: &SmtpConfig, hello_name: &str) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
    let mut builder = match smtp.security {
        SmtpSecurity::Starttls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&smtp.host)?,
        SmtpSecurity::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&smtp.host)?,
        SmtpSecurity::None => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&smtp.host),
    };
    if let Some(port) = smtp.port {
        builder = builder.port(port);
    }
    if let (Some(user), Some(password)) = (&smtp.username, &smtp.password) {
        builder = builder.credentials(Credentials::new(user.clone(), password.clone()));
    }
    Ok(builder
        .hello_name(ClientId::Domain(hello_name.to_owned()))
        .timeout(Some(SEND_TIMEOUT))
        .build())
}

/// 本人への通知（13-security.md「本人への通知」、INV-AUD-06）。秘密の値は本文に含めない。
pub mod notify {
    use super::Mail;

    /// 通知の種類。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Event {
        PasswordChanged,
        MfaEnabled,
        MfaDisabled,
    }

    impl Event {
        fn texts(self) -> (&'static str, &'static str, &'static str) {
            match self {
                Event::PasswordChanged => (
                    "Your password was changed",
                    "パスワードが変更されました",
                    "The password for your CMS account was changed.\n\
                     アカウントのパスワードが変更されました。",
                ),
                Event::MfaEnabled => (
                    "Multi-factor authentication was enabled",
                    "多要素認証が設定されました",
                    "Multi-factor authentication (authenticator app) was enabled for your CMS account.\n\
                     アカウントに多要素認証（認証アプリ）が設定されました。",
                ),
                Event::MfaDisabled => (
                    "Multi-factor authentication was disabled",
                    "多要素認証が解除されました",
                    "Multi-factor authentication was disabled for your CMS account.\n\
                     アカウントの多要素認証が解除されました。",
                ),
            }
        }
    }

    /// 通知のメール。`site` は公開URLのホスト名。
    pub fn mail(to: &str, site: &str, event: Event) -> Mail {
        let (subject_en, subject_ja, what) = event.texts();
        Mail {
            to: to.to_owned(),
            subject: format!("[{site}] {subject_en} / {subject_ja}"),
            body: format!(
                "{what}\n\n\
                 If you did not do this, sign in and change your password, and contact your administrator.\n\
                 心当たりがない場合は、ログインしてパスワードを変更し、管理者に連絡してください。\n\n\
                 {site}\n"
            ),
        }
    }
}

/// 招待のメール。リンクにはトークンが入るので、ジョブの中身は暗号化して保存される。
pub fn invitation(
    to: &str,
    site: &str,
    workspace_name: &str,
    link: &str,
    expires_at: &str,
) -> Mail {
    Mail {
        to: to.to_owned(),
        subject: format!("[{site}] Invitation to {workspace_name} / {workspace_name} への招待"),
        body: format!(
            "You have been invited to the workspace \"{workspace_name}\". Open the link below to accept.\n\
             ワークスペース「{workspace_name}」に招待されました。次のリンクを開いて承諾してください。\n\n\
             {link}\n\n\
             This link can be used once and expires at {expires_at} (UTC).\n\
             このリンクは一度だけ使え、{expires_at}（UTC）に期限が切れます。\n\n\
             If you were not expecting this, you can ignore this email.\n\
             心当たりがない場合は、このメールを無視してください。\n"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_round_trip() {
        let mail = Mail {
            to: "a@example.com".into(),
            subject: "件名".into(),
            body: "本文\nline 2".into(),
        };
        assert_eq!(Mail::decode(&mail.encode()).unwrap(), mail);
        let encoded = mail.encode();
        assert!(Mail::decode(&encoded[..encoded.len() - 1]).is_err());
        let mut extra = encoded.clone();
        extra.push(0);
        assert!(Mail::decode(&extra).is_err());
        assert!(Mail::decode(&[]).is_err());
    }

    #[test]
    fn notifications_mention_the_site_and_no_secrets() {
        let m = notify::mail(
            "a@example.com",
            "cms.example.com",
            notify::Event::PasswordChanged,
        );
        assert_eq!(m.to, "a@example.com");
        assert!(m.subject.starts_with("[cms.example.com]"));
        assert!(m.body.contains("パスワード"));
    }
}
