//! 接続元のIPアドレスから国を調べる（Q175）。
//!
//! - サーバー管理者が MaxMind DB 形式のデータ（GeoLite2-Country など）を用意し、
//!   設定 `[login_history] geoip_database` に置き場所を書いた場合だけ使う。データは同梱しない。
//! - データは起動時に一度だけ読み込み、形を検査する。更新したデータを使うには再起動する。
//! - 外部とは通信しない（ファイルを読むだけ）。

use std::net::IpAddr;
use std::path::Path;

use anyhow::{Context, Result};

/// 読み込んだ国のデータ。
pub struct GeoIp {
    reader: maxminddb::Reader<Vec<u8>>,
}

impl std::fmt::Debug for GeoIp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeoIp")
            .field("database_type", &self.reader.metadata().database_type)
            .finish()
    }
}

impl GeoIp {
    /// データを読み込み、壊れていないことを確かめる。
    pub fn open(path: &Path) -> Result<Self> {
        let reader = maxminddb::Reader::open_readfile(path)
            .with_context(|| format!("cannot read GeoIP database {}", path.display()))?;
        reader
            .verify()
            .with_context(|| format!("GeoIP database {} is invalid", path.display()))?;
        Ok(Self { reader })
    }

    /// 国のコード（ISO 3166-1 の2文字の大文字）。分からなければ None。
    /// 調べられないとき（IPv4だけのデータでIPv6を調べたなど）も None にし、ログインは止めない。
    pub fn country(&self, ip: IpAddr) -> Option<String> {
        let code: String = self
            .reader
            .lookup(ip.to_canonical())
            .ok()?
            .decode_path(&maxminddb::path!["country", "iso_code"])
            .ok()??;
        // メールの本文などに入るので、決まった形のものだけを受け取る
        (code.len() == 2 && code.bytes().all(|b| b.is_ascii_uppercase())).then_some(code)
    }
}
