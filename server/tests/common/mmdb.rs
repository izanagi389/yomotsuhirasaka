//! テスト用の小さな国のデータ（MaxMind DB 形式、IPv4だけ、記録の大きさ24ビット）を作る。
//!
//! 本物のデータは配布の条件があるので使わず、形式の仕様（https://maxmind.github.io/MaxMind-DB/）
//! どおりに、指定したネットワークにだけ `{"country": {"iso_code": ...}}` を持つデータを書き出す。
#![allow(dead_code)]

use std::net::Ipv4Addr;
use std::path::PathBuf;

const EMPTY: u32 = u32::MAX;
const DATA_FLAG: u32 = 1 << 31;

fn control(kind: u8, size: usize) -> Vec<u8> {
    assert!(size < 29);
    if kind <= 7 {
        vec![(kind << 5) | size as u8]
    } else {
        vec![size as u8, kind - 7]
    }
}

fn string(s: &str) -> Vec<u8> {
    let mut out = control(2, s.len());
    out.extend_from_slice(s.as_bytes());
    out
}

fn map(n: usize) -> Vec<u8> {
    control(7, n)
}

fn uint(kind: u8, bytes: &[u8]) -> Vec<u8> {
    let mut out = control(kind, bytes.len());
    out.extend_from_slice(bytes);
    out
}

/// `(ネットワーク, 接頭辞の長さ, 国のコード)` の一覧から、データを書き出してその場所を返す。
pub fn write(networks: &[(Ipv4Addr, u8, &str)]) -> PathBuf {
    // データ部
    let mut data = Vec::new();
    let mut offsets = Vec::new();
    for (_, _, code) in networks {
        offsets.push(data.len() as u32);
        data.extend(map(1));
        data.extend(string("country"));
        data.extend(map(1));
        data.extend(string("iso_code"));
        data.extend(string(code));
    }

    // 探索木（各ノードは左右2つの記録）
    let mut nodes: Vec<[u32; 2]> = vec![[EMPTY, EMPTY]];
    for (i, (net, prefix, _)) in networks.iter().enumerate() {
        let bits = u32::from(*net);
        let mut node = 0usize;
        for depth in 0..*prefix {
            let bit = ((bits >> (31 - depth)) & 1) as usize;
            if depth + 1 == *prefix {
                nodes[node][bit] = DATA_FLAG | offsets[i];
            } else {
                if nodes[node][bit] == EMPTY {
                    nodes.push([EMPTY, EMPTY]);
                    nodes[node][bit] = (nodes.len() - 1) as u32;
                }
                node = nodes[node][bit] as usize;
            }
        }
    }
    let count = nodes.len() as u32;
    let mut out = Vec::new();
    for node in &nodes {
        for &record in node {
            let value = if record == EMPTY {
                count
            } else if record & DATA_FLAG != 0 {
                count + 16 + (record & !DATA_FLAG)
            } else {
                record
            };
            out.extend_from_slice(&value.to_be_bytes()[1..]);
        }
    }
    out.extend_from_slice(&[0u8; 16]);
    out.extend(data);

    // メタデータ
    out.extend_from_slice(b"\xAB\xCD\xEFMaxMind.com");
    out.extend(map(9));
    out.extend(string("binary_format_major_version"));
    out.extend(uint(5, &2u16.to_be_bytes()));
    out.extend(string("binary_format_minor_version"));
    out.extend(uint(5, &0u16.to_be_bytes()));
    out.extend(string("build_epoch"));
    out.extend(uint(9, &1_700_000_000u64.to_be_bytes()));
    out.extend(string("database_type"));
    out.extend(string("Test-Country"));
    out.extend(string("description"));
    out.extend(map(1));
    out.extend(string("en"));
    out.extend(string("test data"));
    out.extend(string("ip_version"));
    out.extend(uint(5, &4u16.to_be_bytes()));
    out.extend(string("languages"));
    out.extend(control(11, 1));
    out.extend(string("en"));
    out.extend(string("node_count"));
    out.extend(uint(6, &count.to_be_bytes()));
    out.extend(string("record_size"));
    out.extend(uint(5, &24u16.to_be_bytes()));

    let path = std::env::temp_dir().join(format!("cms-test-geoip-{}.mmdb", uuid::Uuid::new_v4()));
    std::fs::write(&path, out).unwrap();
    path
}
