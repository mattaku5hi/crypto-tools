//! `getMultipleAccounts` for the Helius endpoint (ADR-019 open-position
//! valuation). Accounts are requested as base64 at an explicit commitment;
//! the response's context slot is returned with the data. Every HTTP attempt
//! goes through the provider's shared `RpcClient`, so it is counted in (and
//! bounded by) the run's request budget. Provider data is external input:
//! the number of values must equal the number requested, each account is
//! size-capped, and malformed base64 is a typed error.

use base64::Engine as _;
use scout_api::ProviderError;
use scout_core::SolanaPubkey;
use serde::Deserialize;

use crate::HeliusProvider;

/// JSON-RPC limit of addresses per `getMultipleAccounts` call.
pub const GET_MULTIPLE_ACCOUNTS_MAX: usize = 100;

/// Largest account data accepted (bytes). The accounts valued here (curve,
/// pool, token vaults) are a few hundred bytes; anything bigger is refused.
pub const MAX_ACCOUNT_DATA_BYTES: usize = 16 * 1024;

/// One existing account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRead {
    pub owner: SolanaPubkey,
    pub lamports: u64,
    pub data: Vec<u8>,
}

/// Result of reading a set of accounts at one commitment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountsRead {
    /// Highest context slot among the chunks (one chunk = 100 addresses).
    pub slot: u64,
    /// Lowest context slot among the chunks (equals `slot` for one chunk).
    pub min_slot: u64,
    /// HTTP calls made (chunks).
    pub calls: u64,
    /// One entry per requested address, in request order; `None` = the
    /// account does not exist at that slot.
    pub accounts: Vec<Option<AccountRead>>,
}

#[derive(Debug, Deserialize)]
struct Context {
    slot: u64,
}

#[derive(Debug, Deserialize)]
struct Value {
    /// `[base64 string, "base64"]`.
    data: (String, String),
    lamports: u64,
    owner: String,
}

#[derive(Debug, Deserialize)]
struct Response {
    context: Context,
    value: Vec<Option<Value>>,
}

fn other(msg: &str) -> ProviderError {
    ProviderError::Other(Box::new(std::io::Error::other(msg.to_string())))
}

fn decode_value(v: Value) -> Result<AccountRead, ProviderError> {
    if v.data.1 != "base64" {
        return Err(other("getMultipleAccounts: unexpected data encoding"));
    }
    // Reject before decoding: base64 expands by 4/3.
    if v.data.0.len() > MAX_ACCOUNT_DATA_BYTES.div_ceil(3).saturating_mul(4) + 4 {
        return Err(other(
            "getMultipleAccounts: account data above the size cap",
        ));
    }
    let data = base64::engine::general_purpose::STANDARD
        .decode(v.data.0.as_bytes())
        .map_err(|_| other("getMultipleAccounts: account data is not valid base64"))?;
    if data.len() > MAX_ACCOUNT_DATA_BYTES {
        return Err(other(
            "getMultipleAccounts: account data above the size cap",
        ));
    }
    let owner: SolanaPubkey = bs58::decode(&v.owner)
        .into_vec()
        .ok()
        .and_then(|b| SolanaPubkey::try_from(b).ok())
        .ok_or_else(|| other("getMultipleAccounts: owner is not a 32-byte base58 key"))?;
    Ok(AccountRead {
        owner,
        lamports: v.lamports,
        data,
    })
}

