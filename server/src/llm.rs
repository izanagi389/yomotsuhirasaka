//! LLMの呼び出し（外部のOpenAI互換のAPI。10-llm-and-budget.md）。M3は埋め込みだけ。
//!
//! - 送信は外部への送信の共通の仕組み（`egress`）を通す。内部の宛先は、サーバー管理者が例外として許したものだけ。
//! - 応答の本文は、上限を付けて読み、解釈にだけ使う。ログや管理APIの応答には出さない。
//! - 使用量（usage）を返さない実装では、文字数から多めに推定する（推定は上限を超える側に倒さない。Q138）。
//! - JSONモードなどの高度な機能には頼らない（Q7）。

use hyper::header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue};
use serde_json::Value;

use crate::config::LlmConfig;
use crate::egress::{Egress, EgressError, EgressRequest};

/// 呼び出しに失敗した理由。`code` はジョブの失敗の記録などに使う（相手の応答の本文は含めない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmError {
    /// 送れなかった（宛先の検査、接続、上限の時間など）。
    Egress(EgressError),
    /// 成功以外の状態コード。
    Status(u16),
    /// 応答を解釈できない。
    InvalidResponse,
    /// ベクトルの長さが、設定の次元数と違う。
    DimensionMismatch,
}

impl LlmError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Egress(e) => e.code(),
            Self::Status(_) => "http_status",
            Self::InvalidResponse => "invalid_response",
            Self::DimensionMismatch => "dimension_mismatch",
        }
    }

    /// 相手が返した状態コード（あれば）。
    pub fn status(self) -> Option<u16> {
        match self {
            Self::Status(s) => Some(s),
            _ => None,
        }
    }
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Status(s) => write!(f, "http_status {s}"),
            other => f.write_str(other.code()),
        }
    }
}

impl std::error::Error for LlmError {}

/// 埋め込みの結果。`vectors` は入力と同じ順。
#[derive(Debug, Clone, PartialEq)]
pub struct Embeddings {
    pub vectors: Vec<Vec<f32>>,
    /// 使ったトークン数（予算に数える）。
    pub tokens: u64,
    /// 相手が使用量を返さず、文字数から推定したか。
    pub tokens_estimated: bool,
}

/// 使用量を返さない実装のための推定。1文字（Unicode のコードポイント）を1トークンと数える。
/// 実際のトークン数（英語なら4文字前後で1トークン）より多めになり、予算の上限を超える側に倒れない。
pub fn estimate_tokens(inputs: &[String]) -> u64 {
    inputs
        .iter()
        .map(|s| (s.chars().count() as u64).max(1))
        .sum()
}

/// LLMの接続先（インスタンスの既定値）。
#[derive(Clone)]
pub struct Llm {
    egress: Egress,
    config: LlmConfig,
}

impl Llm {
    pub fn new(egress: Egress, config: LlmConfig) -> Self {
        Self { egress, config }
    }

    pub fn config(&self) -> &LlmConfig {
        &self.config
    }

    /// 埋め込みを作る。入力が空なら呼ばない。
    pub async fn embed(&self, inputs: &[String]) -> Result<Embeddings, LlmError> {
        if inputs.is_empty() {
            return Ok(Embeddings {
                vectors: Vec::new(),
                tokens: 0,
                tokens_estimated: false,
            });
        }
        let body = serde_json::json!({ "model": self.config.embedding_model, "input": inputs });
        let auth = HeaderValue::from_str(&format!("Bearer {}", self.config.api_key))
            .map_err(|_| LlmError::InvalidResponse)?;
        let dims = self.config.embedding_dimensions as usize;
        // 数値1つあたり25バイトまで（JSONの浮動小数点数の表記）と、そのほかの項目の分
        let limit = inputs.len() * dims * 25 + 1024 * 1024;
        let response = self
            .egress
            .send(EgressRequest {
                method: hyper::Method::POST,
                url: endpoint(&self.config.base_url, "embeddings"),
                headers: vec![
                    (AUTHORIZATION, auth),
                    (CONTENT_TYPE, HeaderValue::from_static("application/json")),
                ],
                body: serde_json::to_vec(&body).map_err(|_| LlmError::InvalidResponse)?,
                read_body_limit: Some(limit),
            })
            .await
            .map_err(LlmError::Egress)?;
        if !(200..300).contains(&response.status) {
            return Err(LlmError::Status(response.status));
        }
        parse_embeddings(response.body.as_deref().unwrap_or_default(), inputs, dims)
    }
}

