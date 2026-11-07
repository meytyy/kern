//! Models and usage as each integration actually reports them. Nothing here is
//! estimated: a value the source does not give stays `None`, and the UI says so.
use serde_json::Value;

/// The words the interface uses for an effort level. Raw values stay out of sight.
pub fn effort_label(level: &str) -> &'static str {
    match level {
        "none" | "minimal" => "Минимум",
        "low" => "Быстрее",
        "medium" => "Стандартно",
        "high" => "Глубже",
        "xhigh" => "Ещё глубже",
        "max" => "Максимум",
        "ultra" => "Максимум+",
        _ => "Другое",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelOption {
    pub id: String,
    pub name: String,
    pub efforts: Vec<String>,
    pub default_effort: Option<String>,
    pub is_default: bool,
    /// Reported by the source; a source that says nothing allows them.
    pub images: bool,
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn int(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(Value::as_i64)
}

/// `model/list` of the Codex App Server.
pub fn parse_codex_models(result: &Value) -> Vec<ModelOption> {
    result
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|m| m.get("hidden").and_then(Value::as_bool) != Some(true))
        .filter_map(|m| {
            Some(ModelOption {
                id: text(m, "id")?,
                name: text(m, "displayName").or_else(|| text(m, "id"))?,
                efforts: m
                    .get("supportedReasoningEfforts")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|e| text(e, "reasoningEffort"))
                    .collect(),
                default_effort: text(m, "defaultReasoningEffort"),
                is_default: m.get("isDefault").and_then(Value::as_bool) == Some(true),
                images: m
                    .get("inputModalities")
                    .and_then(Value::as_array)
                    .is_none_or(|a| a.iter().any(|x| x.as_str() == Some("image"))),
            })
        })
        .collect()
}

/// What to send for a saved choice. An empty or "default" choice sends nothing,
/// so that the connection's own default applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effective {
    pub model: Option<String>,
    pub effort: Option<String>,
    /// The saved model is not offered by this connection.
    pub unavailable: Option<String>,
}

