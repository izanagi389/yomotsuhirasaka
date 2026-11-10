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
    use crate::locale::Locale;

    /// 通知の種類。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Event {
        PasswordChanged,
        MfaEnabled,
        MfaDisabled,
        PersonalTokenIssued,
        ApiKeyIssued,
        NewDeviceLogin,
        NewCountryLogin,
        PasskeyAdded,
        PasskeyRemoved,
        TotpRemoved,
        IdentityLinked,
        IdentityUnlinked,
        RecoveryCodesRegenerated,
    }

    /// (件名, 起きたこと)
    fn texts(event: Event, locale: Locale) -> (&'static str, &'static str) {
        match (event, locale) {
            (Event::RecoveryCodesRegenerated, Locale::En) => (
                "Your recovery codes were replaced",
                "New recovery codes were created for your account. The old codes no longer work.",
            ),
            (Event::RecoveryCodesRegenerated, Locale::Ja) => (
                "復旧コードが作り直されました",
                "あなたのアカウントの復旧コードが作り直されました。前のコードはもう使えません。",
            ),
            (Event::IdentityLinked, Locale::En) => (
                "An external account was linked",
                "An external sign-in account was linked to your account.",
            ),
            (Event::IdentityLinked, Locale::Ja) => (
                "外部のアカウントがひも付けられました",
                "あなたのアカウントに、ログインに使う外部のアカウントがひも付けられました。",
            ),
            (Event::IdentityUnlinked, Locale::En) => (
                "An external account was unlinked",
                "An external sign-in account was unlinked from your account.",
            ),
            (Event::IdentityUnlinked, Locale::Ja) => (
                "外部のアカウントのひも付けが解除されました",
                "あなたのアカウントから、ログインに使う外部のアカウントのひも付けが解除されました。",
            ),
            (Event::PasswordChanged, Locale::En) => (
                "Your password was changed",
                "The password for your account was changed.",
            ),
            (Event::PasswordChanged, Locale::Ja) => (
                "パスワードが変更されました",
                "アカウントのパスワードが変更されました。",
            ),
            (Event::MfaEnabled, Locale::En) => (
                "Multi-factor authentication was enabled",
                "Multi-factor authentication (authenticator app) was enabled for your account.",
            ),
            (Event::MfaEnabled, Locale::Ja) => (
                "多要素認証が設定されました",
                "アカウントに多要素認証（認証アプリ）が設定されました。",
            ),
            (Event::MfaDisabled, Locale::En) => (
                "Multi-factor authentication was disabled",
                "Multi-factor authentication was disabled for your account.",
            ),
            (Event::MfaDisabled, Locale::Ja) => (
                "多要素認証が解除されました",
                "アカウントの多要素認証が解除されました。",
            ),
            (Event::PersonalTokenIssued, Locale::En) => (
                "A personal access token was created",
                "A personal access token (for the CLI) was created for your account.",
            ),
            (Event::PersonalTokenIssued, Locale::Ja) => (
                "個人トークンが発行されました",
                "アカウントに、CLI用の個人トークンが発行されました。",
            ),
            (Event::ApiKeyIssued, Locale::En) => (
                "You created a delivery API key",
                "A delivery API key was created with your account.",
            ),
            (Event::ApiKeyIssued, Locale::Ja) => (
                "配信APIのキーを発行しました",
                "あなたのアカウントで、配信APIのキーが発行されました。",
            ),
            (Event::NewDeviceLogin, Locale::En) => (
                "New sign-in to your account",
                "Your account was signed in to from a new device or browser.",
            ),
            (Event::NewDeviceLogin, Locale::Ja) => (
                "新しい端末からログインがありました",
                "新しい端末またはブラウザから、アカウントにログインがありました。",
            ),
            (Event::PasskeyAdded, Locale::En) => (
                "A passkey was added",
                "A passkey was added to your account for multi-factor authentication.",
            ),
            (Event::PasskeyAdded, Locale::Ja) => (
                "パスキーが追加されました",
                "アカウントの多要素認証に、パスキーが追加されました。",
            ),
            (Event::PasskeyRemoved, Locale::En) => (
                "A passkey was removed",
                "A passkey was removed from your account. Multi-factor authentication is still enabled with your other methods.",
            ),
            (Event::PasskeyRemoved, Locale::Ja) => (
                "パスキーが削除されました",
                "アカウントからパスキーが削除されました。ほかの方法で、多要素認証は引き続き有効です。",
            ),
            (Event::TotpRemoved, Locale::En) => (
                "The authenticator app was removed",
                "The authenticator app was removed from your account. Multi-factor authentication is still enabled with your passkeys.",
            ),
            (Event::TotpRemoved, Locale::Ja) => (
                "認証アプリが解除されました",
                "アカウントから認証アプリが解除されました。パスキーで、多要素認証は引き続き有効です。",
            ),
            (Event::NewCountryLogin, Locale::En) => (
                "Sign-in to your account from a different country",
                "Your account was signed in to from a different country than the previous sign-in.",
            ),
            (Event::NewCountryLogin, Locale::Ja) => (
                "前回と違う国からログインがありました",
                "前回のログインと違う国から、アカウントにログインがありました。",
            ),
        }
    }

    fn advice(locale: Locale) -> &'static str {
        match locale {
            Locale::En => {
                "If you did not do this, sign in and change your password, and contact your administrator."
            }
            Locale::Ja => {
                "心当たりがない場合は、ログインしてパスワードを変更し、管理者に連絡してください。"
            }
        }
    }

    /// 通知のメール（1つの言語だけで書く。Q198）。`site` は公開URLのホスト名。
    pub fn mail(to: &str, site: &str, event: Event, locale: Locale) -> Mail {
        mail_with(to, site, event, locale, None)
    }

    /// ログインの通知（新しい端末・国の変化）。IPアドレス、ブラウザの種類（決まった名前の1つ）、国のコードを添える。
    pub fn login(
        to: &str,
        site: &str,
        locale: Locale,
        event: Event,
        ip: std::net::IpAddr,
        browser: &str,
        country: Option<&str>,
    ) -> Mail {
        let (ip_label, browser_label, country_label) = match locale {
            Locale::En => ("IP address", "Browser", "Country"),
            Locale::Ja => ("IPアドレス", "ブラウザ", "国"),
        };
        let mut details = format!("{ip_label}: {ip}\n{browser_label}: {browser}");
        if let Some(c) = country {
            details.push_str(&format!("\n{country_label}: {c}"));
        }
        mail_with(to, site, event, locale, Some(&details))
    }

    fn mail_with(
        to: &str,
        site: &str,
        event: Event,
        locale: Locale,
        details: Option<&str>,
    ) -> Mail {
        let (subject, what) = texts(event, locale);
        let advice = advice(locale);
        let details = details.map(|d| format!("{d}\n\n")).unwrap_or_default();
        Mail {
            to: to.to_owned(),
            subject: format!("[{site}] {subject}"),
            body: format!("{what}\n\n{details}{advice}\n\n{site}\n"),
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
    locale: crate::locale::Locale,
) -> Mail {
    use crate::locale::Locale;
    let (subject, body) = match locale {
        Locale::En => (
            format!("[{site}] Invitation to {workspace_name}"),
            format!(
                "You have been invited to the workspace \"{workspace_name}\". Open the link below to accept.\n\n\
                 {link}\n\n\
                 This link can be used once and expires at {expires_at} (UTC).\n\n\
                 If you were not expecting this, you can ignore this email.\n"
            ),
        ),
        Locale::Ja => (
            format!("[{site}] {workspace_name} への招待"),
            format!(
                "ワークスペース「{workspace_name}」に招待されました。次のリンクを開いて承諾してください。\n\n\
                 {link}\n\n\
                 このリンクは一度だけ使え、{expires_at}（UTC）に期限が切れます。\n\n\
                 心当たりがない場合は、このメールを無視してください。\n"
            ),
        ),
    };
    Mail {
        to: to.to_owned(),
        subject,
        body,
    }
}

/// ワークスペースの管理者がリセットを求めたが、範囲の外（ほかのワークスペースにも所属する、サーバー管理者）なので
/// 発行しなかったことを、本人に知らせるメール。管理者には、発行したときと同じ応答を返す（所属を推測させない）。
pub fn reset_declined(to: &str, site: &str, locale: crate::locale::Locale) -> Mail {
    use crate::locale::Locale;
    let (subject, body) = match locale {
        Locale::En => (
            "A reset was requested for your account",
            "A workspace administrator asked to reset your password or multi-factor authentication. \
             No reset was issued, because a workspace administrator cannot reset an account that also belongs \
             to other workspaces.\n\n\
             If you need it, ask a server administrator, or use \"Forgot your password?\" on the sign-in page.\n\n\
             If you did not expect this, you can ignore this email. Your account has not been changed.",
        ),
        Locale::Ja => (
            "アカウントのリセットが求められました",
            "ワークスペースの管理者が、あなたのアカウントのパスワードか多要素認証のリセットを求めました。\
             あなたはほかのワークスペースにも所属しているため、ワークスペースの管理者はリセットを発行できず、発行していません。\n\n\
             リセットが必要なら、サーバー管理者に依頼するか、ログインの画面の「パスワードを忘れた場合」を使ってください。\n\n\
             心当たりがない場合は、このメールを無視してください。アカウントは変わっていません。",
        ),
    };
    Mail {
        to: to.to_owned(),
        subject: format!("[{site}] {subject}"),
        body: format!("{body}\n\n{site}\n"),
    }
}

/// リセットのメール。リンクにはトークンが入るので、ジョブの中身は暗号化して保存される。
pub fn reset(
    to: &str,
    site: &str,
    kind: crate::resets::Kind,
    link: &str,
    expires_at: &str,
    locale: crate::locale::Locale,
) -> Mail {
    use crate::locale::Locale;
    use crate::resets::Kind;
    let (subject, what) = match (kind, locale) {
        (Kind::Password, Locale::En) => (
            "Reset your password",
            "A password reset was requested for your account. Open the link below to choose a new password.",
        ),
        (Kind::Password, Locale::Ja) => (
            "パスワードの再設定",
            "アカウントのパスワードの再設定が求められました。次のリンクを開いて、新しいパスワードを決めてください。",
        ),
        (Kind::Mfa, Locale::En) => (
            "Reset multi-factor authentication",
            "An administrator issued a reset of multi-factor authentication for your account. Open the link below to remove it, then sign in and set it up again.",
        ),
        (Kind::Mfa, Locale::Ja) => (
            "多要素認証のリセット",
            "管理者が、アカウントの多要素認証のリセットを発行しました。次のリンクを開いて解除し、ログインしてから設定し直してください。",
        ),
    };
    let rest = match locale {
        Locale::En => format!(
            "This link can be used once and expires at {expires_at} (UTC).\n\n\
             If you did not expect this, you can ignore this email. Your account has not been changed."
        ),
        Locale::Ja => format!(
            "このリンクは一度だけ使え、{expires_at}（UTC）に期限が切れます。\n\n\
             心当たりがない場合は、このメールを無視してください。アカウントは変わっていません。"
        ),
    };
    Mail {
        to: to.to_owned(),
        subject: format!("[{site}] {subject}"),
        body: format!("{what}\n\n{link}\n\n{rest}\n"),
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
    fn notifications_are_written_in_one_language() {
        use crate::locale::Locale;
        let ja = notify::mail(
            "a@example.com",
            "yomi.example.com",
            notify::Event::PasswordChanged,
            Locale::Ja,
        );
        assert_eq!(ja.to, "a@example.com");
        assert!(ja.subject.starts_with("[yomi.example.com]"));
        assert!(ja.body.contains("パスワード") && !ja.body.contains("password"));
        let en = notify::mail(
            "a@example.com",
            "yomi.example.com",
            notify::Event::PasswordChanged,
            Locale::En,
        );
        assert!(en.body.contains("password") && !en.body.contains("パスワード"));
    }
}