/// `<base_url>/<path>`（base_url の末尾の / は1つにまとめる）。
fn endpoint(base_url: &str, path: &str) -> String {
    format!("{}/{path}", base_url.trim_end_matches('/'))
}

/// 埋め込みの応答の解釈。`data[].index` の順に並べ、入力の数とベクトルの長さを確かめる。
fn parse_embeddings(body: &[u8], inputs: &[String], dims: usize) -> Result<Embeddings, LlmError> {
    let json: Value = serde_json::from_slice(body).map_err(|_| LlmError::InvalidResponse)?;
    let data = json
        .get("data")
        .and_then(Value::as_array)
        .ok_or(LlmError::InvalidResponse)?;
    if data.len() != inputs.len() {
        return Err(LlmError::InvalidResponse);
    }
    let mut vectors: Vec<Option<Vec<f32>>> = vec![None; inputs.len()];
    for item in data {
        let index = item
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|i| usize::try_from(i).ok())
            .filter(|&i| i < inputs.len())
            .ok_or(LlmError::InvalidResponse)?;
        let values = item
            .get("embedding")
            .and_then(Value::as_array)
            .ok_or(LlmError::InvalidResponse)?;
        if values.len() != dims {
            return Err(LlmError::DimensionMismatch);
        }
        let vector = values
            .iter()
            .map(|v| v.as_f64().map(|x| x as f32).filter(|x| x.is_finite()))
            .collect::<Option<Vec<f32>>>()
            .ok_or(LlmError::InvalidResponse)?;
        if vectors[index].replace(vector).is_some() {
            return Err(LlmError::InvalidResponse);
        }
    }
    let vectors = vectors
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or(LlmError::InvalidResponse)?;
    let usage = json.get("usage");
    let reported = usage
        .and_then(|u| u.get("prompt_tokens").or_else(|| u.get("total_tokens")))
        .and_then(Value::as_u64);
    Ok(match reported {
        Some(tokens) => Embeddings {
            vectors,
            tokens,
            tokens_estimated: false,
        },
        None => Embeddings {
            vectors,
            tokens: estimate_tokens(inputs),
            tokens_estimated: true,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("text {i}")).collect()
    }

    #[test]
    fn endpoint_joins() {
        assert_eq!(
            endpoint("http://h:1/v1/", "embeddings"),
            "http://h:1/v1/embeddings"
        );
        assert_eq!(
            endpoint("http://h:1/v1", "embeddings"),
            "http://h:1/v1/embeddings"
        );
    }

    #[test]
    fn parses_in_index_order_with_usage() {
        let body = br#"{"data":[{"index":1,"embedding":[0.5,0.5]},{"index":0,"embedding":[1,0]}],"usage":{"prompt_tokens":7,"total_tokens":7}}"#;
        let e = parse_embeddings(body, &inputs(2), 2).unwrap();
        assert_eq!(e.vectors, vec![vec![1.0, 0.0], vec![0.5, 0.5]]);
        assert_eq!((e.tokens, e.tokens_estimated), (7, false));
    }

    #[test]
    fn estimates_when_usage_is_missing() {
        let body = br#"{"data":[{"index":0,"embedding":[1]}]}"#;
        let text = vec!["日本語abc".to_owned()];
        let e = parse_embeddings(body, &text, 1).unwrap();
        assert_eq!((e.tokens, e.tokens_estimated), (6, true));
        // 推定は、ふつうの数え方（4文字で1トークン）より少なくならない
        assert!(estimate_tokens(&text) >= 2);
        assert_eq!(estimate_tokens(&[String::new()]), 1);
    }

    #[test]
    fn rejects_malformed_responses() {
        let two = inputs(2);
        for body in [
            &b"not json"[..],
            br#"{"object":"list","data":["#,
            br#"{"data":[{"index":0,"embedding":[1,0]}]}"#,
            br#"{"data":[{"index":0,"embedding":[1,0]},{"index":0,"embedding":[1,0]}]}"#,
            br#"{"data":[{"index":0,"embedding":[1,0]},{"index":2,"embedding":[1,0]}]}"#,
            br#"{"data":[{"index":0,"embedding":[1,"x"]},{"index":1,"embedding":[1,0]}]}"#,
        ] {
            assert_eq!(
                parse_embeddings(body, &two, 2),
                Err(LlmError::InvalidResponse),
                "{}",
                String::from_utf8_lossy(body)
            );
        }
        assert_eq!(
            parse_embeddings(
                br#"{"data":[{"index":0,"embedding":[1,0,0]}]}"#,
                &inputs(1),
                2
            ),
            Err(LlmError::DimensionMismatch)
        );
    }
}