pub fn effective(options: &[ModelOption], saved_model: &str, saved_effort: &str) -> Effective {
    let wants_default = saved_model.is_empty() || saved_model == "default";
    let chosen = if wants_default {
        options.iter().find(|m| m.is_default)
    } else {
        options.iter().find(|m| m.id == saved_model)
    };
    // Before the list arrives the saved choice cannot be judged: send it as it is.
    let unavailable = (!wants_default && !options.is_empty() && chosen.is_none())
        .then(|| saved_model.to_string());
    let model = if wants_default || unavailable.is_some() {
        None
    } else {
        Some(saved_model.to_string())
    };
    let effort = match chosen {
        Some(m) if m.efforts.iter().any(|e| e == saved_effort) => Some(saved_effort.to_string()),
        None if options.is_empty() && !saved_effort.is_empty() => Some(saved_effort.to_string()),
        _ => None,
    };
    Effective {
        model,
        effort,
        unavailable,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RateWindow {
    pub used_percent: i64,
    pub minutes: Option<i64>,
    pub resets_at: Option<i64>,
}

impl RateWindow {
    pub fn label(&self) -> String {
        match self.minutes {
            Some(10080) => "Weekly".to_string(),
            Some(m) if m % 1440 == 0 => format!("{}-day", m / 1440),
            Some(m) if m % 60 == 0 => format!("{}-hour", m / 60),
            Some(m) => format!("{m}-minute"),
            None => "Window".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LimitBucket {
    pub name: String,
    pub windows: Vec<RateWindow>,
    pub reached: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CodexLimits {
    pub plan: Option<String>,
    pub credits: Option<String>,
    pub allowed: Option<bool>,
    pub buckets: Vec<LimitBucket>,
    pub updated_at: i64,
}

fn window(v: Option<&Value>) -> Option<RateWindow> {
    let v = v.filter(|w| !w.is_null())?;
    Some(RateWindow {
        used_percent: int(v, "usedPercent")?,
        minutes: int(v, "windowDurationMins"),
        resets_at: int(v, "resetsAt"),
    })
}

fn bucket(id: &str, snap: &Value) -> LimitBucket {
    LimitBucket {
        name: text(snap, "limitName").unwrap_or_else(|| id.to_string()),
        windows: [snap.get("primary"), snap.get("secondary")]
            .into_iter()
            .filter_map(window)
            .collect(),
        reached: text(snap, "rateLimitReachedType"),
    }
}

fn credits(snap: &Value) -> Option<String> {
    let c = snap.get("credits").filter(|c| !c.is_null())?;
    if c.get("unlimited").and_then(Value::as_bool) == Some(true) {
        return Some("Unlimited".into());
    }
    if c.get("hasCredits").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    text(c, "balance")
}

/// `account/rateLimits/read`.
pub fn parse_codex_limits(result: &Value, now: i64) -> CodexLimits {
    let main = result.get("rateLimits").unwrap_or(&Value::Null);
    let mut buckets: Vec<LimitBucket> = result
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(id, s)| bucket(id, s)).collect())
        .unwrap_or_default();
    if buckets.is_empty() && !main.is_null() {
        buckets.push(bucket(
            main.get("limitId")
                .and_then(Value::as_str)
                .unwrap_or("codex"),
            main,
        ));
    }
    buckets.retain(|b| !b.windows.is_empty());
    CodexLimits {
        plan: text(main, "planType"),
        credits: credits(main),
        allowed: result.get("ordinaryUsageAllowed").and_then(Value::as_bool),
        buckets,
        updated_at: now,
    }
}

/// `account/rateLimits/updated` is sparse: merge what it has.
pub fn merge_codex_update(limits: &mut CodexLimits, snap: &Value, now: i64) {
    let id = snap
        .get("limitId")
        .and_then(Value::as_str)
        .unwrap_or("codex")
        .to_string();
    let incoming = bucket(&id, snap);
    match limits.buckets.iter_mut().find(|b| b.name == incoming.name) {
        Some(have) => {
            if !incoming.windows.is_empty() {
                have.windows = incoming.windows;
            }
            have.reached = incoming.reached;
        }
        None if !incoming.windows.is_empty() => limits.buckets.push(incoming),
        None => {}
    }
    limits.updated_at = now;
}

impl CodexLimits {
    /// The soonest reset of a limit that was actually reached.
    pub fn reached_reset(&self) -> Option<i64> {
        self.buckets
            .iter()
            .filter(|b| b.reached.is_some())
            .flat_map(|b| b.windows.iter())
            .filter(|w| w.used_percent >= 100)
            .filter_map(|w| w.resets_at)
            .min()
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TokenUsage {
    pub input: i64,
    pub output: i64,
    pub cached: i64,
    pub cache_write: i64,
    pub context_used: Option<i64>,
    pub context_window: Option<i64>,
    /// The source's own estimate, in US dollars.
    pub cost_usd: Option<f64>,
    pub models: Vec<String>,
}

/// `thread/tokenUsage/updated`.
pub fn parse_codex_tokens(params: &Value) -> Option<TokenUsage> {
    let usage = params.get("tokenUsage")?;
    let total = usage.get("total")?;
    let last = usage.get("last");
    Some(TokenUsage {
        input: int(total, "inputTokens")?,
        output: int(total, "outputTokens")?,
        cached: int(total, "cachedInputTokens").unwrap_or(0),
        cache_write: int(total, "cacheWriteInputTokens").unwrap_or(0),
        context_used: last.and_then(|l| int(l, "inputTokens")),
        context_window: int(usage, "modelContextWindow"),
        cost_usd: None,
        models: vec![],
    })
}

pub fn fmt_tokens(n: i64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1000.),
        _ => format!("{:.2}M", n as f64 / 1_000_000.),
    }
}

/// "2h 15m", from seconds.
pub fn fmt_span(secs: i64) -> String {
    let s = secs.max(0);
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86399 => format!("{}h {}m", s / 3600, s % 3600 / 60),
        _ => format!("{}d {}h", s / 86400, s % 86400 / 3600),
    }
}

/// Local wall-clock time of a Unix timestamp.
pub fn fmt_clock(ts: i64, now: i64) -> String {
    let t = ts as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers refer to valid local values for the call.
    #[cfg(unix)]
    unsafe {
        libc::localtime_r(&t, &mut tm)
    };
    #[cfg(windows)]
    unsafe {
        libc::localtime_s(&mut tm, &t)
    };
    let clock = format!("{:02}:{:02}", tm.tm_hour, tm.tm_min);
    if ts - now < 86400 {
        return clock;
    }
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{} {} {clock}",
        MONTHS[tm.tm_mon.clamp(0, 11) as usize],
        tm.tm_mday
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Trimmed from a real `account/rateLimits/read` of Codex 0.166.
    fn real_limits() -> Value {
        json!({"ordinaryUsageAllowed": true,
            "rateLimits": {"limitId":"codex","limitName":null,
                "primary":{"usedPercent":30,"windowDurationMins":300,"resetsAt":1794009926},
                "secondary":{"usedPercent":35,"windowDurationMins":10080,"resetsAt":1794437178},
                "credits":{"hasCredits":false,"unlimited":false,"balance":"0"},
                "planType":"plus","rateLimitReachedType":null},
            "rateLimitsByLimitId":{"codex":{"limitId":"codex","limitName":null,
                "primary":{"usedPercent":30,"windowDurationMins":300,"resetsAt":1794009926},
                "secondary":{"usedPercent":35,"windowDurationMins":10080,"resetsAt":1794437178},
                "rateLimitReachedType":null}}})
    }

    #[test]
    fn codex_windows_and_plan_are_read() {
        let l = parse_codex_limits(&real_limits(), 7);
        assert_eq!(l.plan.as_deref(), Some("plus"));
        assert_eq!(l.credits, None);
        assert_eq!(l.allowed, Some(true));
        assert_eq!(l.updated_at, 7);
        assert_eq!(l.buckets.len(), 1);
        let w = &l.buckets[0].windows;
        assert_eq!(w[0].used_percent, 30);
        assert_eq!(w[0].label(), "5-hour");
        assert_eq!(w[1].label(), "Weekly");
        assert_eq!(w[1].resets_at, Some(1794437178));
    }

    #[test]
    fn a_source_without_windows_has_no_buckets() {
        let l = parse_codex_limits(&json!({"rateLimits":{"limitId":"codex","primary":null}}), 1);
        assert!(l.buckets.is_empty());
    }

    #[test]
    fn sparse_updates_keep_what_they_do_not_mention() {
        let mut l = parse_codex_limits(&real_limits(), 1);
        merge_codex_update(&mut l, &json!({"limitId":"codex"}), 2);
        assert_eq!(l.buckets[0].windows.len(), 2);
        assert_eq!(l.updated_at, 2);
        merge_codex_update(
            &mut l,
            &json!({"limitId":"codex","rateLimitReachedType":"rate_limit_reached",
                "primary":{"usedPercent":100,"windowDurationMins":300,"resetsAt":99}}),
            3,
        );
        assert_eq!(l.buckets[0].windows[0].used_percent, 100);
        assert_eq!(l.reached_reset(), Some(99));
    }

    #[test]
    fn codex_models_and_efforts() {
        let m = parse_codex_models(&json!({"data":[
            {"id":"a","displayName":"A","hidden":false,"isDefault":true,"defaultReasoningEffort":"low",
             "supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"max"}]},
            {"id":"h","displayName":"H","hidden":true}]}));
        assert_eq!(m.len(), 1);
        assert!(m[0].is_default);
        assert_eq!(m[0].efforts, vec!["low", "max"]);
    }

    #[test]
    fn the_saved_choice_decides_what_is_sent() {
        let opts = vec![
            ModelOption {
                id: "a".into(),
                name: "A".into(),
                efforts: vec!["low".into(), "high".into()],
                is_default: true,
                ..Default::default()
            },
            ModelOption {
                id: "b".into(),
                name: "B".into(),
                ..Default::default()
            },
        ];
        let e = effective(&opts, "a", "high");
        assert_eq!(
            (e.model.as_deref(), e.effort.as_deref()),
            (Some("a"), Some("high"))
        );
        // An effort this model does not take is not sent.
        assert_eq!(effective(&opts, "b", "high").effort, None);
        assert_eq!(effective(&opts, "a", "ultra").effort, None);
        // Default: nothing is forced, the default model's efforts apply.
        let e = effective(&opts, "", "low");
        assert_eq!((e.model, e.effort.as_deref()), (None, Some("low")));
        // A model the connection no longer offers is reported and not sent.
        let e = effective(&opts, "gone", "low");
        assert_eq!(e.model, None);
        assert_eq!(e.unavailable.as_deref(), Some("gone"));
        // Before the list is known, the choice goes as saved.
        let e = effective(&[], "x", "high");
        assert_eq!(
            (e.model.as_deref(), e.effort.as_deref()),
            (Some("x"), Some("high"))
        );
        assert_eq!(e.unavailable, None);
    }

    #[test]
    fn tokens_from_codex() {
        let c = parse_codex_tokens(&json!({"tokenUsage":{"modelContextWindow":1000,
            "total":{"inputTokens":10,"outputTokens":5,"cachedInputTokens":2,"reasoningOutputTokens":0,"totalTokens":15},
            "last":{"inputTokens":4,"outputTokens":1,"cachedInputTokens":0,"reasoningOutputTokens":0,"totalTokens":5}}}))
        .unwrap();
        assert_eq!((c.input, c.output, c.cached), (10, 5, 2));
        assert_eq!((c.context_used, c.context_window), (Some(4), Some(1000)));
    }

    #[test]
    fn effort_labels_hide_raw_values() {
        for l in ["low", "medium", "high", "xhigh", "max", "ultra"] {
            assert!(!effort_label(l).is_ascii(), "{l}");
        }
        assert_eq!(effort_label("low"), "Быстрее");
        assert_eq!(effort_label("medium"), "Стандартно");
        assert_eq!(effort_label("high"), "Глубже");
    }

    #[test]
    fn codex_says_which_models_take_images() {
        let m = parse_codex_models(&json!({"data":[
            {"id":"a","inputModalities":["text"]},{"id":"b","inputModalities":["text","image"]},{"id":"c"}]}));
        assert_eq!(
            m.iter().map(|x| x.images).collect::<Vec<_>>(),
            vec![false, true, true]
        );
    }

    #[test]
    fn formats() {
        assert_eq!(fmt_tokens(999), "999");
        assert_eq!(fmt_tokens(12_345), "12.3k");
        assert_eq!(fmt_tokens(2_500_000), "2.50M");
        assert_eq!(fmt_span(45), "45s");
        assert_eq!(fmt_span(8100), "2h 15m");
        assert_eq!(fmt_span(90000), "1d 1h");
        assert_eq!(fmt_span(-5), "0s");
    }
}
