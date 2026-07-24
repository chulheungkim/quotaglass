use serde::{Deserialize, Serialize};

pub mod claude;
pub mod codex;

#[derive(Clone, Copy, Debug, Deserialize, Hash, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Claude,
    Codex,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderLimit {
    pub id: String,
    pub title: String,
    pub utilization: Option<f64>,
    pub resets_at: Option<String>,
    pub window_minutes: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderLimits {
    pub provider: ProviderId,
    pub windows: Vec<ProviderLimit>,
    pub stale: bool,
    pub cached_at: Option<u64>,
    pub plan: Option<String>,
    pub credit_balance: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryMetric {
    pub label: String,
    pub value: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityPoint {
    pub date: String,
    pub value: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakdownRow {
    pub key: String,
    pub value: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStats {
    pub provider: ProviderId,
    pub metrics: Vec<SummaryMetric>,
    pub activity_label: String,
    pub daily14: Vec<ActivityPoint>,
    pub breakdown_label: String,
    pub breakdown: Vec<BreakdownRow>,
    pub footer: Vec<String>,
    pub since: Option<String>,
    pub data_scope: Option<String>,
    pub last_updated: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_ids_deserialize_from_frontend_values() {
        assert_eq!(
            serde_json::from_str::<ProviderId>("\"claude\"").unwrap(),
            ProviderId::Claude
        );
        assert_eq!(
            serde_json::from_str::<ProviderId>("\"codex\"").unwrap(),
            ProviderId::Codex
        );
    }

    #[test]
    fn limits_serialize_as_dynamic_windows() {
        let limits = ProviderLimits {
            provider: ProviderId::Codex,
            windows: vec![ProviderLimit {
                id: "primary".into(),
                title: "Current session".into(),
                utilization: Some(25.0),
                resets_at: Some("2026-07-24T12:00:00Z".into()),
                window_minutes: Some(300),
            }],
            stale: false,
            cached_at: None,
            plan: Some("plus".into()),
            credit_balance: None,
        };
        let json = serde_json::to_value(limits).unwrap();
        assert_eq!(json["provider"], "codex");
        assert_eq!(json["windows"].as_array().unwrap().len(), 1);
        assert_eq!(json["windows"][0]["windowMinutes"], 300);
    }
}
