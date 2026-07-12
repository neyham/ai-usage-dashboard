//! DeepSeek balance fetcher. The key is resolved at call time from the OS
//! credential store / env / config and never persisted or logged.

use super::{send_with_one_retry, Resp};
use crate::config::Config;
use crate::models::DeepSeekService;
use crate::secrets;
use anyhow::{anyhow, bail, Context};
use reqwest::Client;
use serde_json::Value;

const BALANCE_URL: &str = "https://api.deepseek.com/user/balance";

pub async fn fetch(config: &Config, client: &Client) -> anyhow::Result<DeepSeekService> {
    let key = secrets::deepseek_key(config).ok_or_else(|| anyhow!("DeepSeek API key missing"))?;

    let resp: Resp = send_with_one_retry(|| {
        client
            .get(BALANCE_URL)
            .header("Authorization", format!("Bearer {key}"))
            .header("Content-Type", "application/json")
    })
    .await?;

    if !resp.is_success() {
        bail!("DeepSeek balance HTTP {}", resp.status);
    }
    parse_balance(&resp.body)
}

pub(crate) fn parse_balance(body: &str) -> anyhow::Result<DeepSeekService> {
    let root: Value = serde_json::from_str(body).context("parse DeepSeek balance body")?;
    let is_available = root
        .get("is_available")
        .and_then(Value::as_bool)
        .context("DeepSeek balance missing is_available")?;
    let infos = root
        .get("balance_infos")
        .and_then(Value::as_array)
        .context("DeepSeek balance missing balance_infos")?;

    // Prefer the CNY entry; otherwise take the first.
    let selected = infos
        .iter()
        .find(|i| {
            i.get("currency")
                .and_then(Value::as_str)
                .map(|c| c.eq_ignore_ascii_case("CNY"))
                .unwrap_or(false)
        })
        .or_else(|| infos.first());

    let selected = selected.context("DeepSeek balance_infos is empty")?;
    let currency = selected
        .get("currency")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .context("DeepSeek balance missing currency")?;
    let balance = selected
        .get("total_balance")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .context("DeepSeek balance missing total_balance")?;

    Ok(DeepSeekService {
        status: if is_available {
            "NOMINAL".into()
        } else {
            "INSUFFICIENT BALANCE".into()
        },
        from_cache: false,
        data_may_be_stale: false,
        currency: Some(currency.to_string()),
        balance: Some(balance.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::parse_balance;

    #[test]
    fn deepseek_reports_unavailable_balance_without_discarding_amount() {
        let service = parse_balance(
            r#"{
                "is_available": false,
                "balance_infos": [{"currency":"CNY","total_balance":"0.00"}]
            }"#,
        )
        .expect("valid unavailable balance response");

        assert_eq!(service.status, "INSUFFICIENT BALANCE");
        assert_eq!(service.currency.as_deref(), Some("CNY"));
        assert_eq!(service.balance.as_deref(), Some("0.00"));
    }

    #[test]
    fn deepseek_balance_requires_availability_and_amount_fields() {
        for body in [
            r#"{"balance_infos":[{"currency":"CNY","total_balance":"1.00"}]}"#,
            r#"{"is_available":true,"balance_infos":[]}"#,
            r#"{"is_available":true,"balance_infos":[{"currency":"CNY"}]}"#,
        ] {
            assert!(parse_balance(body).is_err(), "unexpectedly accepted {body}");
        }
    }
}