impl HeliusProvider {
    /// Read `addresses` (in chunks of [`GET_MULTIPLE_ACCOUNTS_MAX`]) with
    /// `commitment` (`"confirmed"`, `"finalized"`, ...). A chunk failure
    /// (including an exhausted request budget) fails the whole read: the
    /// caller decides what stays unvalued. An empty input makes no call.
    pub async fn get_multiple_accounts(
        &self,
        addresses: &[SolanaPubkey],
        commitment: &str,
    ) -> Result<AccountsRead, ProviderError> {
        let mut out = AccountsRead {
            slot: 0,
            min_slot: u64::MAX,
            calls: 0,
            accounts: Vec::with_capacity(addresses.len()),
        };
        for chunk in addresses.chunks(GET_MULTIPLE_ACCOUNTS_MAX) {
            let keys: Vec<String> = chunk
                .iter()
                .map(|a| bs58::encode(a).into_string())
                .collect();
            let params =
                serde_json::json!([keys, {"encoding": "base64", "commitment": commitment}]);
            let resp: Response = self.rpc().call("getMultipleAccounts", params).await?;
            out.calls += 1;
            if resp.value.len() != chunk.len() {
                return Err(other(
                    "getMultipleAccounts: value count differs from the request",
                ));
            }
            out.slot = out.slot.max(resp.context.slot);
            out.min_slot = out.min_slot.min(resp.context.slot);
            for v in resp.value {
                out.accounts.push(v.map(decode_value).transpose()?);
            }
        }
        if out.calls == 0 {
            out.min_slot = 0;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn provider(server: &MockServer) -> HeliusProvider {
        HeliusProvider::new_with_endpoint(scout_rpc::RpcEndpoint::new(server.uri()), 5_000, 1)
            .unwrap()
    }

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[tokio::test]
    async fn reads_present_and_missing_accounts_with_slot_and_counts_calls() {
        let server = MockServer::start().await;
        let owner = bs58::encode([7u8; 32]).into_string();
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0", "id": 1,
                "result": {"context": {"slot": 424242, "apiVersion": "2.0"},
                    "value": [
                        {"data": [b64(&[1,2,3]), "base64"], "executable": false,
                         "lamports": 5, "owner": owner, "rentEpoch": 0, "space": 3},
                        null]}
            })))
            .mount(&server)
            .await;
        let p = provider(&server).with_max_total_requests(Some(3));
        let r = p
            .get_multiple_accounts(&[[1u8; 32], [2u8; 32]], "confirmed")
            .await
            .unwrap();
        assert_eq!((r.slot, r.min_slot, r.calls), (424_242, 424_242, 1));
        assert_eq!(r.accounts.len(), 2);
        let a = r.accounts[0].as_ref().unwrap();
        assert_eq!(
            (a.owner, a.lamports, a.data.as_slice()),
            ([7u8; 32], 5, &[1u8, 2, 3][..])
        );
        assert!(r.accounts[1].is_none());
        assert_eq!(p.total_requests_made(), 1);
        // Request shape: base64 + commitment.
        let req: serde_json::Value =
            serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(req["method"], "getMultipleAccounts");
        assert_eq!(req["params"][1]["encoding"], "base64");
        assert_eq!(req["params"][1]["commitment"], "confirmed");
        assert_eq!(req["params"][0][0], bs58::encode([1u8; 32]).into_string());
    }

    #[tokio::test]
    async fn chunks_of_100_and_empty_input_makes_no_call() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(|req: &wiremock::Request| {
                let v: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
                let n = v["params"][0].as_array().unwrap().len();
                ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0", "id": 1,
                    "result": {"context": {"slot": 9}, "value": vec![serde_json::Value::Null; n]}
                }))
            })
            .mount(&server)
            .await;
        let p = provider(&server);
        let keys: Vec<SolanaPubkey> = (0..=100u8).map(|i| [i; 32]).collect();
        let r = p.get_multiple_accounts(&keys, "confirmed").await.unwrap();
        assert_eq!((r.calls, r.accounts.len()), (2, 101));
        assert_eq!(p.total_requests_made(), 2);
        let r = p.get_multiple_accounts(&[], "confirmed").await.unwrap();
        assert_eq!((r.calls, r.accounts.len()), (0, 0));
        assert_eq!(p.total_requests_made(), 2);
    }

    #[tokio::test]
    async fn budget_exhaustion_is_a_typed_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0", "id": 1,
                "result": {"context": {"slot": 1}, "value": [null]}
            })))
            .mount(&server)
            .await;
        let p = provider(&server).with_max_total_requests(Some(1));
        assert!(
            p.get_multiple_accounts(&[[1u8; 32]], "confirmed")
                .await
                .is_ok()
        );
        assert!(
            p.get_multiple_accounts(&[[1u8; 32]], "confirmed")
                .await
                .is_err()
        );
        assert_eq!(p.total_requests_made(), 1);
    }

    #[tokio::test]
    async fn malformed_provider_data_is_rejected() {
        let owner = bs58::encode([7u8; 32]).into_string();
        for (body, why) in [
            (
                json!({"context": {"slot": 1}, "value": []}),
                "count mismatch",
            ),
            (
                json!({"context": {"slot": 1}, "value": [{"data": ["!!!", "base64"],
                    "lamports": 1, "owner": owner}]}),
                "bad base64",
            ),
            (
                json!({"context": {"slot": 1}, "value": [{"data": ["AAAA", "base58"],
                    "lamports": 1, "owner": owner}]}),
                "encoding",
            ),
            (
                json!({"context": {"slot": 1}, "value": [{"data": ["AAAA", "base64"],
                    "lamports": 1, "owner": "short"}]}),
                "owner",
            ),
            (
                json!({"context": {"slot": 1}, "value": [{"data": [b64(&vec![0u8; MAX_ACCOUNT_DATA_BYTES + 1]), "base64"],
                    "lamports": 1, "owner": owner}]}),
                "oversize",
            ),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"jsonrpc": "2.0", "id": 1, "result": body})),
                )
                .mount(&server)
                .await;
            let r = provider(&server)
                .get_multiple_accounts(&[[1u8; 32]], "confirmed")
                .await;
            assert!(r.is_err(), "{why}");
        }
    }
}
